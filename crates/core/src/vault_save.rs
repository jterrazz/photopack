use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::{DuplicateGroup, PhotoFile};
use crate::error::Result;

/// Progress callback events for the vault save operation.
pub enum VaultSaveProgress {
    /// Starting save with total count.
    Start { total: usize },
    /// A file was copied.
    Copied { source: PathBuf, target: PathBuf },
    /// A file was skipped (already exists with same size).
    Skipped { path: PathBuf },
    /// A superseded file was removed from the vault (replaced by higher-quality version).
    Removed { path: PathBuf },
    /// Save completed.
    Complete {
        copied: usize,
        skipped: usize,
        removed: usize,
    },
}

/// Parse an EXIF date string into (year, month, day).
/// Handles both "2024-01-15 12:00:00" (display_value) and "2024:01:15 12:00:00" (raw EXIF).
pub fn parse_exif_date(date_str: &str) -> Option<(u32, u32, u32)> {
    let date_part = date_str.split_whitespace().next()?;
    let parts: Vec<&str> = date_part.split([':', '-']).collect();
    if parts.len() < 3 {
        return None;
    }
    let year: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;

    if !(1970..=2100).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    Some((year, month, day))
}

/// Extract (year, month, day) from a photo's EXIF date, falling back to mtime.
pub fn date_for_photo(photo: &PhotoFile) -> (u32, u32, u32) {
    if let Some(ref exif) = photo.exif {
        if let Some(ref date_str) = exif.date {
            if let Some(date) = parse_exif_date(date_str) {
                return date;
            }
        }
    }

    // Fallback to mtime
    let dt = chrono::DateTime::from_timestamp(photo.mtime, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
    use chrono::Datelike;
    (dt.year() as u32, dt.month(), dt.day())
}

/// Build the target path: vault_path/YYYY/MM/DD/filename.ext
/// Handles filename collisions by appending _1, _2, etc.
/// If a file already exists with a matching size, returns that path (enables incremental skip).
pub fn build_target_path(
    vault_path: &Path,
    date: (u32, u32, u32),
    original_path: &Path,
    expected_size: u64,
) -> PathBuf {
    let (year, month, day) = date;
    let dir = vault_path
        .join(format!("{:04}", year))
        .join(format!("{:02}", month))
        .join(format!("{:02}", day));

    let file_stem = original_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let ext = original_path
        .extension()
        .unwrap_or_default()
        .to_string_lossy();

    let base_name = if ext.is_empty() {
        file_stem.to_string()
    } else {
        format!("{}.{}", file_stem, ext)
    };

    let mut target = dir.join(&base_name);
    let mut counter = 1u32;
    while target.exists() {
        // If existing file matches expected size, this is our file (incremental skip)
        if let Ok(meta) = target.metadata() {
            if meta.len() == expected_size {
                return target;
            }
        }
        target = if ext.is_empty() {
            dir.join(format!("{}_{}", file_stem, counter))
        } else {
            dir.join(format!("{}_{}.{}", file_stem, counter, ext))
        };
        counter += 1;
    }

    target
}

/// Determine which photos to save to the vault:
/// - For each duplicate group, take only the source-of-truth.
/// - For ungrouped photos, take the photo itself.
pub fn select_photos_to_export<'a>(
    all_photos: &'a [PhotoFile],
    groups: &[DuplicateGroup],
) -> Vec<&'a PhotoFile> {
    let mut grouped_ids: HashSet<i64> = HashSet::new();
    let mut sot_ids: HashSet<i64> = HashSet::new();

    for group in groups {
        for member in &group.members {
            grouped_ids.insert(member.id);
        }
        sot_ids.insert(group.source_of_truth_id);
    }

    all_photos
        .iter()
        .filter(|p| {
            if grouped_ids.contains(&p.id) {
                sot_ids.contains(&p.id)
            } else {
                true
            }
        })
        .collect()
}

