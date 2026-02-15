use std::fs;
use std::path::Path;

use rusqlite::Connection;

use crate::error::Result;

/// Embedded manifest stored inside the pack directory at `.photopack/manifest.sqlite`.
/// Maps SHA-256 hashes to file metadata, enabling integrity verification and cleanup.
pub struct Manifest {
    conn: Connection,
}

impl Manifest {
    /// Open (or create) the manifest database inside `pack_path/.photopack/`.
    /// Creates the `.photopack/` directory, `manifest.sqlite`, and `version` file.
    pub fn open(pack_path: &Path) -> Result<Self> {
        let meta_dir = pack_path.join(".photopack");
        fs::create_dir_all(&meta_dir)?;

        let db_path = meta_dir.join("manifest.sqlite");
        let conn = Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS metadata (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pack_files (
                sha256            TEXT PRIMARY KEY,
                original_filename TEXT NOT NULL,
                format            TEXT NOT NULL,
                size              INTEGER NOT NULL,
                exif_date         TEXT,
                camera_make       TEXT,
                camera_model      TEXT,
                added_at          TEXT NOT NULL
            );",
        )?;

        // Seed version metadata if missing
        conn.execute(
            "INSERT OR IGNORE INTO metadata (key, value) VALUES ('version', '1')",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO metadata (key, value) VALUES ('created_at', datetime('now'))",
            [],
        )?;

        // Write version text file
        fs::write(meta_dir.join("version"), "1")?;

        Ok(Self { conn })
    }

    /// Insert or replace a pack file entry.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_file(
        &self,
        sha256: &str,
        original_filename: &str,
        format: &str,
        size: u64,
        exif_date: Option<&str>,
        camera_make: Option<&str>,
        camera_model: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO pack_files
                (sha256, original_filename, format, size, exif_date, camera_make, camera_model, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
            rusqlite::params![sha256, original_filename, format, size as i64, exif_date, camera_make, camera_model],
        )?;
        Ok(())
    }

    /// Check if a SHA-256 hash exists in the manifest.
    pub fn contains(&self, sha256: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM pack_files WHERE sha256 = ?1",
            [sha256],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Remove a pack file entry. Returns true if a row was deleted.
    pub fn remove(&self, sha256: &str) -> Result<bool> {
        let deleted = self.conn.execute(
            "DELETE FROM pack_files WHERE sha256 = ?1",
            [sha256],
        )?;
        Ok(deleted > 0)
    }

    /// List all entries as `(sha256, format)` pairs.
    pub fn list_entries(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT sha256, format FROM pack_files")?;
        let entries = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(entries)
    }

    /// Get the manifest version string.
    pub fn version(&self) -> Result<String> {
        let version: String = self.conn.query_row(
            "SELECT value FROM metadata WHERE key = 'version'",
            [],
            |row| row.get(0),
        )?;
        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_open_creates_db() {
        let tmp = tempfile::tempdir().unwrap();
        let _manifest = Manifest::open(tmp.path()).unwrap();
        assert!(tmp.path().join(".photopack/manifest.sqlite").exists());
        assert!(tmp.path().join(".photopack/version").exists());
    }

    #[test]
    fn test_manifest_version() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Manifest::open(tmp.path()).unwrap();
        assert_eq!(manifest.version().unwrap(), "1");
    }

    #[test]
    fn test_manifest_insert_and_contains() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Manifest::open(tmp.path()).unwrap();

        assert!(!manifest.contains("abc123").unwrap());
        manifest
            .insert_file("abc123", "photo.jpg", "JPEG", 1024, None, None, None)
            .unwrap();
        assert!(manifest.contains("abc123").unwrap());
    }

    #[test]
    fn test_manifest_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Manifest::open(tmp.path()).unwrap();

        manifest
            .insert_file("abc123", "photo.jpg", "JPEG", 1024, None, None, None)
            .unwrap();
        assert!(manifest.contains("abc123").unwrap());

        let removed = manifest.remove("abc123").unwrap();
        assert!(removed);
        assert!(!manifest.contains("abc123").unwrap());

        // Removing again returns false
        let removed_again = manifest.remove("abc123").unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn test_manifest_list_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Manifest::open(tmp.path()).unwrap();

        manifest
            .insert_file("aaa", "a.jpg", "JPEG", 100, None, None, None)
            .unwrap();
        manifest
            .insert_file("bbb", "b.cr2", "CR2", 200, None, None, None)
            .unwrap();

        let entries = manifest.list_entries().unwrap();
        assert_eq!(entries.len(), 2);

        let sha_set: std::collections::HashSet<&str> =
            entries.iter().map(|(s, _)| s.as_str()).collect();
        assert!(sha_set.contains("aaa"));
        assert!(sha_set.contains("bbb"));
    }

    #[test]
    fn test_manifest_insert_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Manifest::open(tmp.path()).unwrap();

        manifest
            .insert_file("abc123", "photo.jpg", "JPEG", 1024, None, None, None)
            .unwrap();
        // Insert again with different metadata — should succeed (OR REPLACE)
        manifest
            .insert_file(
                "abc123",
                "renamed.jpg",
                "JPEG",
                2048,
                Some("2024-01-01"),
                Some("Canon"),
                Some("EOS R5"),
            )
            .unwrap();

        let entries = manifest.list_entries().unwrap();
        assert_eq!(entries.len(), 1);
    }

    // ── Schema safety ───────────────────────────────────────────

    #[test]
    fn test_manifest_tables_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Manifest::open(tmp.path()).unwrap();
        let mut stmt = manifest
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(tables, vec!["metadata", "pack_files"]);
    }

    #[test]
    fn test_manifest_pack_files_columns() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Manifest::open(tmp.path()).unwrap();
        let mut stmt = manifest
            .conn
            .prepare("SELECT name FROM pragma_table_info('pack_files') ORDER BY cid")
            .unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            columns,
            vec![
                "sha256", "original_filename", "format", "size",
                "exif_date", "camera_make", "camera_model", "added_at",
            ]
        );
    }

    #[test]
    fn test_manifest_data_survives_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let manifest = Manifest::open(tmp.path()).unwrap();
            manifest
                .insert_file("abc123", "photo.jpg", "JPEG", 1024, None, None, None)
                .unwrap();
        }
        {
            let manifest = Manifest::open(tmp.path()).unwrap();
            assert!(manifest.contains("abc123").unwrap());
            let entries = manifest.list_entries().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].0, "abc123");
        }
    }
}
