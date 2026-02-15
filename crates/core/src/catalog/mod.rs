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
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Open an in-memory catalog (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::initialize(&conn)?;
        schema::migrate(&conn)?;
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

    /// Remove a source and all its photos from the catalog.
    /// Also cleans up group_members and empty duplicate_groups.
    pub fn remove_source(&self, path: &Path) -> Result<(Source, usize)> {
        // Try canonicalize, fall back to raw path (source dir may have been deleted)
        let lookup_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let path_str = lookup_path.to_string_lossy();

        // Look up the source
        let source: Source = self
            .conn
            .query_row(
                "SELECT id, path, last_scanned FROM sources WHERE path = ?1",
                params![path_str.as_ref()],
                |row| {
                    Ok(Source {
                        id: row.get(0)?,
                        path: PathBuf::from(row.get::<_, String>(1)?),
                        last_scanned: row.get(2)?,
                    })
                },
            )
            .map_err(|_| Error::SourceNotRegistered(lookup_path))?;

        // Count photos before deletion
        let photo_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM photos WHERE source_id = ?1",
            params![source.id],
            |row| row.get(0),
        )?;

        // Delete group_members for photos in this source
        self.conn.execute(
            "DELETE FROM group_members WHERE photo_id IN (SELECT id FROM photos WHERE source_id = ?1)",
            params![source.id],
        )?;

        // Delete groups whose source_of_truth is a photo in this source, or that are now empty.
        // Must happen BEFORE deleting photos (groups.source_of_truth_id → photos.id FK).
        self.conn.execute(
            "DELETE FROM group_members WHERE group_id IN (
                SELECT id FROM duplicate_groups
                WHERE source_of_truth_id IN (SELECT id FROM photos WHERE source_id = ?1)
                   OR id NOT IN (SELECT DISTINCT group_id FROM group_members)
            )",
            params![source.id],
        )?;
        self.conn.execute(
            "DELETE FROM duplicate_groups
             WHERE source_of_truth_id IN (SELECT id FROM photos WHERE source_id = ?1)
                OR id NOT IN (SELECT DISTINCT group_id FROM group_members)",
            params![source.id],
        )?;

        // Delete photos
        self.conn.execute(
            "DELETE FROM photos WHERE source_id = ?1",
            params![source.id],
        )?;

        // Delete the source
        self.conn.execute(
            "DELETE FROM sources WHERE id = ?1",
            params![source.id],
        )?;

        Ok((source, photo_count as usize))
    }

    /// Remove specific photos by path. Cleans up group_members to avoid FK violations.
    /// Returns the number of photos removed.
    pub fn remove_photos_by_paths(&self, paths: &[&Path]) -> Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }

        let path_strs: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let mut total_removed = 0usize;

        // Process in chunks to respect SQLite variable limits
        for chunk in path_strs.chunks(500) {
            let placeholders: String = chunk.iter().enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");

            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();

            // Delete group_members for these photos
            self.conn.execute(
                &format!(
                    "DELETE FROM group_members WHERE photo_id IN (SELECT id FROM photos WHERE path IN ({placeholders}))"
                ),
                params.as_slice(),
            )?;

            // Delete groups whose source_of_truth is one of these photos, or now empty
            self.conn.execute(
                &format!(
                    "DELETE FROM group_members WHERE group_id IN (
                        SELECT id FROM duplicate_groups
                        WHERE source_of_truth_id IN (SELECT id FROM photos WHERE path IN ({placeholders}))
                           OR id NOT IN (SELECT DISTINCT group_id FROM group_members)
                    )"
                ),
                params.as_slice(),
            )?;
            self.conn.execute(
                &format!(
                    "DELETE FROM duplicate_groups
                     WHERE source_of_truth_id IN (SELECT id FROM photos WHERE path IN ({placeholders}))
                        OR id NOT IN (SELECT DISTINCT group_id FROM group_members)"
                ),
                params.as_slice(),
            )?;

            // Delete the photos
            let removed = self.conn.execute(
                &format!("DELETE FROM photos WHERE path IN ({placeholders})"),
                params.as_slice(),
            )?;
            total_removed += removed;
        }

        Ok(total_removed)
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

    /// Load all (path → mtime) pairs for a given source in a single query.
    pub fn get_mtimes_for_source(&self, source_id: i64) -> Result<HashMap<PathBuf, i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, mtime FROM photos WHERE source_id = ?1")?;
        let rows = stmt
            .query_map(params![source_id], |row| {
                Ok((PathBuf::from(row.get::<_, String>(0)?), row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().collect())
    }

    /// Look up existing perceptual hashes by SHA-256 values.
    /// Returns a map of sha256 → (phash, Option<dhash>) for entries that have phash.
    pub fn get_phashes_by_sha256s(&self, sha256s: &[&str]) -> Result<HashMap<String, (u64, Option<u64>)>> {
        if sha256s.is_empty() {
            return Ok(HashMap::new());
        }
        let mut result = HashMap::new();
        // Query in batches to avoid SQLite variable limits
        for chunk in sha256s.chunks(500) {
            let placeholders: Vec<String> = (0..chunk.len()).map(|i| format!("?{}", i + 1)).collect();
            let sql = format!(
                "SELECT sha256, phash, dhash FROM photos WHERE sha256 IN ({}) AND phash IS NOT NULL GROUP BY sha256",
                placeholders.join(", ")
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            let rows = stmt
                .query_map(params.as_slice(), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)? as u64,
                        row.get::<_, Option<i64>>(2)?.map(|v| v as u64),
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for (sha, phash, dhash) in rows {
                result.insert(sha, (phash, dhash));
            }
        }
        Ok(result)
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

    /// Clear all cached perceptual hashes. Used when the hash algorithm changes.
    pub fn clear_perceptual_hashes(&self) -> Result<usize> {
        let count = self.conn.execute(
            "UPDATE photos SET phash = NULL, dhash = NULL WHERE phash IS NOT NULL",
            [],
        )?;
        Ok(count)
    }

    /// Reset all mtime values to 0, forcing every file to be re-processed on next scan.
    pub fn reset_all_mtimes(&self) -> Result<usize> {
        let count = self.conn.execute("UPDATE photos SET mtime = 0", [])?;
        Ok(count)
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

    #[test]
    fn test_remove_source() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let photo = make_photo(source.id, "/tmp/test.jpg", "abc123");
        catalog.upsert_photo(&photo).unwrap();

        assert_eq!(catalog.list_sources().unwrap().len(), 1);
        assert_eq!(catalog.count_photos().unwrap(), 1);

        let (removed, count) = catalog.remove_source(&source.path).unwrap();
        assert_eq!(removed.id, source.id);
        assert_eq!(count, 1);
        assert_eq!(catalog.list_sources().unwrap().len(), 0);
        assert_eq!(catalog.count_photos().unwrap(), 0);
    }

    #[test]
    fn test_remove_source_not_registered() {
        let catalog = Catalog::open_in_memory().unwrap();
        let err = catalog.remove_source(Path::new("/nonexistent")).unwrap_err();
        assert!(matches!(err, Error::SourceNotRegistered(_)));
    }

    #[test]
    fn test_remove_source_cleans_empty_groups() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let id_a = catalog.upsert_photo(&make_photo(source.id, "/tmp/a.jpg", "aaa")).unwrap();
        let id_b = catalog.upsert_photo(&make_photo(source.id, "/tmp/b.jpg", "aaa")).unwrap();
        catalog.insert_group(id_a, Confidence::Certain, &[id_a, id_b]).unwrap();

        assert_eq!(catalog.count_groups().unwrap(), 1);

        catalog.remove_source(&source.path).unwrap();

        assert_eq!(catalog.count_groups().unwrap(), 0);
        assert_eq!(catalog.count_photos().unwrap(), 0);
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

    // ── Clear perceptual hashes ─────────────────────────────────

    #[test]
    fn test_clear_perceptual_hashes_nullifies_values() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let mut photo = make_photo(source.id, "/tmp/a.jpg", "aaa");
        photo.phash = Some(12345);
        photo.dhash = Some(67890);
        catalog.upsert_photo(&photo).unwrap();

        let before = catalog.list_all_photos().unwrap();
        assert!(before[0].phash.is_some());
        assert!(before[0].dhash.is_some());

        let count = catalog.clear_perceptual_hashes().unwrap();
        assert_eq!(count, 1);

        let after = catalog.list_all_photos().unwrap();
        assert!(after[0].phash.is_none());
        assert!(after[0].dhash.is_none());
    }

    #[test]
    fn test_clear_perceptual_hashes_returns_zero_when_none_set() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let mut photo = make_photo(source.id, "/tmp/a.jpg", "aaa");
        photo.phash = None;
        photo.dhash = None;
        catalog.upsert_photo(&photo).unwrap();

        let count = catalog.clear_perceptual_hashes().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_clear_perceptual_hashes_only_affects_non_null() {
        let (catalog, source, _tmp) = make_catalog_with_source();

        let mut with_hash = make_photo(source.id, "/tmp/a.jpg", "aaa");
        with_hash.phash = Some(111);
        with_hash.dhash = Some(222);
        catalog.upsert_photo(&with_hash).unwrap();

        let mut without_hash = make_photo(source.id, "/tmp/b.jpg", "bbb");
        without_hash.phash = None;
        without_hash.dhash = None;
        catalog.upsert_photo(&without_hash).unwrap();

        let count = catalog.clear_perceptual_hashes().unwrap();
        assert_eq!(count, 1);

        let photos = catalog.list_all_photos().unwrap();
        assert!(photos.iter().all(|p| p.phash.is_none() && p.dhash.is_none()));
    }

    #[test]
    fn test_clear_perceptual_hashes_preserves_other_fields() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let mut photo = make_photo(source.id, "/tmp/a.jpg", "sha_abc");
        photo.phash = Some(999);
        photo.dhash = Some(888);
        photo.size = 5000;
        catalog.upsert_photo(&photo).unwrap();

        catalog.clear_perceptual_hashes().unwrap();

        let photos = catalog.list_all_photos().unwrap();
        assert_eq!(photos[0].sha256, "sha_abc");
        assert_eq!(photos[0].size, 5000);
        assert_eq!(photos[0].format, PhotoFormat::Jpeg);
        assert!(photos[0].phash.is_none());
    }

    // ── Schema version tracking ─────────────────────────────────

    #[test]
    fn test_schema_version_set_on_fresh_db() {
        let catalog = Catalog::open_in_memory().unwrap();
        let version = catalog.get_config("schema_version").unwrap();
        assert_eq!(version, Some("1".to_string()));
    }

    #[test]
    fn test_schema_version_persists_across_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("catalog.db");

        {
            let catalog = Catalog::open(&db_path).unwrap();
            assert_eq!(catalog.get_config("schema_version").unwrap(), Some("1".to_string()));
        }
        {
            let catalog = Catalog::open(&db_path).unwrap();
            assert_eq!(catalog.get_config("schema_version").unwrap(), Some("1".to_string()));
        }
    }

    #[test]
    fn test_pre_versioning_db_upgraded_to_v1() {
        // Create a DB with schema but no schema_version key.
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        schema::initialize(&conn).unwrap();

        // Verify no schema_version key exists yet.
        let v: Option<String> = conn
            .query_row("SELECT value FROM config WHERE key = 'schema_version'", [], |r| r.get(0))
            .ok();
        assert!(v.is_none());

        // Running migrate should set it to 1.
        schema::migrate(&conn).unwrap();
        let v: String = conn
            .query_row("SELECT value FROM config WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "1");
    }

    #[test]
    fn test_reject_future_schema_version() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        schema::initialize(&conn).unwrap();

        // Force a future version.
        conn.execute(
            "INSERT INTO config (key, value) VALUES ('schema_version', '999')",
            [],
        )
        .unwrap();

        let err = schema::migrate(&conn).unwrap_err();
        assert!(matches!(err, Error::SchemaTooNew { db: 999, code: 1 }));
    }

    #[test]
    fn test_migration_check_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        schema::initialize(&conn).unwrap();
        schema::migrate(&conn).unwrap();
        schema::migrate(&conn).unwrap(); // second call is a no-op
        let v: String = conn
            .query_row("SELECT value FROM config WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "1");
    }

    // ── Schema structure pinning ────────────────────────────────

    #[test]
    fn test_catalog_tables_exist() {
        let catalog = Catalog::open_in_memory().unwrap();
        let mut stmt = catalog
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(tables, vec!["config", "duplicate_groups", "group_members", "photos", "sources"]);
    }

    #[test]
    fn test_catalog_indexes_exist() {
        let catalog = Catalog::open_in_memory().unwrap();
        let mut stmt = catalog
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%' ORDER BY name")
            .unwrap();
        let indexes: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            indexes,
            vec![
                "idx_group_members_photo",
                "idx_photos_path",
                "idx_photos_sha256",
                "idx_photos_source",
                "idx_photos_source_mtime",
            ]
        );
    }

    #[test]
    fn test_photos_columns() {
        let catalog = Catalog::open_in_memory().unwrap();
        let mut stmt = catalog
            .conn
            .prepare("SELECT name FROM pragma_table_info('photos') ORDER BY cid")
            .unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            columns,
            vec![
                "id", "source_id", "path", "size", "format", "sha256",
                "phash", "dhash", "mtime", "exif_date", "exif_camera_make",
                "exif_camera_model", "exif_gps_lat", "exif_gps_lon",
                "exif_width", "exif_height",
            ]
        );
    }

    #[test]
    fn test_schema_snapshot() {
        let catalog = Catalog::open_in_memory().unwrap();
        let mut stmt = catalog
            .conn
            .prepare(
                "SELECT sql FROM sqlite_master
                 WHERE type IN ('table', 'index') AND name NOT LIKE 'sqlite_%'
                 ORDER BY type DESC, name",
            )
            .unwrap();
        let stmts: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        // Normalize whitespace for comparison stability.
        let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let normalized: Vec<String> = stmts.iter().map(|s| normalize(s)).collect();

        // Tables (sorted alphabetically)
        assert!(normalized.iter().any(|s| s.contains("CREATE TABLE config")));
        assert!(normalized.iter().any(|s| s.contains("CREATE TABLE duplicate_groups")));
        assert!(normalized.iter().any(|s| s.contains("CREATE TABLE group_members")));
        assert!(normalized.iter().any(|s| s.contains("CREATE TABLE photos")));
        assert!(normalized.iter().any(|s| s.contains("CREATE TABLE sources")));

        // Indexes
        assert!(normalized.iter().any(|s| s.contains("idx_photos_sha256")));
        assert!(normalized.iter().any(|s| s.contains("idx_photos_source")));
        assert!(normalized.iter().any(|s| s.contains("idx_photos_path")));
        assert!(normalized.iter().any(|s| s.contains("idx_photos_source_mtime")));
        assert!(normalized.iter().any(|s| s.contains("idx_group_members_photo")));
    }

    // ── Data integrity ──────────────────────────────────────────

    #[test]
    fn test_data_survives_close_and_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("catalog.db");
        let source_dir = tmp.path().join("photos");
        std::fs::create_dir_all(&source_dir).unwrap();

        let source_id;
        {
            let catalog = Catalog::open(&db_path).unwrap();
            let source = catalog.add_source(&source_dir).unwrap();
            source_id = source.id;
            let photo = make_photo(source_id, "/tmp/survive.jpg", "survive_hash");
            catalog.upsert_photo(&photo).unwrap();

            let id_a = catalog.upsert_photo(&make_photo(source_id, "/tmp/ga.jpg", "ga")).unwrap();
            let id_b = catalog.upsert_photo(&make_photo(source_id, "/tmp/gb.jpg", "gb")).unwrap();
            catalog.insert_group(id_a, Confidence::High, &[id_a, id_b]).unwrap();

            catalog.set_config("test_key", "test_value").unwrap();
        }
        {
            let catalog = Catalog::open(&db_path).unwrap();
            let sources = catalog.list_sources().unwrap();
            assert_eq!(sources.len(), 1);
            assert_eq!(sources[0].id, source_id);

            let photos = catalog.list_all_photos().unwrap();
            assert_eq!(photos.len(), 3);

            let groups = catalog.list_groups().unwrap();
            assert_eq!(groups.len(), 1);

            assert_eq!(catalog.get_config("test_key").unwrap(), Some("test_value".to_string()));
        }
    }

    #[test]
    fn test_foreign_key_photos_requires_valid_source() {
        let catalog = Catalog::open_in_memory().unwrap();
        let photo = make_photo(9999, "/tmp/orphan.jpg", "orphan_hash");
        let result = catalog.upsert_photo(&photo);
        assert!(result.is_err());
    }

    #[test]
    fn test_foreign_key_group_members_requires_valid_photo() {
        let (catalog, source, _tmp) = make_catalog_with_source();
        let id_a = catalog.upsert_photo(&make_photo(source.id, "/tmp/a.jpg", "aaa")).unwrap();
        let result = catalog.insert_group(id_a, Confidence::Certain, &[id_a, 9999]);
        assert!(result.is_err());
    }

    #[test]
    fn test_foreign_key_group_requires_valid_sot() {
        let catalog = Catalog::open_in_memory().unwrap();
        let result = catalog.insert_group(9999, Confidence::Certain, &[]);
        assert!(result.is_err());
    }
}