/// Remove superseded vault files: group members that live inside the vault directory
/// and are NOT the source-of-truth. These are lower-quality versions that have been
/// replaced by a higher-quality source-of-truth.
/// Returns the list of removed file paths.
pub fn cleanup_superseded_vault_files(
    vault_path: &Path,
    all_photos: &[PhotoFile],
    groups: &[DuplicateGroup],
) -> Vec<PathBuf> {
    let vault_canonical = vault_path
        .canonicalize()
        .unwrap_or_else(|_| vault_path.to_path_buf());

    let photo_map: std::collections::HashMap<i64, &PhotoFile> =
        all_photos.iter().map(|p| (p.id, p)).collect();

    let mut removed = Vec::new();
    for group in groups {
        for member in &group.members {
            if member.id == group.source_of_truth_id {
                continue;
            }
            let member_canonical = member
                .path
                .canonicalize()
                .unwrap_or_else(|_| member.path.clone());
            if member_canonical.starts_with(&vault_canonical) {
                // Verify the SOT is NOT also in the vault (avoid removing if both are in vault)
                if let Some(sot) = photo_map.get(&group.source_of_truth_id) {
                    let sot_canonical = sot
                        .path
                        .canonicalize()
                        .unwrap_or_else(|_| sot.path.clone());
                    // Only remove if SOT exists outside the vault, or SOT is a different
                    // (higher-quality) file also being synced to the vault
                    if sot_canonical == member_canonical {
                        continue; // SOT and member are the same file
                    }
                }
                if fs::remove_file(&member.path).is_ok() {
                    removed.push(member.path.clone());
                }
            }
        }
    }

    removed
}

