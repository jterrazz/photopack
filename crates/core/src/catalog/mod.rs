pub mod schema;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::domain::*;
use crate::error::{Error, Result};

/// SQLite-backed catalog for photo metadata and duplicate groups.
pub struct Catalog {
    conn: Connection,
}

impl Catalog {
    /// Open or create a catalog at the given path with WAL mode.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::initialize(&conn)?;
        Ok(Self { conn })
    }

    /// Open an in-memory catalog (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::initialize(&conn)?;
        Ok(Self { conn })
    }

    // ── Sources ──────────────────────────────────────────────────────

    pub fn add_source(&self, path: &Path) -> Result<Source> {
        let canonical = path.canonicalize()?;
        let path_str = canonical.to_string_lossy();

        // Check if already exists
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sources WHERE path = ?1",
                params![path_str.as_ref()],
                |row| row.get(0),
            )
            .ok();

        if existing.is_some() {
            return Err(Error::SourceAlreadyExists(canonical));
        }

        self.conn.execute(
            "INSERT INTO sources (path) VALUES (?1)",
            params![path_str.as_ref()],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(Source {
            id,
            path: canonical,
            last_scanned: None,
        })
    }

    pub fn list_sources(&self) -> Result<Vec<Source>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, last_scanned FROM sources")?;
        let sources = stmt
            .query_map([], |row| {
                Ok(Source {
                    id: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                    last_scanned: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(sources)
    }

    pub fn update_source_scanned(&self, source_id: i64, timestamp: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE sources SET last_scanned = ?1 WHERE id = ?2",
            params![timestamp, source_id],
        )?;
        Ok(())
    }

    // ── Photos ───────────────────────────────────────────────────────

    pub fn upsert_photo(&self, photo: &PhotoFile) -> Result<i64> {
        let path_str = photo.path.to_string_lossy();
        let format_str = photo.format.as_str();

        // Try to get existing photo by path
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM photos WHERE path = ?1",
                params![path_str.as_ref()],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing_id {
            self.conn.execute(
                "UPDATE photos SET source_id=?1, size=?2, format=?3, sha256=?4, phash=?5, dhash=?6, mtime=?7,
                 exif_date=?8, exif_camera_make=?9, exif_camera_model=?10, exif_gps_lat=?11, exif_gps_lon=?12,
                 exif_width=?13, exif_height=?14
                 WHERE id=?15",
                params![
                    photo.source_id,
                    photo.size as i64,
                    format_str,
                    photo.sha256,
                    photo.phash.map(|v| v as i64),
                    photo.dhash.map(|v| v as i64),
                    photo.mtime,
                    photo.exif.as_ref().and_then(|e| e.date.clone()),
                    photo.exif.as_ref().and_then(|e| e.camera_make.clone()),
                    photo.exif.as_ref().and_then(|e| e.camera_model.clone()),
                    photo.exif.as_ref().and_then(|e| e.gps_lat),
                    photo.exif.as_ref().and_then(|e| e.gps_lon),
                    photo.exif.as_ref().and_then(|e| e.width),
                    photo.exif.as_ref().and_then(|e| e.height),
                    id,
                ],
            )?;
            Ok(id)
        } else {
            self.conn.execute(
                "INSERT INTO photos (source_id, path, size, format, sha256, phash, dhash, mtime,
                 exif_date, exif_camera_make, exif_camera_model, exif_gps_lat, exif_gps_lon, exif_width, exif_height)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                params![
                    photo.source_id,
                    path_str.as_ref(),
                    photo.size as i64,
                    format_str,
                    photo.sha256,
                    photo.phash.map(|v| v as i64),
                    photo.dhash.map(|v| v as i64),
                    photo.mtime,
                    photo.exif.as_ref().and_then(|e| e.date.clone()),
                    photo.exif.as_ref().and_then(|e| e.camera_make.clone()),
                    photo.exif.as_ref().and_then(|e| e.camera_model.clone()),
                    photo.exif.as_ref().and_then(|e| e.gps_lat),
                    photo.exif.as_ref().and_then(|e| e.gps_lon),
                    photo.exif.as_ref().and_then(|e| e.width),
                    photo.exif.as_ref().and_then(|e| e.height),
                ],
            )?;
            Ok(self.conn.last_insert_rowid())
        }
    }

    /// Upsert multiple photos in a single transaction for bulk performance.
    pub fn upsert_photos_batch(&mut self, photos: &[PhotoFile]) -> Result<Vec<i64>> {
        let tx = self.conn.transaction()?;
        let mut ids = Vec::with_capacity(photos.len());

        for photo in photos {
            let path_str = photo.path.to_string_lossy();
            let format_str = photo.format.as_str();

            let existing_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM photos WHERE path = ?1",
                    params![path_str.as_ref()],
                    |row| row.get(0),
                )
                .ok();

            if let Some(id) = existing_id {
                tx.execute(
                    "UPDATE photos SET source_id=?1, size=?2, format=?3, sha256=?4, phash=?5, dhash=?6, mtime=?7,
                     exif_date=?8, exif_camera_make=?9, exif_camera_model=?10, exif_gps_lat=?11, exif_gps_lon=?12,
                     exif_width=?13, exif_height=?14
                     WHERE id=?15",
                    params![
                        photo.source_id,
                        photo.size as i64,
                        format_str,
                        photo.sha256,
                        photo.phash.map(|v| v as i64),
                        photo.dhash.map(|v| v as i64),
                        photo.mtime,
                        photo.exif.as_ref().and_then(|e| e.date.clone()),
                        photo.exif.as_ref().and_then(|e| e.camera_make.clone()),
                        photo.exif.as_ref().and_then(|e| e.camera_model.clone()),
                        photo.exif.as_ref().and_then(|e| e.gps_lat),
                        photo.exif.as_ref().and_then(|e| e.gps_lon),
                        photo.exif.as_ref().and_then(|e| e.width),
                        photo.exif.as_ref().and_then(|e| e.height),
                        id,
                    ],
                )?;
                ids.push(id);
            } else {
                tx.execute(
                    "INSERT INTO photos (source_id, path, size, format, sha256, phash, dhash, mtime,
                     exif_date, exif_camera_make, exif_camera_model, exif_gps_lat, exif_gps_lon, exif_width, exif_height)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                    params![
                        photo.source_id,
                        path_str.as_ref(),
                        photo.size as i64,
                        format_str,
                        photo.sha256,
                        photo.phash.map(|v| v as i64),
                        photo.dhash.map(|v| v as i64),
                        photo.mtime,
                        photo.exif.as_ref().and_then(|e| e.date.clone()),
                        photo.exif.as_ref().and_then(|e| e.camera_make.clone()),
                        photo.exif.as_ref().and_then(|e| e.camera_model.clone()),
                        photo.exif.as_ref().and_then(|e| e.gps_lat),
                        photo.exif.as_ref().and_then(|e| e.gps_lon),
                        photo.exif.as_ref().and_then(|e| e.width),
                        photo.exif.as_ref().and_then(|e| e.height),
                    ],
                )?;
                ids.push(tx.last_insert_rowid());
            }
        }

        tx.commit()?;
        Ok(ids)
    }

    pub fn get_photo_mtime(&self, path: &Path) -> Result<Option<i64>> {
        let path_str = path.to_string_lossy();
        let mtime = self
            .conn
            .query_row(
                "SELECT mtime FROM photos WHERE path = ?1",
                params![path_str.as_ref()],
                |row| row.get(0),
            )
            .ok();
        Ok(mtime)
    }

    pub fn list_all_photos(&self) -> Result<Vec<PhotoFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_id, path, size, format, sha256, phash, dhash, mtime,
             exif_date, exif_camera_make, exif_camera_model, exif_gps_lat, exif_gps_lon,
             exif_width, exif_height
             FROM photos",
        )?;
        let photos = stmt
            .query_map([], |row| {
                let exif_date: Option<String> = row.get(9)?;
                let exif_make: Option<String> = row.get(10)?;
                let exif_model: Option<String> = row.get(11)?;
                let exif_lat: Option<f64> = row.get(12)?;
                let exif_lon: Option<f64> = row.get(13)?;
                let exif_w: Option<u32> = row.get(14)?;
                let exif_h: Option<u32> = row.get(15)?;

                let exif = if exif_date.is_some()
                    || exif_make.is_some()
                    || exif_model.is_some()
                    || exif_lat.is_some()
                {
                    Some(ExifData {
                        date: exif_date,
                        camera_make: exif_make,
                        camera_model: exif_model,
                        gps_lat: exif_lat,
                        gps_lon: exif_lon,
                        width: exif_w,
                        height: exif_h,
                    })
                } else {
                    None
                };

                Ok(PhotoFile {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    path: PathBuf::from(row.get::<_, String>(2)?),
                    size: row.get::<_, i64>(3)? as u64,
                    format: parse_format(&row.get::<_, String>(4)?),
                    sha256: row.get(5)?,
                    phash: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                    dhash: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                    exif,
                    mtime: row.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(photos)
    }

    pub fn count_photos(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM photos", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Get all catalog statistics in a single query for the status dashboard.
    pub fn stats_summary(&self) -> Result<(usize, usize, usize)> {
        let (photos, groups, duplicates) = self.conn.query_row(
            "SELECT
                (SELECT COUNT(*) FROM photos),
                (SELECT COUNT(*) FROM duplicate_groups),
                (SELECT COUNT(DISTINCT gm.photo_id) FROM group_members gm
                 JOIN duplicate_groups dg ON gm.group_id = dg.id
                 WHERE gm.photo_id != dg.source_of_truth_id)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as usize,
                    row.get::<_, i64>(1)? as usize,
                    row.get::<_, i64>(2)? as usize,
                ))
            },
        )?;
        Ok((photos, groups, duplicates))
    }

    // ── Duplicate Groups ─────────────────────────────────────────────

    pub fn clear_groups(&self) -> Result<()> {
        self.conn.execute("DELETE FROM group_members", [])?;
        self.conn.execute("DELETE FROM duplicate_groups", [])?;
        Ok(())
    }

    pub fn insert_group(&self, source_of_truth_id: i64, confidence: Confidence, member_ids: &[i64]) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO duplicate_groups (source_of_truth_id, confidence) VALUES (?1, ?2)",
            params![source_of_truth_id, confidence.as_str()],
        )?;
        let group_id = self.conn.last_insert_rowid();

        let mut stmt = self
            .conn
            .prepare("INSERT INTO group_members (group_id, photo_id) VALUES (?1, ?2)")?;
        for &photo_id in member_ids {
            stmt.execute(params![group_id, photo_id])?;
        }
        Ok(group_id)
    }

    /// Clear existing groups and insert new ones in a single transaction.
    pub fn replace_groups_batch(&mut self, groups: &[(i64, Confidence, Vec<i64>)]) -> Result<Vec<i64>> {
        let tx = self.conn.transaction()?;

        tx.execute("DELETE FROM group_members", [])?;
        tx.execute("DELETE FROM duplicate_groups", [])?;

        let mut group_ids = Vec::with_capacity(groups.len());

        for (source_of_truth_id, confidence, member_ids) in groups {
            tx.execute(
                "INSERT INTO duplicate_groups (source_of_truth_id, confidence) VALUES (?1, ?2)",
                params![source_of_truth_id, confidence.as_str()],
            )?;
            let group_id = tx.last_insert_rowid();

            for &photo_id in member_ids {
                tx.execute(
                    "INSERT INTO group_members (group_id, photo_id) VALUES (?1, ?2)",
                    params![group_id, photo_id],
                )?;
            }
            group_ids.push(group_id);
        }

        tx.commit()?;
        Ok(group_ids)
    }

    pub fn list_groups(&self) -> Result<Vec<DuplicateGroup>> {
        // Single JOIN query to avoid N+1 problem
        let mut stmt = self.conn.prepare(
            "SELECT dg.id, dg.source_of_truth_id, dg.confidence,
                    p.id, p.source_id, p.path, p.size, p.format, p.sha256, p.phash, p.dhash, p.mtime,
                    p.exif_date, p.exif_camera_make, p.exif_camera_model, p.exif_gps_lat, p.exif_gps_lon,
                    p.exif_width, p.exif_height
             FROM duplicate_groups dg
             JOIN group_members gm ON gm.group_id = dg.id
             JOIN photos p ON p.id = gm.photo_id
             ORDER BY dg.id",
        )?;

        let rows = stmt
            .query_map([], |row| {
                let exif_date: Option<String> = row.get(12)?;
                let exif_make: Option<String> = row.get(13)?;
                let exif_model: Option<String> = row.get(14)?;
                let exif_lat: Option<f64> = row.get(15)?;
                let exif_lon: Option<f64> = row.get(16)?;
                let exif_w: Option<u32> = row.get(17)?;
                let exif_h: Option<u32> = row.get(18)?;

                let exif = if exif_date.is_some()
                    || exif_make.is_some()
                    || exif_model.is_some()
                    || exif_lat.is_some()
                {
                    Some(ExifData {
                        date: exif_date,
                        camera_make: exif_make,
                        camera_model: exif_model,
                        gps_lat: exif_lat,
                        gps_lon: exif_lon,
                        width: exif_w,
                        height: exif_h,
                    })
                } else {
                    None
                };

                Ok((
                    row.get::<_, i64>(0)?,       // group id
                    row.get::<_, i64>(1)?,       // sot_id
                    row.get::<_, String>(2)?,    // confidence
                    PhotoFile {
                        id: row.get(3)?,
                        source_id: row.get(4)?,
                        path: PathBuf::from(row.get::<_, String>(5)?),
                        size: row.get::<_, i64>(6)? as u64,
                        format: parse_format(&row.get::<_, String>(7)?),
                        sha256: row.get(8)?,
                        phash: row.get::<_, Option<i64>>(9)?.map(|v| v as u64),
                        dhash: row.get::<_, Option<i64>>(10)?.map(|v| v as u64),
                        exif,
                        mtime: row.get(11)?,
                    },
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Group rows by group_id
        let mut group_map: HashMap<i64, (i64, String, Vec<PhotoFile>)> = HashMap::new();
        let mut group_order: Vec<i64> = Vec::new();

        for (group_id, sot_id, conf_str, photo) in rows {
            let entry = group_map
                .entry(group_id)
                .or_insert_with(|| {
                    group_order.push(group_id);
                    (sot_id, conf_str.clone(), Vec::new())
                });
            entry.2.push(photo);
        }

        let result = group_order
            .into_iter()
            .map(|gid| {
                let (sot_id, conf_str, members) = group_map.remove(&gid).unwrap();
                DuplicateGroup {
                    id: gid,
                    members,
                    source_of_truth_id: sot_id,
                    confidence: parse_confidence(&conf_str),
                }
            })
            .collect();

        Ok(result)
    }

    pub fn get_group(&self, group_id: i64) -> Result<DuplicateGroup> {
        let (sot_id, conf_str) = self
            .conn
            .query_row(
                "SELECT source_of_truth_id, confidence FROM duplicate_groups WHERE id = ?1",
                params![group_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|_| Error::GroupNotFound(group_id))?;

        let members = self.get_group_members(group_id)?;
        Ok(DuplicateGroup {
            id: group_id,
            members,
            source_of_truth_id: sot_id,
            confidence: parse_confidence(&conf_str),
        })
    }

    pub fn count_groups(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM duplicate_groups", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn count_duplicate_photos(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT gm.photo_id) FROM group_members gm
                 JOIN duplicate_groups dg ON gm.group_id = dg.id
                 WHERE gm.photo_id != dg.source_of_truth_id",
                [],
                |row| row.get(0),
            )?;
        Ok(count as usize)
    }

    fn get_group_members(&self, group_id: i64) -> Result<Vec<PhotoFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.source_id, p.path, p.size, p.format, p.sha256, p.phash, p.dhash, p.mtime,
             p.exif_date, p.exif_camera_make, p.exif_camera_model, p.exif_gps_lat, p.exif_gps_lon,
             p.exif_width, p.exif_height
             FROM photos p
             JOIN group_members gm ON gm.photo_id = p.id
             WHERE gm.group_id = ?1",
        )?;
        let photos = stmt
            .query_map(params![group_id], |row| {
                let exif_date: Option<String> = row.get(9)?;
                let exif_make: Option<String> = row.get(10)?;
                let exif_model: Option<String> = row.get(11)?;
                let exif_lat: Option<f64> = row.get(12)?;
                let exif_lon: Option<f64> = row.get(13)?;
                let exif_w: Option<u32> = row.get(14)?;
                let exif_h: Option<u32> = row.get(15)?;

                let exif = if exif_date.is_some()
                    || exif_make.is_some()
                    || exif_model.is_some()
                    || exif_lat.is_some()
                {
                    Some(ExifData {
                        date: exif_date,
                        camera_make: exif_make,
                        camera_model: exif_model,
                        gps_lat: exif_lat,
                        gps_lon: exif_lon,
                        width: exif_w,
                        height: exif_h,
                    })
                } else {
                    None
                };

                Ok(PhotoFile {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    path: PathBuf::from(row.get::<_, String>(2)?),
                    size: row.get::<_, i64>(3)? as u64,
                    format: parse_format(&row.get::<_, String>(4)?),
                    sha256: row.get(5)?,
                    phash: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                    dhash: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                    exif,
                    mtime: row.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(photos)
    }

    // ── Config ───────────────────────────────────────────────────

    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO config (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_config(&self, key: &str) -> Result<Option<String>> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM config WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .ok();
        Ok(value)
    }
}

fn parse_format(s: &str) -> PhotoFormat {
    match s {
        "CR2" => PhotoFormat::Cr2,
        "CR3" => PhotoFormat::Cr3,
        "NEF" => PhotoFormat::Nef,
        "ARW" => PhotoFormat::Arw,
        "ORF" => PhotoFormat::Orf,
        "RAF" => PhotoFormat::Raf,
        "RW2" => PhotoFormat::Rw2,
        "DNG" => PhotoFormat::Dng,
        "TIFF" => PhotoFormat::Tiff,
        "PNG" => PhotoFormat::Png,
        "HEIC" => PhotoFormat::Heic,
        "WebP" => PhotoFormat::Webp,
        _ => PhotoFormat::Jpeg,
    }
}

fn parse_confidence(s: &str) -> Confidence {
    match s {
        "Certain" => Confidence::Certain,
        "Near-Certain" => Confidence::NearCertain,
        "High" => Confidence::High,
        "Probable" => Confidence::Probable,
        _ => Confidence::Low,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_catalog_with_source() -> (Catalog, Source, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("photos");
        std::fs::create_dir_all(&source_dir).unwrap();
        let catalog = Catalog::open_in_memory().unwrap();
        let source = catalog.add_source(&source_dir).unwrap();
        (catalog, source, tmp)
    }

    fn make_photo(source_id: i64, path: &str, sha: &str) -> PhotoFile {
        PhotoFile {
            id: 0,
            source_id,
            path: PathBuf::from(path),
            size: 1024,
            format: PhotoFormat::Jpeg,
            sha256: sha.to_string(),
            phash: Some(12345),
            dhash: Some(67890),
            exif: None,
            mtime: 1000,
        }
    }

    // ── Source tests ─────────────────────────────────────────────

    #[test]
    fn test_catalog_open_and_add_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("photos");
        std::fs::create_dir_all(&source_dir).unwrap();

        let catalog = Catalog::open(&tmp.path().join("test.db")).unwrap();
        let source = catalog.add_source(&source_dir).unwrap();
        assert_eq!(source.path, source_dir.canonicalize().unwrap());
        assert!(source.last_scanned.is_none());

        let sources = catalog.list_sources().unwrap();
        assert_eq!(sources.len(), 1);
    }

    #[test]
    fn test_duplicate_source_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("photos");
        std::fs::create_dir_all(&source_dir).unwrap();

        let catalog = Catalog::open(&tmp.path().join("test.db")).unwrap();
        catalog.add_source(&source_dir).unwrap();
        let err = catalog.add_source(&source_dir).unwrap_err();
        assert!(matches!(err, Error::SourceAlreadyExists(_)));
    }

    #[test]
    fn test_update_source_scanned() {
        let (catalog, source, _tmp) = make_catalog_with_source();

        catalog.update_source_scanned(source.id, 1700000000).unwrap();
        let sources = catalog.list_sources().unwrap();
        assert_eq!(sources[0].last_scanned, Some(1700000000));
    }

    #[test]
    fn test_multiple_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_a = tmp.path().join("a");
        let dir_b = tmp.path().join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        let catalog = Catalog::open_in_memory().unwrap();
        catalog.add_source(&dir_a).unwrap();
        catalog.add_source(&dir_b).unwrap();

        let sources = catalog.list_sources().unwrap();
        assert_eq!(sources.len(), 2);
    }

    // ── Photo tests ──────────────────────────────────────────────

    #[test]
    fn test_upsert_photo_insert() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let photo = make_photo(source.id, "/tmp/test.jpg", "abc123");

        let id = catalog.upsert_photo(&photo).unwrap();
        assert!(id > 0);

        let photos = catalog.list_all_photos().unwrap();
        assert_eq!(photos.len(), 1);
        assert_eq!(photos[0].sha256, "abc123");
        assert_eq!(photos[0].size, 1024);
        assert_eq!(photos[0].format, PhotoFormat::Jpeg);
    }

    #[test]
    fn test_upsert_photo_update() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let mut photo = make_photo(source.id, "/tmp/test.jpg", "abc123");

        let id1 = catalog.upsert_photo(&photo).unwrap();
        photo.sha256 = "updated_hash".to_string();
        photo.size = 2048;
        let id2 = catalog.upsert_photo(&photo).unwrap();

        assert_eq!(id1, id2);
        let photos = catalog.list_all_photos().unwrap();
        assert_eq!(photos.len(), 1);
        assert_eq!(photos[0].sha256, "updated_hash");
        assert_eq!(photos[0].size, 2048);
    }

    #[test]
    fn test_upsert_photo_with_exif() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let mut photo = make_photo(source.id, "/tmp/exif.jpg", "exif_hash");
        photo.exif = Some(ExifData {
            date: Some("2024-01-15".to_string()),
            camera_make: Some("Canon".to_string()),
            camera_model: Some("EOS R5".to_string()),
            gps_lat: Some(48.8566),
            gps_lon: Some(2.3522),
            width: Some(8192),
            height: Some(5464),
        });

        catalog.upsert_photo(&photo).unwrap();
        let photos = catalog.list_all_photos().unwrap();
        let exif = photos[0].exif.as_ref().unwrap();
        assert_eq!(exif.date.as_deref(), Some("2024-01-15"));
        assert_eq!(exif.camera_make.as_deref(), Some("Canon"));
        assert_eq!(exif.camera_model.as_deref(), Some("EOS R5"));
        assert!((exif.gps_lat.unwrap() - 48.8566).abs() < 0.0001);
        assert_eq!(exif.width, Some(8192));
    }

    #[test]
    fn test_get_photo_mtime() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let photo = make_photo(source.id, "/tmp/mtime.jpg", "mtime_hash");

        assert_eq!(catalog.get_photo_mtime(Path::new("/tmp/mtime.jpg")).unwrap(), None);

        catalog.upsert_photo(&photo).unwrap();
        assert_eq!(catalog.get_photo_mtime(Path::new("/tmp/mtime.jpg")).unwrap(), Some(1000));
    }

    #[test]
    fn test_count_photos() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        assert_eq!(catalog.count_photos().unwrap(), 0);

        catalog.upsert_photo(&make_photo(source.id, "/tmp/a.jpg", "aaa")).unwrap();
        catalog.upsert_photo(&make_photo(source.id, "/tmp/b.jpg", "bbb")).unwrap();
        assert_eq!(catalog.count_photos().unwrap(), 2);
    }

    // ── Group tests ──────────────────────────────────────────────

    #[test]
    fn test_insert_and_get_group() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let id_a = catalog.upsert_photo(&make_photo(source.id, "/tmp/a.jpg", "aaa")).unwrap();
        let id_b = catalog.upsert_photo(&make_photo(source.id, "/tmp/b.jpg", "aaa")).unwrap();

        let group_id = catalog.insert_group(id_a, Confidence::Certain, &[id_a, id_b]).unwrap();
        assert!(group_id > 0);

        let group = catalog.get_group(group_id).unwrap();
        assert_eq!(group.id, group_id);
        assert_eq!(group.source_of_truth_id, id_a);
        assert_eq!(group.confidence, Confidence::Certain);
        assert_eq!(group.members.len(), 2);
    }

    #[test]
    fn test_list_groups() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let id_a = catalog.upsert_photo(&make_photo(source.id, "/tmp/a.jpg", "aaa")).unwrap();
        let id_b = catalog.upsert_photo(&make_photo(source.id, "/tmp/b.jpg", "aaa")).unwrap();
        let id_c = catalog.upsert_photo(&make_photo(source.id, "/tmp/c.jpg", "ccc")).unwrap();
        let id_d = catalog.upsert_photo(&make_photo(source.id, "/tmp/d.jpg", "ccc")).unwrap();

        catalog.insert_group(id_a, Confidence::Certain, &[id_a, id_b]).unwrap();
        catalog.insert_group(id_c, Confidence::High, &[id_c, id_d]).unwrap();

        let groups = catalog.list_groups().unwrap();
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn test_get_group_not_found() {
        let catalog = Catalog::open_in_memory().unwrap();
        let err = catalog.get_group(999).unwrap_err();
        assert!(matches!(err, Error::GroupNotFound(999)));
    }

    #[test]
    fn test_count_groups() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        assert_eq!(catalog.count_groups().unwrap(), 0);

        let id_a = catalog.upsert_photo(&make_photo(source.id, "/tmp/a.jpg", "aaa")).unwrap();
        let id_b = catalog.upsert_photo(&make_photo(source.id, "/tmp/b.jpg", "aaa")).unwrap();
        catalog.insert_group(id_a, Confidence::Certain, &[id_a, id_b]).unwrap();

        assert_eq!(catalog.count_groups().unwrap(), 1);
    }

    #[test]
    fn test_count_duplicate_photos() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let id_a = catalog.upsert_photo(&make_photo(source.id, "/tmp/a.jpg", "aaa")).unwrap();
        let id_b = catalog.upsert_photo(&make_photo(source.id, "/tmp/b.jpg", "aaa")).unwrap();
        let id_c = catalog.upsert_photo(&make_photo(source.id, "/tmp/c.jpg", "aaa")).unwrap();

        // a is source of truth, b and c are duplicates
        catalog.insert_group(id_a, Confidence::Certain, &[id_a, id_b, id_c]).unwrap();

        assert_eq!(catalog.count_duplicate_photos().unwrap(), 2);
    }

    #[test]
    fn test_clear_groups() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let id_a = catalog.upsert_photo(&make_photo(source.id, "/tmp/a.jpg", "aaa")).unwrap();
        let id_b = catalog.upsert_photo(&make_photo(source.id, "/tmp/b.jpg", "aaa")).unwrap();
        catalog.insert_group(id_a, Confidence::Certain, &[id_a, id_b]).unwrap();

        assert_eq!(catalog.count_groups().unwrap(), 1);
        catalog.clear_groups().unwrap();
        assert_eq!(catalog.count_groups().unwrap(), 0);
    }

    // ── Round-trip format/confidence parsing ─────────────────────

    #[test]
    fn test_format_roundtrip() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let formats = [
            PhotoFormat::Jpeg, PhotoFormat::Png, PhotoFormat::Tiff, PhotoFormat::Webp,
            PhotoFormat::Heic, PhotoFormat::Cr2, PhotoFormat::Cr3, PhotoFormat::Nef,
            PhotoFormat::Arw, PhotoFormat::Orf, PhotoFormat::Raf, PhotoFormat::Rw2,
            PhotoFormat::Dng,
        ];

        for (i, fmt) in formats.iter().enumerate() {
            let mut photo = make_photo(source.id, &format!("/tmp/{i}.raw"), &format!("hash{i}"));
            photo.format = *fmt;
            catalog.upsert_photo(&photo).unwrap();
        }

        let photos = catalog.list_all_photos().unwrap();
        assert_eq!(photos.len(), formats.len());
        for (photo, expected) in photos.iter().zip(formats.iter()) {
            assert_eq!(photo.format, *expected);
        }
    }

    #[test]
    fn test_confidence_roundtrip() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let id_a = catalog.upsert_photo(&make_photo(source.id, "/tmp/a.jpg", "aaa")).unwrap();
        let id_b = catalog.upsert_photo(&make_photo(source.id, "/tmp/b.jpg", "bbb")).unwrap();

        let confidences = [
            Confidence::Certain, Confidence::NearCertain, Confidence::High,
            Confidence::Probable, Confidence::Low,
        ];

        for conf in &confidences {
            catalog.clear_groups().unwrap();
            catalog.insert_group(id_a, *conf, &[id_a, id_b]).unwrap();
            let group = catalog.list_groups().unwrap().pop().unwrap();
            assert_eq!(group.confidence, *conf, "roundtrip failed for {conf}");
        }
    }

    // ── Config ──────────────────────────────────────────────────

    #[test]
    fn test_set_and_get_config() {
        let catalog = Catalog::open_in_memory().unwrap();
        assert_eq!(catalog.get_config("vault_path").unwrap(), None);

        catalog.set_config("vault_path", "/tmp/vault").unwrap();
        assert_eq!(
            catalog.get_config("vault_path").unwrap(),
            Some("/tmp/vault".to_string())
        );
    }

    #[test]
    fn test_set_config_overwrite() {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.set_config("vault_path", "/old").unwrap();
        catalog.set_config("vault_path", "/new").unwrap();
        assert_eq!(
            catalog.get_config("vault_path").unwrap(),
            Some("/new".to_string())
        );
    }
}
