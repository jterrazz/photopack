//! Catalog database schema and migration framework.
//!
//! ## Versioning strategy
//!
//! The catalog stores its schema version in the `config` table under the
//! `schema_version` key. On every open, [`migrate`] compares the stored
//! version against [`SCHEMA_VERSION`]:
//!
//! - **DB version == code version** → no-op.
//! - **DB version < code version** → run pending migrations in a transaction.
//! - **DB version > code version** → fail with [`Error::SchemaTooNew`] so the
//!   user knows to upgrade photopack.
//! - **No version key** (pre-versioning DB) → auto-set to 1.
//!
//! ## Adding a migration
//!
//! 1. Increment [`SCHEMA_VERSION`].
//! 2. Write a `fn(conn: &Connection) -> Result<()>` that performs the DDL/DML.
//! 3. Append it to [`MIGRATIONS`]. The array index maps to the transition:
//!    `MIGRATIONS[0]` = v1→v2, `MIGRATIONS[1]` = v2→v3, etc.

use rusqlite::{params, Connection};

use crate::error::{Error, Result};

/// Current schema version. Bump when adding a migration.
pub const SCHEMA_VERSION: i64 = 1;

/// Ordered list of migrations. `MIGRATIONS[i]` migrates from version `i+1` to `i+2`.
pub const MIGRATIONS: &[fn(&Connection) -> Result<()>] = &[];

pub fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sources (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            path        TEXT NOT NULL UNIQUE,
            last_scanned INTEGER
        );

        CREATE TABLE IF NOT EXISTS photos (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            source_id   INTEGER NOT NULL REFERENCES sources(id),
            path        TEXT NOT NULL UNIQUE,
            size        INTEGER NOT NULL,
            format      TEXT NOT NULL,
            sha256      TEXT NOT NULL,
            phash       INTEGER,
            dhash       INTEGER,
            mtime       INTEGER NOT NULL,
            exif_date       TEXT,
            exif_camera_make  TEXT,
            exif_camera_model TEXT,
            exif_gps_lat     REAL,
            exif_gps_lon     REAL,
            exif_width       INTEGER,
            exif_height      INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_photos_sha256 ON photos(sha256);
        CREATE INDEX IF NOT EXISTS idx_photos_source ON photos(source_id);
        CREATE INDEX IF NOT EXISTS idx_photos_path ON photos(path);
        CREATE INDEX IF NOT EXISTS idx_photos_source_mtime ON photos(source_id, mtime);

        CREATE TABLE IF NOT EXISTS duplicate_groups (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            source_of_truth_id INTEGER NOT NULL REFERENCES photos(id),
            confidence      TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS group_members (
            group_id    INTEGER NOT NULL REFERENCES duplicate_groups(id),
            photo_id    INTEGER NOT NULL REFERENCES photos(id),
            PRIMARY KEY (group_id, photo_id)
        );

        CREATE INDEX IF NOT EXISTS idx_group_members_photo ON group_members(photo_id);

        CREATE TABLE IF NOT EXISTS config (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

/// Read the schema version from the config table. Returns 0 if the key is absent
/// (pre-versioning database).
fn get_schema_version(conn: &Connection) -> Result<i64> {
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM config WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(version.and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// Write the schema version into the config table.
fn set_schema_version(conn: &Connection, version: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO config (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![version.to_string()],
    )?;
    Ok(())
}

/// Run pending migrations and update the stored schema version.
///
/// Must be called **after** [`initialize`] so the config table exists.
pub fn migrate(conn: &Connection) -> Result<()> {
    let db_version = get_schema_version(conn);

    // Treat version 0 (no key) as pre-versioning — set to 1 (initial schema).
    let db_version = match db_version {
        Ok(0) => {
            set_schema_version(conn, 1)?;
            1
        }
        Ok(v) => v,
        Err(e) => return Err(e),
    };

    if db_version > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew {
            db: db_version,
            code: SCHEMA_VERSION,
        });
    }

    // Run pending migrations inside a transaction.
    if db_version < SCHEMA_VERSION {
        let tx = conn.unchecked_transaction()?;
        for migration in MIGRATIONS.iter().skip(db_version as usize) {
            migration(&tx)?;
        }
        set_schema_version(&tx, SCHEMA_VERSION)?;
        tx.commit()?;
    }

    Ok(())
}