/// Copy a single file to the target path, creating parent directories as needed.
/// Returns Ok(false) if skipped (file exists with same size), Ok(true) if copied.
pub fn copy_photo_to_vault(source: &Path, target: &Path, expected_size: u64) -> Result<bool> {
    if target.exists() {
        if let Ok(metadata) = target.metadata() {
            if metadata.len() == expected_size {
                return Ok(false);
            }
        }
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::copy(source, target)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;

    // ── parse_exif_date ─────────────────────────────────────────

    #[test]
    fn test_parse_exif_date_hyphenated() {
        assert_eq!(
            parse_exif_date("2024-06-15 12:30:00"),
            Some((2024, 6, 15))
        );
    }

    #[test]
    fn test_parse_exif_date_colons() {
        assert_eq!(
            parse_exif_date("2024:01:15 12:00:00"),
            Some((2024, 1, 15))
        );
    }

    #[test]
    fn test_parse_exif_date_date_only() {
        assert_eq!(parse_exif_date("2024:01:01"), Some((2024, 1, 1)));
    }

    #[test]
    fn test_parse_exif_date_invalid() {
        assert_eq!(parse_exif_date("not-a-date"), None);
        assert_eq!(parse_exif_date(""), None);
    }

    #[test]
    fn test_parse_exif_date_out_of_range() {
        assert_eq!(parse_exif_date("1969:01:01 00:00:00"), None);
        assert_eq!(parse_exif_date("2024:13:01 00:00:00"), None);
        assert_eq!(parse_exif_date("2024:01:32 00:00:00"), None);
    }

    // ── date_for_photo ──────────────────────────────────────────

    fn make_photo(id: i64, mtime: i64) -> PhotoFile {
        PhotoFile {
            id,
            source_id: 1,
            path: PathBuf::from(format!("/test/{id}.jpg")),
            size: 1000,
            format: PhotoFormat::Jpeg,
            sha256: format!("sha_{id}"),
            phash: None,
            dhash: None,
            exif: None,
            mtime,
        }
    }

    #[test]
    fn test_date_for_photo_uses_exif() {
        let mut photo = make_photo(1, 0);
        photo.exif = Some(ExifData {
            date: Some("2024-06-15 12:30:00".to_string()),
            camera_make: None,
            camera_model: None,
            gps_lat: None,
            gps_lon: None,
            width: None,
            height: None,
        });
        assert_eq!(date_for_photo(&photo), (2024, 6, 15));
    }

    #[test]
    fn test_date_for_photo_falls_back_to_mtime() {
        // 1718444400 = 2024-06-15 11:00:00 UTC
        let photo = make_photo(1, 1718444400);
        let (year, month, day) = date_for_photo(&photo);
        assert_eq!(year, 2024);
        assert_eq!(month, 6);
        assert_eq!(day, 15);
    }

    // ── select_photos_to_export ─────────────────────────────────

    fn make_photo_with_path(id: i64, path: &str) -> PhotoFile {
        let mut p = make_photo(id, 1000);
        p.path = PathBuf::from(path);
        p
    }

    #[test]
    fn test_select_ungrouped_all_included() {
        let photos = vec![
            make_photo_with_path(1, "/a.jpg"),
            make_photo_with_path(2, "/b.jpg"),
        ];
        let selected = select_photos_to_export(&photos, &[]);
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn test_select_group_only_sot() {
        let photos = vec![
            make_photo_with_path(1, "/a.jpg"),
            make_photo_with_path(2, "/b.jpg"),
            make_photo_with_path(3, "/c.jpg"),
        ];
        let groups = vec![DuplicateGroup {
            id: 1,
            members: vec![photos[0].clone(), photos[1].clone()],
            source_of_truth_id: 1,
            confidence: Confidence::Certain,
        }];
        let selected = select_photos_to_export(&photos, &groups);
        assert_eq!(selected.len(), 2);
        let ids: HashSet<i64> = selected.iter().map(|p| p.id).collect();
        assert!(ids.contains(&1), "SoT should be included");
        assert!(ids.contains(&3), "ungrouped should be included");
        assert!(!ids.contains(&2), "non-SoT group member should be excluded");
    }

    // ── build_target_path ───────────────────────────────────────

    #[test]
    fn test_build_target_path_basic() {
        let vault = PathBuf::from("/vault");
        let target =
            build_target_path(&vault, (2024, 6, 15), Path::new("/source/photo.jpg"), 1000);
        assert_eq!(target, PathBuf::from("/vault/2024/06/15/photo.jpg"));
    }

    #[test]
    fn test_build_target_path_zero_padding() {
        let vault = PathBuf::from("/vault");
        let target = build_target_path(&vault, (2024, 1, 5), Path::new("/source/img.png"), 1000);
        assert_eq!(target, PathBuf::from("/vault/2024/01/05/img.png"));
    }

    #[test]
    fn test_build_target_path_collision_different_size() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path();
        let date_dir = vault.join("2024/06/15");
        fs::create_dir_all(&date_dir).unwrap();

        // Create an existing file with 5 bytes
        fs::write(date_dir.join("photo.jpg"), b"hello").unwrap();

        // Build path for a file with a different size (1000) — should get _1 suffix
        let target =
            build_target_path(vault, (2024, 6, 15), Path::new("/source/photo.jpg"), 1000);
        assert_eq!(
            target.file_name().unwrap().to_string_lossy(),
            "photo_1.jpg"
        );
    }

    #[test]
    fn test_build_target_path_collision_same_size_returns_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path();
        let date_dir = vault.join("2024/06/15");
        fs::create_dir_all(&date_dir).unwrap();

        // Create an existing file with 5 bytes
        fs::write(date_dir.join("photo.jpg"), b"hello").unwrap();

        // Build path for a file with matching size (5) — should return existing path
        let target = build_target_path(vault, (2024, 6, 15), Path::new("/source/photo.jpg"), 5);
        assert_eq!(target.file_name().unwrap().to_string_lossy(), "photo.jpg");
    }

    #[test]
    fn test_build_target_path_multiple_collisions() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path();
        let date_dir = vault.join("2024/01/01");
        fs::create_dir_all(&date_dir).unwrap();

        // Create photo.jpg, photo_1.jpg, photo_2.jpg — all different sizes
        fs::write(date_dir.join("photo.jpg"), b"a").unwrap();
        fs::write(date_dir.join("photo_1.jpg"), b"ab").unwrap();
        fs::write(date_dir.join("photo_2.jpg"), b"abc").unwrap();

        let target =
            build_target_path(vault, (2024, 1, 1), Path::new("/source/photo.jpg"), 9999);
        assert_eq!(
            target.file_name().unwrap().to_string_lossy(),
            "photo_3.jpg"
        );
    }

    // ── date_for_photo edge cases ───────────────────────────────

    #[test]
    fn test_date_for_photo_invalid_exif_falls_back_to_mtime() {
        let mut photo = make_photo(1, 1718444400); // 2024-06-15
        photo.exif = Some(ExifData {
            date: Some("garbage".to_string()),
            camera_make: None,
            camera_model: None,
            gps_lat: None,
            gps_lon: None,
            width: None,
            height: None,
        });
        let (year, month, day) = date_for_photo(&photo);
        assert_eq!(year, 2024);
        assert_eq!(month, 6);
        assert_eq!(day, 15);
    }

    #[test]
    fn test_date_for_photo_exif_no_date_falls_back_to_mtime() {
        let mut photo = make_photo(1, 1718444400); // 2024-06-15
        photo.exif = Some(ExifData {
            date: None,
            camera_make: Some("Canon".to_string()),
            camera_model: None,
            gps_lat: None,
            gps_lon: None,
            width: None,
            height: None,
        });
        let (year, month, day) = date_for_photo(&photo);
        assert_eq!(year, 2024);
        assert_eq!(month, 6);
        assert_eq!(day, 15);
    }

    // ── select_photos_to_export edge cases ──────────────────────

    #[test]
    fn test_select_photos_multiple_groups() {
        let photos = vec![
            make_photo_with_path(1, "/a.jpg"),
            make_photo_with_path(2, "/b.jpg"),
            make_photo_with_path(3, "/c.jpg"),
            make_photo_with_path(4, "/d.jpg"),
            make_photo_with_path(5, "/e.jpg"),
        ];
        let groups = vec![
            DuplicateGroup {
                id: 1,
                members: vec![photos[0].clone(), photos[1].clone()],
                source_of_truth_id: 1,
                confidence: Confidence::Certain,
            },
            DuplicateGroup {
                id: 2,
                members: vec![photos[2].clone(), photos[3].clone()],
                source_of_truth_id: 3,
                confidence: Confidence::High,
            },
        ];
        let selected = select_photos_to_export(&photos, &groups);
        // SoT 1 from group 1 + SoT 3 from group 2 + ungrouped 5 = 3
        assert_eq!(selected.len(), 3);
        let ids: HashSet<i64> = selected.iter().map(|p| p.id).collect();
        assert!(ids.contains(&1));
        assert!(ids.contains(&3));
        assert!(ids.contains(&5));
    }

    #[test]
    fn test_select_photos_all_grouped() {
        let photos = vec![
            make_photo_with_path(1, "/a.jpg"),
            make_photo_with_path(2, "/b.jpg"),
        ];
        let groups = vec![DuplicateGroup {
            id: 1,
            members: vec![photos[0].clone(), photos[1].clone()],
            source_of_truth_id: 2,
            confidence: Confidence::Certain,
        }];
        let selected = select_photos_to_export(&photos, &groups);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, 2);
    }

    #[test]
    fn test_select_photos_empty_input() {
        let selected = select_photos_to_export(&[], &[]);
        assert!(selected.is_empty());
    }

    // ── copy_photo_to_vault ─────────────────────────────────────

    #[test]
    fn test_copy_photo_creates_dirs_and_copies() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.jpg");
        fs::write(&source, b"photo data").unwrap();

        let target = tmp.path().join("deep/nested/dir/target.jpg");
        let result = copy_photo_to_vault(&source, &target, 1000).unwrap();
        assert!(result, "should copy when target doesn't exist");
        assert!(target.exists());
        assert_eq!(fs::read(&target).unwrap(), b"photo data");
    }

    #[test]
    fn test_copy_photo_skips_same_size() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.jpg");
        fs::write(&source, b"photo data").unwrap(); // 10 bytes

        let target = tmp.path().join("target.jpg");
        fs::write(&target, b"old  data!").unwrap(); // also 10 bytes

        let result = copy_photo_to_vault(&source, &target, 10).unwrap();
        assert!(!result, "should skip when sizes match");
        // Content should NOT be overwritten
        assert_eq!(fs::read(&target).unwrap(), b"old  data!");
    }

    #[test]
    fn test_copy_photo_overwrites_different_size() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.jpg");
        fs::write(&source, b"new photo data").unwrap(); // 14 bytes

        let target = tmp.path().join("target.jpg");
        fs::write(&target, b"old").unwrap(); // 3 bytes

        let result = copy_photo_to_vault(&source, &target, 14).unwrap();
        assert!(result, "should copy when sizes differ");
        assert_eq!(fs::read(&target).unwrap(), b"new photo data");
    }

    #[test]
    fn test_copy_photo_source_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("nonexistent.jpg");
        let target = tmp.path().join("target.jpg");

        let result = copy_photo_to_vault(&source, &target, 1000);
        assert!(result.is_err());
    }
}
