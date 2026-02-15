use rusqlite::Connection;

use crate::error::Result;

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
