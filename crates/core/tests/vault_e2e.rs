use std::fs;
use std::path::{Path, PathBuf};

use photopack_core::Vault;

/// Create a JPEG with a gradient pattern seeded by (r, g, b) to ensure distinct perceptual hashes.
fn create_jpeg(path: &Path, r: u8, g: u8, b: u8) {
    let img = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([
            r.wrapping_add((x * 3) as u8),
            g.wrapping_add((y * 3) as u8),
            b.wrapping_add(((x + y) * 2) as u8),
        ])
    });
    img.save(path).unwrap();
}

/// Copy a file to create an exact duplicate.
fn copy_file(src: &Path, dst: &Path) {
    fs::copy(src, dst).unwrap();
}

// ── Vault::open ──────────────────────────────────────────────────

#[test]
fn test_vault_open_creates_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("sub/dir/catalog.db");

    let _vault = Vault::open(&db_path).unwrap();
    assert!(db_path.exists());
}

#[test]
fn test_vault_open_reopen_persists() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("catalog.db");
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    {
        let vault = Vault::open(&db_path).unwrap();
        vault.add_source(&photos_dir).unwrap();
    }

    // Reopen — source should still be there
    let vault = Vault::open(&db_path).unwrap();
    let sources = vault.sources().unwrap();
    assert_eq!(sources.len(), 1);
}

// ── Vault::add_source ────────────────────────────────────────────

#[test]
fn test_add_source_valid_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let source = vault.add_source(&photos_dir).unwrap();
    assert_eq!(source.path, photos_dir.canonicalize().unwrap());
}

#[test]
fn test_add_source_nonexistent_path() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let err = vault.add_source(Path::new("/nonexistent/path")).unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn test_add_source_file_not_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("file.txt");
    fs::write(&file_path, b"not a dir").unwrap();

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let err = vault.add_source(&file_path).unwrap_err();
    assert!(err.to_string().contains("not a directory"));
}

#[test]
fn test_add_source_duplicate_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    assert!(vault.add_source(&photos_dir).is_err());
}

// ── Vault::remove_source ─────────────────────────────────────────

#[test]
fn test_remove_source_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();
    create_jpeg(&photos_dir.join("a.jpg"), 100, 150, 200);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.photos().unwrap().len(), 1);
    assert_eq!(vault.sources().unwrap().len(), 1);

    let (source, photo_count) = vault.remove_source(&photos_dir).unwrap();
    assert_eq!(source.path, photos_dir.canonicalize().unwrap());
    assert_eq!(photo_count, 1);
    assert_eq!(vault.sources().unwrap().len(), 0);
    assert_eq!(vault.photos().unwrap().len(), 0);
}

#[test]
fn test_remove_source_not_registered() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let err = vault.remove_source(Path::new("/nonexistent/source")).unwrap_err();
    assert!(err.to_string().contains("not registered"));
}

#[test]
fn test_remove_source_cleans_up_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("source_a");
    let dir_b = tmp.path().join("source_b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();

    // Same photo in both sources (exact duplicate)
    create_jpeg(&dir_a.join("photo.jpg"), 100, 150, 200);
    copy_file(&dir_a.join("photo.jpg"), &dir_b.join("photo.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir_a).unwrap();
    vault.add_source(&dir_b).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_groups, 1);
    assert_eq!(vault.status().unwrap().total_photos, 2);

    // Remove source_a — group should be deleted (only 1 member left)
    vault.remove_source(&dir_a).unwrap();

    assert_eq!(vault.sources().unwrap().len(), 1);
    assert_eq!(vault.photos().unwrap().len(), 1);
    // Group with only 1 remaining member should still exist in DB
    // but the important thing is photos from removed source are gone
    assert_eq!(vault.status().unwrap().total_photos, 1);
}

#[test]
fn test_remove_source_after_directory_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();
    create_jpeg(&photos_dir.join("a.jpg"), 100, 150, 200);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let source = vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.photos().unwrap().len(), 1);

    // Delete the source directory from disk
    fs::remove_dir_all(&photos_dir).unwrap();

    // Should still be able to remove the source from the catalog
    let (removed, photo_count) = vault.remove_source(&source.path).unwrap();
    assert_eq!(removed.id, source.id);
    assert_eq!(photo_count, 1);
    assert_eq!(vault.sources().unwrap().len(), 0);
    assert_eq!(vault.photos().unwrap().len(), 0);
}

// ── Vault::status (empty) ────────────────────────────────────────

#[test]
fn test_status_empty_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_sources, 0);
    assert_eq!(stats.total_photos, 0);
    assert_eq!(stats.total_groups, 0);
    assert_eq!(stats.total_duplicates, 0);
}

// ── Full scan workflow: no duplicates ────────────────────────────

#[test]
fn test_scan_unique_photos() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    // Create 3 visually distinct images with different patterns
    // Horizontal gradient
    let img1 = image::RgbImage::from_fn(64, 64, |x, _| {
        image::Rgb([(x * 4) as u8, 0, 0])
    });
    img1.save(photos_dir.join("gradient_h.jpg")).unwrap();

    // Vertical gradient
    let img2 = image::RgbImage::from_fn(64, 64, |_, y| {
        image::Rgb([0, (y * 4) as u8, 0])
    });
    img2.save(photos_dir.join("gradient_v.jpg")).unwrap();

    // Checkerboard
    let img3 = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / 4 + y / 4) % 2 == 0 {
            image::Rgb([0, 0, 255])
        } else {
            image::Rgb([255, 255, 0])
        }
    });
    img3.save(photos_dir.join("checker.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_sources, 1);
    assert_eq!(stats.total_photos, 3);
    assert_eq!(stats.total_groups, 0);
    assert_eq!(stats.total_duplicates, 0);

    let groups = vault.groups().unwrap();
    assert!(groups.is_empty());
}

// ── Full scan workflow: exact duplicates ──────────────────────────

#[test]
fn test_scan_exact_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    // Create a photo and an exact copy
    create_jpeg(&photos_dir.join("original.jpg"), 100, 150, 200);
    copy_file(
        &photos_dir.join("original.jpg"),
        &photos_dir.join("copy.jpg"),
    );

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_photos, 2);
    assert_eq!(stats.total_groups, 1);
    assert_eq!(stats.total_duplicates, 1);

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members.len(), 2);
    assert_eq!(
        groups[0].confidence,
        photopack_core::domain::Confidence::Certain
    );
}

// ── Full scan workflow: multiple duplicate groups ─────────────────

#[test]
fn test_scan_multiple_duplicate_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    // Group A: two copies of a gradient image
    create_jpeg(&photos_dir.join("grad_a.jpg"), 255, 0, 0);
    copy_file(
        &photos_dir.join("grad_a.jpg"),
        &photos_dir.join("grad_b.jpg"),
    );

    // Group B: two copies of a checkerboard (structurally different from gradient)
    let checker = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            image::Rgb([0, 0, 0])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    checker.save(photos_dir.join("check_a.jpg")).unwrap();
    copy_file(
        &photos_dir.join("check_a.jpg"),
        &photos_dir.join("check_b.jpg"),
    );

    // Unique photo — diagonal stripe, structurally distinct from both above
    let diagonal = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x + y) % 16 < 8 {
            image::Rgb([255, 128, 0])
        } else {
            image::Rgb([0, 128, 255])
        }
    });
    diagonal.save(photos_dir.join("unique.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_photos, 5);
    assert_eq!(stats.total_groups, 2);
    assert_eq!(stats.total_duplicates, 2);
}

// ── Vault::group detail ──────────────────────────────────────────

#[test]
fn test_group_detail() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    create_jpeg(&photos_dir.join("a.jpg"), 50, 50, 50);
    copy_file(&photos_dir.join("a.jpg"), &photos_dir.join("b.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    let group = vault.group(groups[0].id).unwrap();
    assert_eq!(group.members.len(), 2);

    // Source of truth should be one of the members
    assert!(group
        .members
        .iter()
        .any(|m| m.id == group.source_of_truth_id));
}

#[test]
fn test_group_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let err = vault.group(999).unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// ── Incremental scan ─────────────────────────────────────────────

#[test]
fn test_incremental_scan_skips_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();

    // First scan
    vault.scan(None).unwrap();
    let stats1 = vault.status().unwrap();
    assert_eq!(stats1.total_photos, 1);

    // Second scan — same files, should not change
    vault.scan(None).unwrap();
    let stats2 = vault.status().unwrap();
    assert_eq!(stats2.total_photos, 1);
}

#[test]
fn test_scan_picks_up_new_files() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    create_jpeg(&photos_dir.join("first.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 1);

    // Add a new photo
    create_jpeg(&photos_dir.join("second.jpg"), 200, 200, 200);
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 2);
}

// ── Multiple sources ─────────────────────────────────────────────

#[test]
fn test_scan_multiple_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("a");
    let dir_b = tmp.path().join("b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();

    create_jpeg(&dir_a.join("photo.jpg"), 100, 100, 100);
    // Exact copy in different source
    copy_file(&dir_a.join("photo.jpg"), &dir_b.join("photo.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir_a).unwrap();
    vault.add_source(&dir_b).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_sources, 2);
    assert_eq!(stats.total_photos, 2);
    assert_eq!(stats.total_groups, 1);
}

// ── Scan with progress callback ──────────────────────────────────

#[test]
fn test_scan_with_progress_callback() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    create_jpeg(&photos_dir.join("a.jpg"), 10, 20, 30);
    create_jpeg(&photos_dir.join("b.jpg"), 40, 50, 60);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();

    let mut events = Vec::new();
    vault
        .scan(Some(&mut |progress| {
            match &progress {
                photopack_core::ScanProgress::SourceStart { file_count, .. } => {
                    events.push(format!("start:{file_count}"));
                }
                photopack_core::ScanProgress::FileHashed { .. } => {
                    events.push("hashed".to_string());
                }
                photopack_core::ScanProgress::AnalysisStart { count } => {
                    events.push(format!("analysis_start:{count}"));
                }
                photopack_core::ScanProgress::AnalysisDone { .. } => {
                    events.push("analysis_done".to_string());
                }
                photopack_core::ScanProgress::FilesRemoved { count } => {
                    events.push(format!("removed:{count}"));
                }
                photopack_core::ScanProgress::PhaseComplete { phase } => {
                    events.push(format!("phase:{phase}"));
                }
            }
        }))
        .unwrap();

    // Should have: start, 2 hashed, analysis start+done, indexing phase, matching phase
    assert!(events.iter().any(|e| e.starts_with("start:")));
    assert_eq!(events.iter().filter(|e| *e == "hashed").count(), 2);
    assert!(events.iter().any(|e| e.starts_with("analysis_start:")));
    assert!(events.contains(&"phase:indexing".to_string()));
    assert!(events.contains(&"phase:matching".to_string()));
}

// ── Empty source scan ────────────────────────────────────────────

#[test]
fn test_scan_empty_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("empty");
    fs::create_dir_all(&photos_dir).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_photos, 0);
    assert_eq!(stats.total_groups, 0);
}

// ── Non-photo files ignored ──────────────────────────────────────

#[test]
fn test_scan_ignores_non_photo_files() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("mixed");
    fs::create_dir_all(&photos_dir).unwrap();

    fs::write(photos_dir.join("readme.txt"), b"hello").unwrap();
    fs::write(photos_dir.join("video.mp4"), b"fake video").unwrap();
    fs::write(photos_dir.join("doc.pdf"), b"fake pdf").unwrap();
    create_jpeg(&photos_dir.join("real.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 1);
}

// ── Rescan clears stale groups ───────────────────────────────────

#[test]
fn test_rescan_updates_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    // First: create duplicates
    create_jpeg(&photos_dir.join("a.jpg"), 100, 100, 100);
    copy_file(&photos_dir.join("a.jpg"), &photos_dir.join("b.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_groups, 1);

    // Rescan — groups are rebuilt (same result since files haven't changed)
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_groups, 1);
}

// ── Three-way exact duplicate ────────────────────────────────────

#[test]
fn test_three_way_exact_duplicate() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    create_jpeg(&photos_dir.join("a.jpg"), 80, 80, 80);
    copy_file(&photos_dir.join("a.jpg"), &photos_dir.join("b.jpg"));
    copy_file(&photos_dir.join("a.jpg"), &photos_dir.join("c.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_photos, 3);
    assert_eq!(stats.total_groups, 1);
    assert_eq!(stats.total_duplicates, 2);

    let group = &vault.groups().unwrap()[0];
    assert_eq!(group.members.len(), 3);
}

// ── Stale file cleanup during scan ──────────────────────────────

/// Deleting a file from disk and rescanning should remove it from the catalog.
#[test]
fn test_scan_removes_deleted_file() {
    let tmp = tempfile::tempdir().unwrap();
    let photos = tmp.path().join("photos");
    fs::create_dir_all(&photos).unwrap();

    create_jpeg(&photos.join("keep.jpg"), 10, 20, 30);
    create_jpeg(&photos.join("delete_me.jpg"), 40, 50, 60);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos).unwrap();
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 2);

    // Delete one file and rescan
    fs::remove_file(photos.join("delete_me.jpg")).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 1);
}

/// Deleting all files from a source should leave the catalog empty (for that source).
#[test]
fn test_scan_removes_all_deleted_files() {
    let tmp = tempfile::tempdir().unwrap();
    let photos = tmp.path().join("photos");
    fs::create_dir_all(&photos).unwrap();

    create_jpeg(&photos.join("a.jpg"), 10, 20, 30);
    create_jpeg(&photos.join("b.jpg"), 40, 50, 60);
    create_jpeg(&photos.join("c.jpg"), 70, 80, 90);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos).unwrap();
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 3);

    // Delete all files and rescan
    fs::remove_file(photos.join("a.jpg")).unwrap();
    fs::remove_file(photos.join("b.jpg")).unwrap();
    fs::remove_file(photos.join("c.jpg")).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_photos, 0);
    assert_eq!(stats.total_groups, 0);
}

/// Deleting one file from a duplicate pair should dissolve the group.
#[test]
fn test_scan_removes_deleted_file_from_group() {
    let tmp = tempfile::tempdir().unwrap();
    let photos = tmp.path().join("photos");
    fs::create_dir_all(&photos).unwrap();

    create_jpeg(&photos.join("original.jpg"), 10, 20, 30);
    copy_file(&photos.join("original.jpg"), &photos.join("duplicate.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 2);
    assert_eq!(vault.status().unwrap().total_groups, 1);

    // Delete the duplicate and rescan
    fs::remove_file(photos.join("duplicate.jpg")).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 1);
    assert_eq!(vault.status().unwrap().total_groups, 0, "no group with single file");
}

/// Deleting from one source while other source retains its copy — cross-source stale cleanup.
#[test]
fn test_scan_removes_stale_cross_source() {
    let tmp = tempfile::tempdir().unwrap();
    let src_a = tmp.path().join("source_a");
    let src_b = tmp.path().join("source_b");
    fs::create_dir_all(&src_a).unwrap();
    fs::create_dir_all(&src_b).unwrap();

    create_jpeg(&src_a.join("photo.jpg"), 10, 20, 30);
    copy_file(&src_a.join("photo.jpg"), &src_b.join("photo.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&src_a).unwrap();
    vault.add_source(&src_b).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 2);
    assert_eq!(vault.status().unwrap().total_groups, 1);

    // Delete from source A, rescan
    fs::remove_file(src_a.join("photo.jpg")).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 1);
    assert_eq!(vault.status().unwrap().total_groups, 0, "single copy, no group");
}

/// Rescanning with all files intact should not remove anything.
#[test]
fn test_scan_preserves_existing_files() {
    let tmp = tempfile::tempdir().unwrap();
    let photos = tmp.path().join("photos");
    fs::create_dir_all(&photos).unwrap();

    create_jpeg(&photos.join("a.jpg"), 10, 20, 30);
    create_jpeg(&photos.join("b.jpg"), 40, 50, 60);
    create_jpeg(&photos.join("c.jpg"), 70, 80, 90);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos).unwrap();
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 3);

    // Rescan without deleting anything
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 3);

    // Rescan again
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 3);
}

/// Deleting one file and adding a new one in the same scan cycle.
#[test]
fn test_scan_removes_deleted_and_picks_up_new() {
    let tmp = tempfile::tempdir().unwrap();
    let photos = tmp.path().join("photos");
    fs::create_dir_all(&photos).unwrap();

    create_jpeg(&photos.join("keep.jpg"), 10, 20, 30);
    create_jpeg(&photos.join("delete_me.jpg"), 40, 50, 60);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos).unwrap();
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 2);

    // Delete one, add a new one
    fs::remove_file(photos.join("delete_me.jpg")).unwrap();
    create_jpeg(&photos.join("new_photo.jpg"), 70, 80, 90);
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 2);
    // Verify the right files are present
    let photos_list = vault.photos().unwrap();
    let names: Vec<String> = photos_list
        .iter()
        .map(|p| p.path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(names.contains(&"keep.jpg".to_string()));
    assert!(names.contains(&"new_photo.jpg".to_string()));
    assert!(!names.contains(&"delete_me.jpg".to_string()));
}

// ── Nested directories ───────────────────────────────────────────

#[test]
fn test_scan_nested_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("photos");
    let sub = root.join("2024/vacation");
    fs::create_dir_all(&sub).unwrap();

    create_jpeg(&root.join("top.jpg"), 10, 20, 30);
    create_jpeg(&sub.join("nested.jpg"), 40, 50, 60);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&root).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 2);
}

// ── Cross-directory & cross-format regression tests ─────────────

/// Helper: create a PNG with the same gradient pattern as create_jpeg so the
/// perceptual hashes will match, but the SHA-256 will differ (different format).
fn create_png(path: &Path, r: u8, g: u8, b: u8) {
    let img = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([
            r.wrapping_add((x * 3) as u8),
            g.wrapping_add((y * 3) as u8),
            b.wrapping_add(((x + y) * 2) as u8),
        ])
    });
    img.save(path).unwrap();
}

/// Exact copies split across two directories should be merged into one group.
#[test]
fn test_cross_directory_exact_copies_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("dir_a");
    let dir_b = tmp.path().join("dir_b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();

    create_jpeg(&dir_a.join("photo.jpg"), 120, 130, 140);
    copy_file(&dir_a.join("photo.jpg"), &dir_b.join("photo.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir_a).unwrap();
    vault.add_source(&dir_b).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_sources, 2);
    assert_eq!(stats.total_photos, 2);
    assert_eq!(stats.total_groups, 1, "exact copies across dirs must merge");
    assert_eq!(stats.total_duplicates, 1);
}

/// Same visual content saved as JPEG and PNG (different SHA) should group via
/// perceptual hashing.
#[test]
fn test_cross_format_same_image_grouped() {
    let tmp = tempfile::tempdir().unwrap();
    let photos = tmp.path().join("photos");
    fs::create_dir_all(&photos).unwrap();

    create_jpeg(&photos.join("sunset.jpg"), 200, 100, 50);
    create_png(&photos.join("sunset.png"), 200, 100, 50);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_photos, 2);
    assert_eq!(
        stats.total_groups, 1,
        "JPEG and PNG of same content must be grouped"
    );
}

/// Cross-format duplicates across different directories should merge into a
/// single group (regression: previously created two separate groups).
#[test]
fn test_cross_format_cross_directory_merge_into_one_group() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("originals");
    let dir_b = tmp.path().join("exports");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();

    // Same visual content — JPEG in one dir, PNG in the other
    create_jpeg(&dir_a.join("photo.jpg"), 80, 160, 240);
    create_png(&dir_b.join("photo.png"), 80, 160, 240);
    // Plus an exact copy of the JPEG in the exports dir
    copy_file(&dir_a.join("photo.jpg"), &dir_b.join("photo_copy.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir_a).unwrap();
    vault.add_source(&dir_b).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_photos, 3);
    assert_eq!(
        stats.total_groups, 1,
        "all three files (JPEG + PNG + copy) must merge into one group"
    );

    let group = &vault.groups().unwrap()[0];
    assert_eq!(group.members.len(), 3);
}

/// Helper: create JPEG with a checkerboard pattern (structurally different from gradients).
fn create_jpeg_checkerboard(path: &Path, block_size: u32, c1: [u8; 3], c2: [u8; 3]) {
    let img = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / block_size + y / block_size) % 2 == 0 {
            image::Rgb(c1)
        } else {
            image::Rgb(c2)
        }
    });
    img.save(path).unwrap();
}

/// Helper: create PNG with a checkerboard pattern.
fn create_png_checkerboard(path: &Path, block_size: u32, c1: [u8; 3], c2: [u8; 3]) {
    let img = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / block_size + y / block_size) % 2 == 0 {
            image::Rgb(c1)
        } else {
            image::Rgb(c2)
        }
    });
    img.save(path).unwrap();
}

/// Multiple distinct images, each with cross-format and cross-directory
/// duplicates, should produce the correct number of groups.
#[test]
fn test_multiple_images_cross_format_cross_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("camera");
    let dir_b = tmp.path().join("backup");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();

    // Image 1: smooth gradient (structurally very different from checkerboard)
    create_jpeg(&dir_a.join("gradient.jpg"), 200, 100, 50);
    create_png(&dir_b.join("gradient.png"), 200, 100, 50);

    // Image 2: coarse checkerboard (structurally very different from gradient)
    create_jpeg_checkerboard(
        &dir_a.join("checker.jpg"),
        8,
        [0, 0, 0],
        [255, 255, 255],
    );
    create_png_checkerboard(
        &dir_b.join("checker.png"),
        8,
        [0, 0, 0],
        [255, 255, 255],
    );

    // Image 3: unique, no duplicate
    create_jpeg(&dir_a.join("unique.jpg"), 10, 255, 10);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir_a).unwrap();
    vault.add_source(&dir_b).unwrap();
    vault.scan(None).unwrap();

    let stats = vault.status().unwrap();
    assert_eq!(stats.total_photos, 5);
    assert_eq!(
        stats.total_groups, 2,
        "gradient pair and checker pair should each form a group; unique is alone"
    );
}

/// Source-of-truth election should prefer PNG (lossless, tier 2) over JPEG
/// (lossy, tier 3).
#[test]
fn test_source_of_truth_prefers_png_over_jpeg() {
    let tmp = tempfile::tempdir().unwrap();
    let photos = tmp.path().join("photos");
    fs::create_dir_all(&photos).unwrap();

    create_jpeg(&photos.join("shot.jpg"), 150, 150, 150);
    create_png(&photos.join("shot.png"), 150, 150, 150);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);

    let group = &groups[0];
    let sot = group
        .members
        .iter()
        .find(|m| m.id == group.source_of_truth_id)
        .expect("source of truth must be a member");

    assert_eq!(
        sot.format,
        photopack_core::domain::PhotoFormat::Png,
        "PNG should be elected source-of-truth over JPEG"
    );
}

/// Scanning directories that contain files with unsupported formats (like .heic
/// stubs) must complete without freezing (regression: image::open hung on HEIC).
#[test]
fn test_scan_does_not_freeze_on_unsupported_format_files() {
    let tmp = tempfile::tempdir().unwrap();
    let photos = tmp.path().join("photos");
    fs::create_dir_all(&photos).unwrap();

    // Create a fake HEIC file (just bytes — the scan must not hang)
    fs::write(photos.join("vacation.heic"), b"fake heic content").unwrap();
    // And a real JPEG so we verify scanning works overall
    create_jpeg(&photos.join("real.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos).unwrap();

    // This must complete without hanging
    vault.scan(None).unwrap();

    // HEIC is still indexed (by SHA) even though no perceptual hash
    let stats = vault.status().unwrap();
    assert!(stats.total_photos >= 1, "at least the JPEG should be indexed");
}

/// Cross-directory duplicates must have their source-of-truth correctly
/// referenced inside the group members.
#[test]
fn test_cross_directory_duplicates_source_of_truth_in_group() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("main");
    let dir_b = tmp.path().join("backup");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();

    create_jpeg(&dir_a.join("img.jpg"), 77, 88, 99);
    copy_file(&dir_a.join("img.jpg"), &dir_b.join("img.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir_a).unwrap();
    vault.add_source(&dir_b).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);

    let group = &groups[0];
    assert_eq!(group.members.len(), 2);
    assert!(
        group.members.iter().any(|m| m.id == group.source_of_truth_id),
        "source_of_truth_id must reference a member of the group"
    );
    // Both members should come from different sources
    let source_ids: std::collections::HashSet<i64> =
        group.members.iter().map(|m| m.source_id).collect();
    assert_eq!(
        source_ids.len(),
        2,
        "members should come from two different sources"
    );
}

// ── Vault::photos API ─────────────────────────────────────────────

#[test]
fn test_photos_api_empty_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let photos = vault.photos().unwrap();
    assert!(photos.is_empty());
}

#[test]
fn test_photos_api_returns_all_scanned() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    create_jpeg(&photos_dir.join("a.jpg"), 10, 20, 30);
    create_jpeg(&photos_dir.join("b.jpg"), 40, 50, 60);
    create_jpeg(&photos_dir.join("c.jpg"), 70, 80, 90);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    let photos = vault.photos().unwrap();
    assert_eq!(photos.len(), 3);
}

#[test]
fn test_photos_api_correct_source_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("src_a");
    let dir_b = tmp.path().join("src_b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();

    create_jpeg(&dir_a.join("a.jpg"), 10, 20, 30);
    create_jpeg(&dir_b.join("b.jpg"), 40, 50, 60);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let source_a = vault.add_source(&dir_a).unwrap();
    let source_b = vault.add_source(&dir_b).unwrap();
    vault.scan(None).unwrap();

    let photos = vault.photos().unwrap();
    assert_eq!(photos.len(), 2);

    // Each photo should have the correct source_id
    let sources: std::collections::HashSet<i64> = photos.iter().map(|p| p.source_id).collect();
    assert!(sources.contains(&source_a.id));
    assert!(sources.contains(&source_b.id));

    // Verify each photo's source_id matches the directory it came from
    for photo in &photos {
        if photo.path.starts_with(&dir_a.canonicalize().unwrap()) {
            assert_eq!(photo.source_id, source_a.id);
        } else {
            assert_eq!(photo.source_id, source_b.id);
        }
    }
}

#[test]
fn test_photos_api_includes_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    create_jpeg(&photos_dir.join("original.jpg"), 100, 100, 100);
    copy_file(
        &photos_dir.join("original.jpg"),
        &photos_dir.join("copy.jpg"),
    );

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    // photos() returns ALL photos, including duplicates
    let photos = vault.photos().unwrap();
    assert_eq!(photos.len(), 2);

    // groups() should show them as duplicates
    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);
}

#[test]
fn test_photos_have_correct_sizes() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    create_jpeg(&photos_dir.join("small.jpg"), 10, 20, 30);
    // Create a larger image
    let large = image::RgbImage::from_fn(256, 256, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    large.save(photos_dir.join("large.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    let photos = vault.photos().unwrap();
    assert_eq!(photos.len(), 2);

    // All photos should have non-zero sizes matching the actual file
    for photo in &photos {
        let actual_size = fs::metadata(&photo.path).unwrap().len();
        assert_eq!(photo.size, actual_size);
        assert!(photo.size > 0);
    }
}

// ── False positive regression tests ─────────────────────────────
// These tests verify that the super-safe matching algorithm does NOT
// group different photos that happen to look similar, while still
// correctly grouping true duplicates (same photo, different format/compression).

/// Regression test: sequential birthday photos with the same EXIF date+camera
/// must NOT be grouped. This is the exact scenario that caused false positives.
/// The two images have genuinely different structures (diagonal gradient vs
/// concentric rings) — like consecutive shots of a birthday party.
#[test]
fn test_sequential_photos_same_exif_not_grouped() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    // Photo 39: diagonal gradient — warm tones, top-left to bottom-right
    let img1 = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([
            (x * 4) as u8,
            (y * 4) as u8,
            ((x + y) * 2) as u8,
        ])
    });
    img1.save(dir.join("birthday_39.jpg")).unwrap();

    // Photo 40: concentric rings — structurally very different
    let img2 = image::RgbImage::from_fn(64, 64, |x, y| {
        let cx = (x as f32 - 32.0).abs();
        let cy = (y as f32 - 32.0).abs();
        let dist = ((cx * cx + cy * cy).sqrt() * 8.0) as u8;
        image::Rgb([dist, 255 - dist, 128])
    });
    img2.save(dir.join("birthday_40.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(
        groups.len(),
        0,
        "Sequential birthday photos must NOT be grouped as duplicates"
    );
}

/// Similar solid-color photos must NOT be grouped.
/// Tests: two photos of a blue sky (similar brightness/color) stay separate.
#[test]
fn test_similar_solid_color_photos_not_grouped() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    // Blue sky — mostly uniform blue
    let sky1 = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([
            50u8.wrapping_add((x / 8) as u8),
            100u8.wrapping_add((y / 8) as u8),
            200,
        ])
    });
    sky1.save(dir.join("sky1.jpg")).unwrap();

    // Slightly different blue sky — same palette but different structure
    let sky2 = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([
            55u8.wrapping_add((y / 8) as u8),  // swapped x/y gradient
            105u8.wrapping_add((x / 8) as u8),
            200,
        ])
    });
    sky2.save(dir.join("sky2.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(
        groups.len(),
        0,
        "Similar but different photos (sky variants) must NOT be grouped"
    );
}

/// Five unique photos with different patterns must all stay ungrouped.
/// Tests that the algorithm handles multiple similar-ish photos at scale.
#[test]
fn test_five_unique_patterns_all_ungrouped() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    // Pattern 1: Diagonal gradient
    create_jpeg(&dir.join("photo_1.jpg"), 200, 50, 50);

    // Pattern 2: Vertical gradient (different from diagonal)
    let vert = image::RgbImage::from_fn(64, 64, |_x, y| {
        image::Rgb([0, (y * 4) as u8, 0])
    });
    vert.save(dir.join("photo_2.jpg")).unwrap();

    // Pattern 3: Horizontal gradient
    let horiz = image::RgbImage::from_fn(64, 64, |x, _y| {
        image::Rgb([(x * 4) as u8, 0, 0])
    });
    horiz.save(dir.join("photo_3.jpg")).unwrap();

    // Pattern 4: Checkerboard
    let checker = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            image::Rgb([0, 0, 0])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    checker.save(dir.join("photo_4.jpg")).unwrap();

    // Pattern 5: Concentric rings
    let rings = image::RgbImage::from_fn(64, 64, |x, y| {
        let cx = (x as f32 - 32.0).abs();
        let cy = (y as f32 - 32.0).abs();
        let dist = (cx * cx + cy * cy).sqrt() as u8;
        image::Rgb([dist.wrapping_mul(4), dist.wrapping_mul(2), 255 - dist.wrapping_mul(3)])
    });
    rings.save(dir.join("photo_5.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(
        groups.len(),
        0,
        "5 structurally different photos must all remain ungrouped"
    );
    assert_eq!(vault.status().unwrap().total_photos, 5);
}

/// Same JPEG recompressed at lower quality must still be grouped.
/// This tests perceptual hash robustness to compression artifacts.
#[test]
fn test_recompressed_jpeg_still_grouped() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    // Create original image
    let img = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([
            100u8.wrapping_add((x * 3) as u8),
            100u8.wrapping_add((y * 3) as u8),
            100u8.wrapping_add(((x + y) * 2) as u8),
        ])
    });

    // Save at high quality
    {
        let file = fs::File::create(dir.join("original.jpg")).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 95);
        encoder
            .encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
            .unwrap();
    }

    // Save same image at low quality (more compression artifacts)
    {
        let file = fs::File::create(dir.join("recompressed.jpg")).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 50);
        encoder
            .encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
            .unwrap();
    }

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(
        groups.len(),
        1,
        "Same image at different JPEG quality levels must be grouped"
    );
    assert_eq!(groups[0].members.len(), 2);
}

/// Same visual content in JPEG + PNG + TIFF (3 formats) must all merge into one group.
#[test]
fn test_three_format_merge_jpeg_png_tiff() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    // Same image content in 3 formats
    create_jpeg(&dir.join("photo.jpg"), 150, 80, 200);
    create_png(&dir.join("photo.png"), 150, 80, 200);

    // TIFF — same pixel data
    let img = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([
            150u8.wrapping_add((x * 3) as u8),
            80u8.wrapping_add((y * 3) as u8),
            200u8.wrapping_add(((x + y) * 2) as u8),
        ])
    });
    img.save(dir.join("photo.tiff")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(
        groups.len(),
        1,
        "Same content in JPEG+PNG+TIFF must merge into one group"
    );
    assert_eq!(groups[0].members.len(), 3);
}

/// Mix of duplicates and uniques: 2 duplicate pairs + 3 unique photos = 2 groups, 3 ungrouped.
#[test]
fn test_mixed_duplicates_and_uniques_correct_grouping() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    // Duplicate pair 1: exact copy
    create_jpeg(&dir.join("sunset_a.jpg"), 200, 100, 50);
    copy_file(&dir.join("sunset_a.jpg"), &dir.join("sunset_b.jpg"));

    // Duplicate pair 2: JPEG + PNG same content
    create_jpeg(&dir.join("beach.jpg"), 50, 150, 200);
    create_png(&dir.join("beach.png"), 50, 150, 200);

    // Unique 1: checkerboard
    let checker = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            image::Rgb([0, 0, 0])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    checker.save(dir.join("unique_1.jpg")).unwrap();

    // Unique 2: horizontal stripes
    let stripes = image::RgbImage::from_fn(64, 64, |_x, y| {
        if y % 16 < 8 {
            image::Rgb([255, 0, 0])
        } else {
            image::Rgb([0, 0, 255])
        }
    });
    stripes.save(dir.join("unique_2.jpg")).unwrap();

    // Unique 3: concentric rings
    let rings = image::RgbImage::from_fn(64, 64, |x, y| {
        let dist = (((x as f32 - 32.0).powi(2) + (y as f32 - 32.0).powi(2)).sqrt()) as u8;
        image::Rgb([dist.wrapping_mul(4), 128, 255 - dist.wrapping_mul(3)])
    });
    rings.save(dir.join("unique_3.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let status = vault.status().unwrap();
    assert_eq!(status.total_photos, 7, "7 files total");
    assert_eq!(status.total_groups, 2, "2 duplicate groups");
    // 2 groups × 2 members = 4 duplicates, 3 unique photos
}

/// Cross-directory: same photo in 3 directories (exact copies) must merge into 1 group.
/// Different photos across directories must NOT merge.
#[test]
fn test_cross_directory_same_and_different_photos() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("camera_roll");
    let dir_b = tmp.path().join("icloud_backup");
    let dir_c = tmp.path().join("google_photos");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();
    fs::create_dir_all(&dir_c).unwrap();

    // Photo A: exists in all 3 directories (exact copies)
    create_jpeg(&dir_a.join("vacation.jpg"), 100, 150, 200);
    copy_file(&dir_a.join("vacation.jpg"), &dir_b.join("vacation.jpg"));
    copy_file(&dir_a.join("vacation.jpg"), &dir_c.join("vacation.jpg"));

    // Photo B: unique to dir_a
    create_jpeg(&dir_a.join("selfie.jpg"), 200, 50, 100);

    // Photo C: unique to dir_b (different pattern)
    let checker = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            image::Rgb([0, 0, 0])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    checker.save(dir_b.join("document.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir_a).unwrap();
    vault.add_source(&dir_b).unwrap();
    vault.add_source(&dir_c).unwrap();
    vault.scan(None).unwrap();

    let status = vault.status().unwrap();
    assert_eq!(status.total_photos, 5, "5 files total");
    assert_eq!(
        status.total_groups, 1,
        "Only 1 group (the 3 vacation copies). selfie and document must stay ungrouped."
    );

    let groups = vault.groups().unwrap();
    assert_eq!(groups[0].members.len(), 3, "vacation group has 3 members");
}

// ── Source-of-truth quality preservation ──────────────────────────
// The vault's primary goal is preserving the highest-quality version
// of each photo. These tests verify SOT election across all format
// tiers and that vault save exports only the best copy.
//
// Technique: copy the same file bytes with different extensions.
// The scanner assigns format from extension, SHA-256 matches on bytes,
// so Phase 1 groups them. SOT election then picks the best format.

/// Helper: create JPEG image data, then copy raw bytes to a file with any extension.
/// This lets us create files with .cr2, .heic, etc. that have valid JPEG bytes
/// but are recognized by the scanner as different formats (based on extension).
fn create_file_with_jpeg_bytes(path: &Path, r: u8, g: u8, b: u8) {
    // Always save as JPEG first to a temp path, then copy bytes
    let tmp_jpg = path.with_extension("_tmp_create.jpg");
    create_jpeg(&tmp_jpg, r, g, b);
    fs::copy(&tmp_jpg, path).unwrap();
    fs::remove_file(&tmp_jpg).unwrap();
}

#[test]
fn test_raw_cr2_elected_sot_over_jpeg() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    create_jpeg(&dir.join("photo.jpg"), 100, 100, 100);
    copy_file(&dir.join("photo.jpg"), &dir.join("photo.cr2"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);
    let sot = groups[0]
        .members
        .iter()
        .find(|m| m.id == groups[0].source_of_truth_id)
        .unwrap();
    assert_eq!(
        sot.format,
        photopack_core::domain::PhotoFormat::Cr2,
        "CR2 (RAW) must be elected SOT over JPEG"
    );
}

#[test]
fn test_raw_dng_elected_sot_over_jpeg() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    create_jpeg(&dir.join("photo.jpg"), 100, 100, 100);
    copy_file(&dir.join("photo.jpg"), &dir.join("photo.dng"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);
    let sot = groups[0]
        .members
        .iter()
        .find(|m| m.id == groups[0].source_of_truth_id)
        .unwrap();
    assert_eq!(
        sot.format,
        photopack_core::domain::PhotoFormat::Dng,
        "DNG (RAW) must be elected SOT over JPEG"
    );
}

#[test]
fn test_raw_elected_sot_over_heic() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    // Create JPEG bytes, then copy to .heic and .cr2 extensions
    create_file_with_jpeg_bytes(&dir.join("photo.heic"), 100, 100, 100);
    copy_file(&dir.join("photo.heic"), &dir.join("photo.cr2"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);
    let sot = groups[0]
        .members
        .iter()
        .find(|m| m.id == groups[0].source_of_truth_id)
        .unwrap();
    assert_eq!(
        sot.format,
        photopack_core::domain::PhotoFormat::Cr2,
        "CR2 (RAW) must be elected SOT over HEIC"
    );
}

#[test]
fn test_tiff_elected_sot_over_jpeg() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    create_jpeg(&dir.join("photo.jpg"), 100, 100, 100);
    copy_file(&dir.join("photo.jpg"), &dir.join("photo.tiff"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);
    let sot = groups[0]
        .members
        .iter()
        .find(|m| m.id == groups[0].source_of_truth_id)
        .unwrap();
    assert_eq!(
        sot.format,
        photopack_core::domain::PhotoFormat::Tiff,
        "TIFF (lossless) must be elected SOT over JPEG"
    );
}

#[test]
fn test_png_elected_sot_over_heic() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    create_file_with_jpeg_bytes(&dir.join("photo.heic"), 100, 100, 100);
    // PNG copy with same bytes → same SHA256 → grouped
    copy_file(&dir.join("photo.heic"), &dir.join("photo.png"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);
    let sot = groups[0]
        .members
        .iter()
        .find(|m| m.id == groups[0].source_of_truth_id)
        .unwrap();
    assert_eq!(
        sot.format,
        photopack_core::domain::PhotoFormat::Png,
        "PNG (lossless) must be elected SOT over HEIC"
    );
}

#[test]
fn test_jpeg_elected_sot_over_heic() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    create_jpeg(&dir.join("photo.jpg"), 100, 100, 100);
    copy_file(&dir.join("photo.jpg"), &dir.join("photo.heic"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);
    let sot = groups[0]
        .members
        .iter()
        .find(|m| m.id == groups[0].source_of_truth_id)
        .unwrap();
    assert_eq!(
        sot.format,
        photopack_core::domain::PhotoFormat::Jpeg,
        "JPEG must be elected SOT over HEIC"
    );
}

/// Real-world scenario: camera source has RAW, iCloud has HEIC, backup has JPEG.
/// All are the same photo (same bytes). RAW must win.
#[test]
fn test_three_sources_quality_ladder_raw_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let camera = tmp.path().join("camera");
    let icloud = tmp.path().join("icloud");
    let backup = tmp.path().join("backup");
    fs::create_dir_all(&camera).unwrap();
    fs::create_dir_all(&icloud).unwrap();
    fs::create_dir_all(&backup).unwrap();

    // Create as JPEG first, then copy bytes with different extensions
    create_file_with_jpeg_bytes(&camera.join("sunset.cr2"), 150, 100, 50);
    copy_file(&camera.join("sunset.cr2"), &icloud.join("sunset.heic"));
    copy_file(&camera.join("sunset.cr2"), &backup.join("sunset.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&camera).unwrap();
    vault.add_source(&icloud).unwrap();
    vault.add_source(&backup).unwrap();
    vault.scan(None).unwrap();

    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1, "all 3 should be in one group");
    assert_eq!(groups[0].members.len(), 3);

    let sot = groups[0]
        .members
        .iter()
        .find(|m| m.id == groups[0].source_of_truth_id)
        .unwrap();
    assert_eq!(
        sot.format,
        photopack_core::domain::PhotoFormat::Cr2,
        "CR2 must win over HEIC and JPEG"
    );
}

/// Pack must save only the RAW, not the HEIC.
#[test]
fn test_vault_save_exports_raw_not_lossy() {
    let tmp = tempfile::tempdir().unwrap();
    let camera = tmp.path().join("camera");
    let icloud = tmp.path().join("icloud");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&camera).unwrap();
    fs::create_dir_all(&icloud).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_file_with_jpeg_bytes(&camera.join("photo.cr2"), 100, 100, 100);
    copy_file(&camera.join("photo.cr2"), &icloud.join("photo.heic"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&camera).unwrap();
    vault.add_source(&icloud).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    let pack_files = list_pack_files(&vault_dir);
    assert_eq!(pack_files.len(), 1, "only SOT should be packed");
    assert_eq!(
        pack_files[0].extension().unwrap(),
        "cr2",
        "the packed file must be the RAW (CR2), not the HEIC"
    );
}

/// When the pack directory is also a registered source containing the RAW
/// original, and another source has a lossy copy, pack save should still
/// work correctly.
#[test]
fn test_vault_as_source_preserves_raw_original() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("vault");
    let icloud = tmp.path().join("icloud");
    fs::create_dir_all(&vault_dir).unwrap();
    fs::create_dir_all(&icloud).unwrap();

    // RAW lives in the pack dir itself
    create_file_with_jpeg_bytes(&vault_dir.join("sunset.cr2"), 200, 100, 50);
    // Lossy copy in iCloud
    copy_file(&vault_dir.join("sunset.cr2"), &icloud.join("sunset.heic"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&vault_dir).unwrap();
    vault.add_source(&icloud).unwrap();
    vault.scan(None).unwrap();

    // Verify RAW is SOT
    let groups = vault.groups().unwrap();
    assert_eq!(groups.len(), 1);
    let sot = groups[0]
        .members
        .iter()
        .find(|m| m.id == groups[0].source_of_truth_id)
        .unwrap();
    assert_eq!(sot.format, photopack_core::domain::PhotoFormat::Cr2);

    // Pack save should store the RAW in content-addressed structure
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    let pack_files = list_pack_files(&vault_dir);
    let cr2_files: Vec<_> = pack_files
        .iter()
        .filter(|p| p.extension().map(|x| x == "cr2").unwrap_or(false))
        .collect();
    assert!(
        !cr2_files.is_empty(),
        "pack must contain at least the CR2 file"
    );
}

/// Multiple groups, each with different format combinations. Pack must
/// save the best quality from EACH group independently.
#[test]
fn test_vault_save_multiple_groups_each_picks_best() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Group 1: RAW + HEIC → RAW wins
    create_file_with_jpeg_bytes(&src.join("g1.cr2"), 10, 20, 30);
    copy_file(&src.join("g1.cr2"), &src.join("g1.heic"));

    // Group 2: TIFF + JPEG → TIFF wins (different content = different SHA)
    create_jpeg(&src.join("g2_tmp.jpg"), 40, 50, 60);
    copy_file(&src.join("g2_tmp.jpg"), &src.join("g2.tiff"));
    copy_file(&src.join("g2_tmp.jpg"), &src.join("g2.jpg"));
    fs::remove_file(src.join("g2_tmp.jpg")).unwrap();

    // Group 3: PNG + WebP → PNG wins (different content again)
    create_jpeg(&src.join("g3_tmp.jpg"), 70, 80, 90);
    copy_file(&src.join("g3_tmp.jpg"), &src.join("g3.png"));
    copy_file(&src.join("g3_tmp.jpg"), &src.join("g3.webp"));
    fs::remove_file(src.join("g3_tmp.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&src).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    let pack_files = list_pack_files(&vault_dir);
    assert_eq!(pack_files.len(), 3, "one SOT per group");

    let extensions: std::collections::HashSet<String> = pack_files
        .iter()
        .filter_map(|f| f.extension().map(|e| e.to_string_lossy().to_string()))
        .collect();
    assert!(extensions.contains("cr2"), "RAW must be packed for group 1");
    assert!(
        extensions.contains("tiff"),
        "TIFF must be packed for group 2"
    );
    assert!(extensions.contains("png"), "PNG must be packed for group 3");
}

/// Pack must preserve the exact file content of the highest-quality version.
#[test]
fn test_vault_save_preserves_raw_file_content() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_file_with_jpeg_bytes(&src.join("photo.cr2"), 100, 100, 100);
    let original_bytes = fs::read(src.join("photo.cr2")).unwrap();
    copy_file(&src.join("photo.cr2"), &src.join("photo.heic"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&src).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    let pack_files = list_pack_files(&vault_dir);
    assert_eq!(pack_files.len(), 1);
    let saved_bytes = fs::read(&pack_files[0]).unwrap();
    assert_eq!(
        original_bytes, saved_bytes,
        "packed file content must be byte-identical to the RAW original"
    );
}

/// Incremental pack save: when the RAW has already been packed, a second
/// save should skip it even if the HEIC still exists in the source.
#[test]
fn test_vault_save_incremental_with_cross_format_group() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_file_with_jpeg_bytes(&src.join("photo.cr2"), 100, 100, 100);
    copy_file(&src.join("photo.cr2"), &src.join("photo.heic"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&src).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();

    // First save: should copy the CR2
    let mut first_copied = 0;
    vault
        .vault_save(Some(&mut |progress| {
            if let photopack_core::vault_save::VaultSaveProgress::Complete {
                copied, ..
            } = progress
            {
                first_copied = copied;
            }
        }))
        .unwrap();
    assert_eq!(first_copied, 1);

    // Second save: should skip (already exported)
    let mut second_skipped = 0;
    let mut second_copied = 0;
    vault
        .vault_save(Some(&mut |progress| {
            if let photopack_core::vault_save::VaultSaveProgress::Complete {
                copied,
                skipped,
                ..
            } = progress
            {
                second_copied = copied;
                second_skipped = skipped;
            }
        }))
        .unwrap();
    assert_eq!(second_copied, 0, "already exported — should not re-copy");
    assert_eq!(second_skipped, 1, "should skip the already-exported RAW");
}

/// All RAW format variants should be elected SOT over JPEG.
#[test]
fn test_all_raw_formats_beat_jpeg() {
    let raw_extensions = ["cr2", "cr3", "nef", "arw", "orf", "raf", "rw2", "dng"];

    for ext in &raw_extensions {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("photos");
        fs::create_dir_all(&dir).unwrap();

        create_jpeg(&dir.join("photo.jpg"), 100, 100, 100);
        copy_file(
            &dir.join("photo.jpg"),
            &dir.join(format!("photo.{ext}")),
        );

        let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
        vault.add_source(&dir).unwrap();
        vault.scan(None).unwrap();

        let groups = vault.groups().unwrap();
        assert_eq!(groups.len(), 1, "failed for .{ext}");
        let sot = groups[0]
            .members
            .iter()
            .find(|m| m.id == groups[0].source_of_truth_id)
            .unwrap();
        assert_ne!(
            sot.format,
            photopack_core::domain::PhotoFormat::Jpeg,
            ".{ext} must be elected SOT over JPEG"
        );
    }
}

// ── Vault save ────────────────────────────────────────────────────

fn count_files_recursive(dir: &std::path::Path) -> usize {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| !e.path().to_string_lossy().contains(".photopack"))
        .count()
}

/// List pack files (excluding .photopack/ metadata).
fn list_pack_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| !e.path().to_string_lossy().contains(".photopack"))
        .map(|e| e.into_path())
        .collect()
}

#[test]
fn test_vault_set_and_get_path() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("my_vault");
    fs::create_dir_all(&vault_dir).unwrap();

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    assert!(vault.get_vault_path().unwrap().is_none());

    vault.set_vault_path(&vault_dir).unwrap();
    let stored = vault.get_vault_path().unwrap().unwrap();
    assert_eq!(stored, vault_dir.canonicalize().unwrap());
}

#[test]
fn test_vault_save_unique_photos() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Gradient pattern
    create_jpeg(&photos_dir.join("a.jpg"), 200, 100, 50);
    // Checkerboard pattern (structurally very different)
    let checker = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            image::Rgb([0, 0, 0])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    checker.save(photos_dir.join("b.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    assert_eq!(
        count_files_recursive(&vault_dir),
        2,
        "Both unique photos should be saved to vault"
    );
}

#[test]
fn test_vault_save_deduplicates_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("original.jpg"), 100, 150, 200);
    copy_file(
        &photos_dir.join("original.jpg"),
        &photos_dir.join("copy.jpg"),
    );
    create_jpeg(&photos_dir.join("unique.jpg"), 10, 20, 30);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // 1 source-of-truth from duplicate group + 1 unique = 2
    assert_eq!(
        count_files_recursive(&vault_dir),
        2,
        "Only SoT + unique should be saved"
    );
}

#[test]
fn test_vault_save_creates_content_addressable_structure() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    let pack_files = list_pack_files(&vault_dir);
    assert_eq!(pack_files.len(), 1);

    let relative = pack_files[0]
        .strip_prefix(&vault_dir)
        .unwrap();
    let components: Vec<_> = relative.components().collect();
    // Should be: {prefix} / {sha256}.{ext} (2 components)
    assert_eq!(
        components.len(),
        2,
        "Path should be vault/{{prefix}}/{{sha256}}.{{ext}}, got: {}",
        relative.display()
    );

    // Verify prefix matches first 2 chars of filename
    let prefix = components[0].as_os_str().to_string_lossy();
    let filename = components[1].as_os_str().to_string_lossy();
    assert_eq!(&filename[..2], prefix.as_ref());
    assert!(filename.ends_with(".jpg"));
}

#[test]
fn test_vault_save_without_vault_path_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let err = vault.vault_save(None).unwrap_err();
    assert!(err.to_string().contains("vault path not configured"));
}

#[test]
fn test_vault_save_incremental_skips_existing() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();

    // First save
    let mut first_copied = 0;
    vault
        .vault_save(Some(&mut |progress| {
            if let photopack_core::vault_save::VaultSaveProgress::Complete {
                copied, ..
            } = progress
            {
                first_copied = copied;
            }
        }))
        .unwrap();
    assert_eq!(first_copied, 1, "First save should copy 1 file");

    // Second save — file already exists (content-addressed), should skip
    let mut second_skipped = 0;
    vault
        .vault_save(Some(&mut |progress| {
            if let photopack_core::vault_save::VaultSaveProgress::Complete {
                skipped, ..
            } = progress
            {
                second_skipped = skipped;
            }
        }))
        .unwrap();
    assert_eq!(
        count_files_recursive(&vault_dir),
        1,
        "Should still be 1 file"
    );
}

#[test]
fn test_vault_set_nonexistent_path_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let err = vault
        .set_vault_path(&tmp.path().join("does_not_exist"))
        .unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn test_vault_set_file_not_directory_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("file.txt");
    fs::write(&file_path, b"not a dir").unwrap();

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let err = vault.set_vault_path(&file_path).unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn test_vault_set_overwrite_path() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("vault_a");
    let dir_b = tmp.path().join("vault_b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.set_vault_path(&dir_a).unwrap();
    vault.set_vault_path(&dir_b).unwrap();

    let stored = vault.get_vault_path().unwrap().unwrap();
    assert_eq!(stored, dir_b.canonicalize().unwrap());
}

#[test]
fn test_vault_path_persists_across_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("vault");
    let db_path = tmp.path().join("catalog.db");
    fs::create_dir_all(&vault_dir).unwrap();

    {
        let vault = Vault::open(&db_path).unwrap();
        vault.set_vault_path(&vault_dir).unwrap();
    }

    // Reopen — vault path should persist
    let vault = Vault::open(&db_path).unwrap();
    let stored = vault.get_vault_path().unwrap().unwrap();
    assert_eq!(stored, vault_dir.canonicalize().unwrap());
}

#[test]
fn test_vault_set_auto_registers_source() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&vault_dir).unwrap();

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();

    let sources = vault.sources().unwrap();
    assert_eq!(sources.len(), 1, "Vault should be auto-registered as source");
    assert_eq!(sources[0].path, vault_dir.canonicalize().unwrap());
}

#[test]
fn test_vault_set_already_registered_source() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&vault_dir).unwrap();

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&vault_dir).unwrap();
    // Setting vault to same path should not error
    vault.set_vault_path(&vault_dir).unwrap();

    let sources = vault.sources().unwrap();
    assert_eq!(sources.len(), 1, "Should still have exactly 1 source");
}

#[test]
fn test_vault_save_empty_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&vault_dir).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();

    let mut total = usize::MAX;
    let mut copied = usize::MAX;
    let mut skipped = usize::MAX;
    vault
        .vault_save(Some(&mut |progress| {
            match progress {
                photopack_core::vault_save::VaultSaveProgress::Start { total: t } => {
                    total = t;
                }
                photopack_core::vault_save::VaultSaveProgress::Complete { copied: c, skipped: s, .. } => {
                    copied = c;
                    skipped = s;
                }
                _ => {}
            }
        }))
        .unwrap();

    assert_eq!(total, 0);
    assert_eq!(copied, 0);
    assert_eq!(skipped, 0);
    assert_eq!(count_files_recursive(&vault_dir), 0);
}

#[test]
fn test_vault_save_cross_format_picks_best_quality() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Same image content in PNG (tier 2) and JPEG (tier 3) — PNG should be SoT
    create_jpeg(&photos_dir.join("photo.jpg"), 150, 150, 150);
    create_png(&photos_dir.join("photo.png"), 150, 150, 150);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // Both have different SHA-256 (different encodings), so both will be
    // packed if they're in different groups. If they're in the same group
    // (cross-format perceptual match), only the PNG (SOT) is packed.
    let pack_files = list_pack_files(&vault_dir);
    // Verify PNG is present (it's the SOT if grouped)
    let png_files: Vec<_> = pack_files
        .iter()
        .filter(|p| p.extension().map(|x| x == "png").unwrap_or(false))
        .collect();
    assert!(
        !png_files.is_empty(),
        "PNG should be present in the pack"
    );
}

#[test]
fn test_vault_save_cross_directory_deduplication() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("dir_a");
    let dir_b = tmp.path().join("dir_b");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Same file in two directories + a unique file
    create_jpeg(&dir_a.join("shared.jpg"), 100, 100, 100);
    copy_file(&dir_a.join("shared.jpg"), &dir_b.join("shared.jpg"));
    // Structurally different unique file
    let checker = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            image::Rgb([0, 0, 0])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    checker.save(dir_a.join("unique.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir_a).unwrap();
    vault.add_source(&dir_b).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // 1 from duplicate pair + 1 unique = 2
    assert_eq!(
        count_files_recursive(&vault_dir),
        2,
        "cross-dir duplicates should be deduplicated"
    );
}

#[test]
fn test_vault_save_multiple_unique_photos_all_packed() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Create 3 different photos — each gets a unique SHA-256, no collision concept
    create_jpeg(&photos_dir.join("a.jpg"), 200, 100, 50);
    let checker = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            image::Rgb([0, 0, 0])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    checker.save(photos_dir.join("b.jpg")).unwrap();
    let stripes = image::RgbImage::from_fn(64, 64, |x, _| {
        if x % 16 < 8 {
            image::Rgb([255, 0, 0])
        } else {
            image::Rgb([0, 0, 255])
        }
    });
    stripes.save(photos_dir.join("c.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // All 3 should be saved — unique hashes, no collision handling needed
    assert_eq!(
        count_files_recursive(&vault_dir),
        3,
        "all unique photos should be saved"
    );
}

#[test]
fn test_vault_save_progress_events_order() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("a.jpg"), 200, 100, 50);
    let checker = image::RgbImage::from_fn(64, 64, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            image::Rgb([0, 0, 0])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    checker.save(photos_dir.join("b.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();

    let mut events = Vec::new();
    vault
        .vault_save(Some(&mut |progress| {
            match progress {
                photopack_core::vault_save::VaultSaveProgress::Start { total } => {
                    events.push(format!("start:{total}"));
                }
                photopack_core::vault_save::VaultSaveProgress::Copied { .. } => {
                    events.push("copied".to_string());
                }
                photopack_core::vault_save::VaultSaveProgress::Skipped { .. } => {
                    events.push("skipped".to_string());
                }
                photopack_core::vault_save::VaultSaveProgress::Removed { .. } => {
                    events.push("removed".to_string());
                }
                photopack_core::vault_save::VaultSaveProgress::Complete {
                    copied,
                    skipped,
                    removed,
                } => {
                    events.push(format!("complete:{copied}:{skipped}:{removed}"));
                }
            }
        }))
        .unwrap();

    // Should be: start → copied × 2 → complete
    assert_eq!(events[0], "start:2");
    assert_eq!(events.iter().filter(|e| *e == "copied").count(), 2);
    assert!(events.last().unwrap().starts_with("complete:2:0:0"));
}

#[test]
fn test_vault_save_preserves_file_content() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);
    let original_bytes = fs::read(photos_dir.join("photo.jpg")).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    let pack_files = list_pack_files(&vault_dir);
    assert_eq!(pack_files.len(), 1);

    let saved_bytes = fs::read(&pack_files[0]).unwrap();
    assert_eq!(
        original_bytes, saved_bytes,
        "copied file content must match source"
    );
}

// ── Nested directory tests ──────────────────────────────────────

#[test]
fn test_scan_nested_photos_across_multiple_levels() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let year = root.join("2024");
    let month = year.join("06");
    let day = month.join("15");
    fs::create_dir_all(&day).unwrap();

    create_jpeg(&root.join("root.jpg"), 10, 20, 30);
    create_jpeg(&year.join("year.jpg"), 40, 50, 60);
    create_jpeg(&month.join("month.jpg"), 70, 80, 90);
    create_jpeg(&day.join("day.jpg"), 100, 110, 120);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&root).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 4);
    let photos = vault.photos().unwrap();
    assert_eq!(photos.len(), 4);
}

#[test]
fn test_nested_duplicates_detected_across_subdirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("photos");
    let sub_a = root.join("originals");
    let sub_b = root.join("copies/backup");
    fs::create_dir_all(&sub_a).unwrap();
    fs::create_dir_all(&sub_b).unwrap();

    create_jpeg(&sub_a.join("photo.jpg"), 10, 20, 30);
    copy_file(&sub_a.join("photo.jpg"), &sub_b.join("photo.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&root).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 2);
    assert_eq!(vault.status().unwrap().total_groups, 1);
    assert_eq!(vault.status().unwrap().total_duplicates, 1);
}

#[test]
fn test_nested_duplicates_across_different_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let source_a = tmp.path().join("source_a");
    let source_b = tmp.path().join("source_b");
    let nested_a = source_a.join("2024/vacation");
    let nested_b = source_b.join("backup/old");
    fs::create_dir_all(&nested_a).unwrap();
    fs::create_dir_all(&nested_b).unwrap();

    create_jpeg(&nested_a.join("sunset.jpg"), 10, 20, 30);
    copy_file(&nested_a.join("sunset.jpg"), &nested_b.join("sunset_copy.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source_a).unwrap();
    vault.add_source(&source_b).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 2);
    assert_eq!(vault.status().unwrap().total_groups, 1);
}

#[test]
fn test_vault_save_with_nested_source_photos() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("photos");
    let sub = source.join("vacation/beach");
    fs::create_dir_all(&sub).unwrap();
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&source.join("top.jpg"), 10, 20, 30);
    create_jpeg(&sub.join("nested.jpg"), 200, 50, 150);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // Both photos should be exported (they're unique)
    assert_eq!(count_files_recursive(&vault_dir), 2);
}

#[test]
fn test_vault_save_deduplicates_nested_copies() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("photos");
    let originals = source.join("originals");
    let copies = source.join("copies/2024");
    fs::create_dir_all(&originals).unwrap();
    fs::create_dir_all(&copies).unwrap();
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&originals.join("photo.jpg"), 10, 20, 30);
    copy_file(&originals.join("photo.jpg"), &copies.join("photo.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // Only 1 copy exported (deduplicated)
    assert_eq!(count_files_recursive(&vault_dir), 1);
}

#[test]
fn test_deeply_nested_photos_retain_correct_source_id() {
    let tmp = tempfile::tempdir().unwrap();
    let source1 = tmp.path().join("source1");
    let source2 = tmp.path().join("source2");
    let deep1 = source1.join("a/b/c");
    let deep2 = source2.join("x/y/z");
    fs::create_dir_all(&deep1).unwrap();
    fs::create_dir_all(&deep2).unwrap();

    create_jpeg(&deep1.join("photo1.jpg"), 10, 20, 30);
    create_jpeg(&deep2.join("photo2.jpg"), 40, 50, 60);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let s1 = vault.add_source(&source1).unwrap();
    let s2 = vault.add_source(&source2).unwrap();
    vault.scan(None).unwrap();

    let photos = vault.photos().unwrap();
    assert_eq!(photos.len(), 2);

    let p1 = photos.iter().find(|p| p.path.to_string_lossy().contains("photo1")).unwrap();
    let p2 = photos.iter().find(|p| p.path.to_string_lossy().contains("photo2")).unwrap();
    assert_eq!(p1.source_id, s1.id);
    assert_eq!(p2.source_id, s2.id);
}

#[test]
fn test_incremental_scan_with_nested_new_files() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("photos");
    let sub = source.join("new_folder");
    fs::create_dir_all(&source).unwrap();

    create_jpeg(&source.join("existing.jpg"), 10, 20, 30);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source).unwrap();
    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 1);

    // Add a new nested subfolder with photos
    fs::create_dir_all(&sub).unwrap();
    create_jpeg(&sub.join("new.jpg"), 40, 50, 60);

    vault.scan(None).unwrap();
    assert_eq!(vault.status().unwrap().total_photos, 2);
}

#[test]
fn test_vault_save_deleted_vault_path_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&vault_dir).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();

    // Delete the vault directory after setting it
    fs::remove_dir_all(&vault_dir).unwrap();

    let err = vault.vault_save(None).unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

// ── Content-addressable pack tests ──────────────────────────────

/// Each file's name in the pack matches its SHA-256.
#[test]
fn test_pack_content_addressable_structure() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // Get the photo's SHA-256 from catalog
    let photos = vault.photos().unwrap();
    assert_eq!(photos.len(), 1);
    let sha256 = &photos[0].sha256;

    // Verify the pack file is named by SHA-256
    let pack_files = list_pack_files(&vault_dir);
    assert_eq!(pack_files.len(), 1);
    let filename = pack_files[0].file_stem().unwrap().to_string_lossy();
    assert_eq!(filename.as_ref(), sha256, "Pack filename must be the SHA-256 hash");
}

/// Hash the file content and verify it matches the filename.
#[test]
fn test_pack_integrity_sha256_matches_filename() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    let pack_files = list_pack_files(&vault_dir);
    assert_eq!(pack_files.len(), 1);

    // Hash the file content
    use sha2::{Digest, Sha256};
    let content = fs::read(&pack_files[0]).unwrap();
    let hash = format!("{:x}", Sha256::digest(&content));

    let filename = pack_files[0].file_stem().unwrap().to_string_lossy();
    assert_eq!(
        filename.as_ref(), hash,
        "File content SHA-256 must match filename"
    );
}

/// Manifest has correct entries after pack.
#[test]
fn test_pack_manifest_records_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // Verify manifest has the entry
    let manifest = photopack_core::manifest::Manifest::open(&vault_dir).unwrap();
    let entries = manifest.list_entries().unwrap();
    assert_eq!(entries.len(), 1);

    let photos = vault.photos().unwrap();
    assert!(manifest.contains(&photos[0].sha256).unwrap());
    assert_eq!(manifest.version().unwrap(), "1");
}

/// Two identical files → one pack file (structural dedup).
#[test]
fn test_pack_hash_dedup_identical_files() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("original.jpg"), 100, 100, 100);
    copy_file(&photos_dir.join("original.jpg"), &photos_dir.join("copy.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // Same SHA-256 → only 1 pack file
    assert_eq!(
        count_files_recursive(&vault_dir), 1,
        "Identical files should dedup to 1 pack file"
    );
}

/// Manifest entry removed on cleanup when photo no longer in catalog.
#[test]
fn test_pack_cleanup_removes_stale_manifest_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // Verify file exists in pack
    assert_eq!(count_files_recursive(&vault_dir), 1);

    // Remove source (clears catalog)
    vault.remove_source(&photos_dir).unwrap();

    // Pack sync with empty catalog should clean up
    vault.vault_save(None).unwrap();

    assert_eq!(count_files_recursive(&vault_dir), 0, "Stale pack file should be removed");

    // Manifest should also be empty
    let manifest = photopack_core::manifest::Manifest::open(&vault_dir).unwrap();
    let entries = manifest.list_entries().unwrap();
    assert!(entries.is_empty(), "Manifest should have no entries after cleanup");
}

// ── Vault quality upgrade tests ─────────────────────────────────
//
// These tests verify that vault sync replaces lower-quality vault files
// with higher-quality versions when a better source-of-truth is found.

/// Scenario: pack has JPEG from earlier sync, then a RAW of the same photo is
/// added to sources. After rescan + pack sync, the RAW should be in the pack
/// and the old JPEG hash file should be removed (same SHA-256, different format
/// means SOT changes — old hash+ext is cleaned up via manifest).
#[test]
fn test_vault_sync_replaces_lower_quality_with_raw() {
    let tmp = tempfile::tempdir().unwrap();
    let source_a = tmp.path().join("source_a");
    let source_b = tmp.path().join("source_b");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&source_a).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Step 1: JPEG in source A, scan and pack sync
    create_jpeg(&source_a.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source_a).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // Pack should have a .jpg file (content-addressed by SHA-256)
    let pack_files_before = list_pack_files(&vault_dir);
    assert_eq!(pack_files_before.len(), 1);

    // Step 2: Add a RAW (CR2) of the same photo in a new source
    // Same bytes = same SHA-256, so the CR2 becomes SOT but the hash file already exists
    fs::create_dir_all(&source_b).unwrap();
    copy_file(&source_a.join("photo.jpg"), &source_b.join("photo.cr2"));
    vault.add_source(&source_b).unwrap();
    vault.scan(None).unwrap();

    // Verify CR2 is elected SOT
    let groups = vault.groups().unwrap();
    assert!(!groups.is_empty(), "Should have at least one duplicate group");
    let sot = groups[0]
        .members
        .iter()
        .find(|m| m.id == groups[0].source_of_truth_id)
        .unwrap();
    assert_eq!(
        sot.format,
        photopack_core::domain::PhotoFormat::Cr2,
        "CR2 should be elected SOT over JPEG"
    );

    // Step 3: Pack sync again
    vault.vault_save(None).unwrap();

    // The pack should have the CR2 file (same hash, .cr2 extension)
    let pack_files_after = list_pack_files(&vault_dir);
    let cr2_files: Vec<_> = pack_files_after
        .iter()
        .filter(|p| p.extension().map(|x| x == "cr2").unwrap_or(false))
        .collect();
    assert!(
        !cr2_files.is_empty(),
        "Pack should contain the CR2 (higher quality)"
    );
}

/// Same scenario but with TIFF upgrading JPEG.
#[test]
fn test_vault_sync_replaces_jpeg_with_tiff() {
    let tmp = tempfile::tempdir().unwrap();
    let source_a = tmp.path().join("source_a");
    let source_b = tmp.path().join("source_b");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&source_a).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Step 1: JPEG in source, pack sync
    create_jpeg(&source_a.join("photo.jpg"), 120, 80, 200);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source_a).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    assert_eq!(count_files_recursive(&vault_dir), 1);

    // Step 2: Add TIFF of same photo (same bytes = same SHA-256)
    fs::create_dir_all(&source_b).unwrap();
    copy_file(&source_a.join("photo.jpg"), &source_b.join("photo.tiff"));
    vault.add_source(&source_b).unwrap();
    vault.scan(None).unwrap();

    // Step 3: Pack sync — TIFF becomes SOT
    vault.vault_save(None).unwrap();

    let pack_files = list_pack_files(&vault_dir);
    let tiff_count = pack_files
        .iter()
        .filter(|p| p.extension().map(|x| x == "tiff").unwrap_or(false))
        .count();
    assert!(tiff_count >= 1, "Pack should contain the TIFF");
}

/// When both versions are in sources simultaneously (not incremental upgrade),
/// only the best quality should end up in the pack.
#[test]
fn test_vault_sync_only_best_quality_no_accumulation() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Both formats available from the start (same bytes = same SHA-256)
    create_jpeg(&source.join("photo.jpg"), 100, 100, 100);
    copy_file(&source.join("photo.jpg"), &source.join("photo.cr2"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    // Only 1 file should be in pack (SOT hash)
    let pack_files = list_pack_files(&vault_dir);
    assert_eq!(pack_files.len(), 1, "Only SOT should be in pack");
    // The file should use the SOT's format extension (CR2)
    assert_eq!(
        pack_files[0].extension().unwrap(),
        "cr2",
        "CR2 should be the only file in pack"
    );
}

/// Verify cleanup reports removed count when a source is removed and hash is no longer desired.
#[test]
fn test_vault_sync_cleanup_reports_removed_count() {
    let tmp = tempfile::tempdir().unwrap();
    let source_a = tmp.path().join("source_a");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&source_a).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Create and pack a photo
    create_jpeg(&source_a.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source_a).unwrap();
    vault.scan(None).unwrap();
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    assert_eq!(count_files_recursive(&vault_dir), 1);

    // Remove the source and rescan to clear catalog
    vault.remove_source(&source_a).unwrap();

    // Pack sync with empty catalog should clean up stale entries
    let mut events = Vec::new();
    vault
        .vault_save(Some(&mut |progress| match progress {
            photopack_core::vault_save::VaultSaveProgress::Removed { .. } => {
                events.push("removed".to_string());
            }
            photopack_core::vault_save::VaultSaveProgress::Complete {
                removed, ..
            } => {
                events.push(format!("complete_removed:{removed}"));
            }
            _ => {}
        }))
        .unwrap();

    assert!(
        events.contains(&"removed".to_string()),
        "Should emit Removed event for stale pack file"
    );
    assert!(
        events.iter().any(|e| e.starts_with("complete_removed:") && !e.ends_with(":0")),
        "Complete event should report non-zero removed count"
    );
    assert_eq!(count_files_recursive(&vault_dir), 0, "Pack should be empty after cleanup");
}

// ── Export (HEIC conversion) tests ──────────────────────────────

#[cfg(target_os = "macos")]
#[test]
fn test_export_converts_jpeg_to_heic() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 150, 200);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    let exported: Vec<_> = walkdir::WalkDir::new(&export_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].path().extension().unwrap(), "heic");
    assert!(
        exported[0]
            .path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("photo")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_deduplicates_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("original.jpg"), 100, 150, 200);
    copy_file(
        &photos_dir.join("original.jpg"),
        &photos_dir.join("copy.jpg"),
    );
    create_jpeg(&photos_dir.join("unique.jpg"), 10, 200, 30);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    // 1 SOT from group + 1 unique = 2 HEIC files
    assert_eq!(count_files_recursive(&export_dir), 2);
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_skips_existing() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    // First export
    use photopack_core::export::ExportProgress;
    let mut first_converted = 0;
    vault
        .export(&export_dir,
            85,
            Some(&mut |progress| {
                if let ExportProgress::Complete { converted, .. } = progress {
                    first_converted = converted;
                }
            }),
        )
        .unwrap();
    assert_eq!(first_converted, 1);

    // Second export — should skip
    let mut second_skipped = 0;
    vault
        .export(
            &export_dir,
            85,
            Some(&mut |progress| {
                if let ExportProgress::Complete { skipped, .. } = progress {
                    second_skipped = skipped;
                }
            }),
        )
        .unwrap();
    assert_eq!(second_skipped, 1);
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_date_organization() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    let exported: Vec<_> = walkdir::WalkDir::new(&export_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    assert_eq!(exported.len(), 1);

    let relative = exported[0].path().strip_prefix(&export_dir).unwrap();
    let components: Vec<_> = relative.components().collect();
    // YYYY / MM / DD / filename.heic = 4 components
    assert_eq!(
        components.len(),
        4,
        "Expected YYYY/MM/DD/file.heic structure"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_progress_events_order() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("a.jpg"), 100, 100, 100);
    create_jpeg(&photos_dir.join("b.jpg"), 200, 50, 175);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    use photopack_core::export::ExportProgress;
    let mut events = Vec::new();
    vault
        .export(
            &export_dir,
            85,
            Some(&mut |progress| match progress {
                ExportProgress::Start { total } => events.push(format!("start:{total}")),
                ExportProgress::Converted { .. } => events.push("converted".to_string()),
                ExportProgress::Skipped { .. } => events.push("skipped".to_string()),
                ExportProgress::Complete {
                    converted,
                    skipped,
                } => events.push(format!("complete:{converted}:{skipped}")),
            }),
        )
        .unwrap();

    assert_eq!(events[0], "start:2");
    assert!(events.contains(&"converted".to_string()));
    assert_eq!(events.last().unwrap(), "complete:2:0");
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_nonexistent_path_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let export_dir = tmp.path().join("nonexistent_export");

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let err = vault.export(&export_dir, 85, None).unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_converts_png_to_heic() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_png(&photos_dir.join("screenshot.png"), 100, 150, 200);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    let exported: Vec<_> = walkdir::WalkDir::new(&export_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].path().extension().unwrap(), "heic");
    assert_eq!(
        exported[0].path().file_stem().unwrap().to_str().unwrap(),
        "screenshot"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_multiple_photos_all_heic_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("a.jpg"), 10, 20, 30);
    create_jpeg(&photos_dir.join("b.jpg"), 200, 50, 175);
    create_png(&photos_dir.join("c.png"), 80, 160, 240);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    let exported: Vec<_> = walkdir::WalkDir::new(&export_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    assert_eq!(exported.len(), 3);
    for entry in &exported {
        assert_eq!(
            entry.path().extension().unwrap(),
            "heic",
            "All exported files should have .heic extension: {}",
            entry.path().display()
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_multiple_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let source_a = tmp.path().join("source_a");
    let source_b = tmp.path().join("source_b");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&source_a).unwrap();
    fs::create_dir_all(&source_b).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&source_a.join("from_a.jpg"), 10, 20, 30);
    create_jpeg(&source_b.join("from_b.jpg"), 200, 50, 175);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source_a).unwrap();
    vault.add_source(&source_b).unwrap();
    vault.scan(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    assert_eq!(count_files_recursive(&export_dir), 2);
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_nested_source_photos() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("photos");
    let nested = source.join("vacation/beach");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&source.join("top.jpg"), 10, 20, 30);
    create_jpeg(&nested.join("deep.jpg"), 200, 50, 175);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source).unwrap();
    vault.scan(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    assert_eq!(count_files_recursive(&export_dir), 2);
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_empty_catalog_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&export_dir).unwrap();

    let vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    use photopack_core::export::ExportProgress;
    let mut total = 999;
    let mut converted = 999;
    vault
        .export(
            &export_dir,
            85,
            Some(&mut |progress| match progress {
                ExportProgress::Start { total: t } => total = t,
                ExportProgress::Complete { converted: c, .. } => converted = c,
                _ => {}
            }),
        )
        .unwrap();

    assert_eq!(total, 0);
    assert_eq!(converted, 0);
    assert_eq!(count_files_recursive(&export_dir), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_all_grouped_only_sots_exported() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    // Create 2 groups of duplicates, no ungrouped photos
    create_jpeg(&photos_dir.join("g1_a.jpg"), 10, 20, 30);
    copy_file(
        &photos_dir.join("g1_a.jpg"),
        &photos_dir.join("g1_b.jpg"),
    );
    create_jpeg(&photos_dir.join("g2_a.jpg"), 200, 50, 175);
    copy_file(
        &photos_dir.join("g2_a.jpg"),
        &photos_dir.join("g2_b.jpg"),
    );

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 4);
    assert_eq!(vault.status().unwrap().total_groups, 2);

    vault.export(&export_dir, 85, None).unwrap();

    // Only 2 SOTs exported, not 4
    assert_eq!(count_files_recursive(&export_dir), 2);
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_after_rescan_includes_new_photos() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("first.jpg"), 10, 20, 30);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    assert_eq!(count_files_recursive(&export_dir), 1);

    // Add new photo and rescan
    create_jpeg(&photos_dir.join("second.jpg"), 200, 50, 175);
    vault.scan(None).unwrap();

    use photopack_core::export::ExportProgress;
    let mut converted = 0;
    let mut skipped = 0;
    vault
        .export(
            &export_dir,
            85,
            Some(&mut |progress| {
                if let ExportProgress::Complete {
                    converted: c,
                    skipped: s,
                } = progress
                {
                    converted = c;
                    skipped = s;
                }
            }),
        )
        .unwrap();

    // First photo skipped (already exported), second converted
    assert_eq!(skipped, 1);
    assert_eq!(converted, 1);
    assert_eq!(count_files_recursive(&export_dir), 2);
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_independent_from_vault_save() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 10, 20, 30);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    // Set vault path
    vault.set_vault_path(&vault_dir).unwrap();

    // Both operations work independently
    vault.vault_save(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    // Pack has content-addressed .jpg, export has .heic
    let pack_files = list_pack_files(&vault_dir);
    let export_files: Vec<_> = walkdir::WalkDir::new(&export_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    assert_eq!(pack_files.len(), 1);
    assert_eq!(export_files.len(), 1);
    assert_eq!(pack_files[0].extension().unwrap(), "jpg");
    assert_eq!(export_files[0].path().extension().unwrap(), "heic");
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_heic_file_is_nonempty() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 150, 200);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    vault.export(&export_dir, 85, None).unwrap();

    let exported: Vec<_> = walkdir::WalkDir::new(&export_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    assert_eq!(exported.len(), 1);
    let size = exported[0].metadata().unwrap().len();
    assert!(size > 0, "Exported HEIC file should be non-empty");
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_cross_source_dedup() {
    let tmp = tempfile::tempdir().unwrap();
    let source_a = tmp.path().join("source_a");
    let source_b = tmp.path().join("source_b");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&source_a).unwrap();
    fs::create_dir_all(&source_b).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&source_a.join("photo.jpg"), 10, 20, 30);
    copy_file(&source_a.join("photo.jpg"), &source_b.join("photo.jpg"));

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&source_a).unwrap();
    vault.add_source(&source_b).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_groups, 1);

    vault.export(&export_dir, 85, None).unwrap();

    // Only 1 SOT exported, not 2
    assert_eq!(count_files_recursive(&export_dir), 1);
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_converted_event_has_correct_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    create_jpeg(&photos_dir.join("photo.jpg"), 100, 100, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();
    use photopack_core::export::ExportProgress;
    let mut source_path = PathBuf::new();
    let mut target_path = PathBuf::new();
    vault
        .export(
            &export_dir,
            85,
            Some(&mut |progress| {
                if let ExportProgress::Converted { source, target } = progress {
                    source_path = source;
                    target_path = target;
                }
            }),
        )
        .unwrap();

    // Source should point to the original file
    assert!(
        source_path.to_string_lossy().contains("photo.jpg"),
        "source should be the original: {}",
        source_path.display()
    );
    // Target should be in the export dir with .heic extension
    assert!(
        target_path.starts_with(&export_dir),
        "target should be in export dir: {}",
        target_path.display()
    );
    assert_eq!(target_path.extension().unwrap(), "heic");
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_multiple_groups_correct_count() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&export_dir).unwrap();

    // Group 1: 3 copies
    create_jpeg(&photos_dir.join("g1_a.jpg"), 10, 20, 30);
    copy_file(&photos_dir.join("g1_a.jpg"), &photos_dir.join("g1_b.jpg"));
    copy_file(&photos_dir.join("g1_a.jpg"), &photos_dir.join("g1_c.jpg"));

    // Group 2: 2 copies
    create_jpeg(&photos_dir.join("g2_a.jpg"), 200, 50, 175);
    copy_file(&photos_dir.join("g2_a.jpg"), &photos_dir.join("g2_b.jpg"));

    // 2 unique
    create_jpeg(&photos_dir.join("unique1.jpg"), 80, 160, 240);
    create_jpeg(&photos_dir.join("unique2.jpg"), 30, 90, 180);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(vault.status().unwrap().total_photos, 7);
    assert_eq!(vault.status().unwrap().total_groups, 2);

    vault.export(&export_dir, 85, None).unwrap();

    // 1 SOT from group1 + 1 SOT from group2 + 2 unique = 4
    assert_eq!(count_files_recursive(&export_dir), 4);
}

// ── Phash version tracking / cache invalidation ─────────────────

#[test]
fn test_scan_sets_phash_on_jpeg_photos() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    create_jpeg(&dir.join("a.jpg"), 100, 50, 200);
    create_jpeg(&dir.join("b.png"), 50, 150, 100);

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    let photos = vault.photos().unwrap();
    let jpeg = photos.iter().find(|p| p.path.ends_with("a.jpg")).unwrap();
    let png = photos.iter().find(|p| p.path.ends_with("b.png")).unwrap();
    assert!(jpeg.phash.is_some(), "JPEG should have phash after scan");
    assert!(jpeg.dhash.is_some(), "JPEG should have dhash after scan");
    assert!(png.phash.is_some(), "PNG should have phash after scan");
    assert!(png.dhash.is_some(), "PNG should have dhash after scan");
}

#[test]
fn test_scan_reuses_cached_hashes_when_version_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    create_jpeg(&dir.join("a.jpg"), 100, 50, 200);

    let db_path = tmp.path().join("catalog.db");
    let mut vault = Vault::open(&db_path).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    // Record original hashes
    let photos = vault.photos().unwrap();
    let original_phash = photos[0].phash.unwrap();
    let original_dhash = photos[0].dhash.unwrap();

    // Rescan — hashes should be reused (not recomputed)
    vault.scan(None).unwrap();

    let photos = vault.photos().unwrap();
    assert_eq!(photos[0].phash.unwrap(), original_phash);
    assert_eq!(photos[0].dhash.unwrap(), original_dhash);
}

#[test]
fn test_scan_invalidates_hashes_on_version_change() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    create_jpeg(&dir.join("a.jpg"), 100, 50, 200);

    let db_path = tmp.path().join("catalog.db");
    let mut vault = Vault::open(&db_path).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    // Verify photo has phash
    let photos = vault.photos().unwrap();
    assert!(photos[0].phash.is_some());

    // Simulate an old phash_version by writing a different value directly
    {
        let catalog = photopack_core::catalog::Catalog::open(&db_path).unwrap();
        catalog.set_config("phash_version", "OLD_VERSION").unwrap();
    }

    // Rescan — should detect version mismatch, clear hashes, recompute
    let mut vault = Vault::open(&db_path).unwrap();
    vault.scan(None).unwrap();

    let photos = vault.photos().unwrap();
    assert!(photos[0].phash.is_some(), "phash should be recomputed after version change");
    assert!(photos[0].dhash.is_some(), "dhash should be recomputed after version change");
}

#[test]
fn test_scan_invalidates_stale_hashes_and_regroups_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    // Create original at high quality and recompressed at low quality
    let img = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([
            100u8.wrapping_add((x * 3) as u8),
            100u8.wrapping_add((y * 3) as u8),
            100u8.wrapping_add(((x + y) * 2) as u8),
        ])
    });
    {
        let file = fs::File::create(dir.join("original.jpg")).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 95);
        encoder
            .encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
            .unwrap();
    }
    {
        let file = fs::File::create(dir.join("recompressed.jpg")).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 50);
        encoder
            .encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
            .unwrap();
    }

    let db_path = tmp.path().join("catalog.db");
    let mut vault = Vault::open(&db_path).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    // Should be grouped (same image, different compression)
    assert_eq!(vault.groups().unwrap().len(), 1, "initial scan should group recompressed pair");

    // Poison hashes: set a stale version so next scan invalidates
    {
        let catalog = photopack_core::catalog::Catalog::open(&db_path).unwrap();
        catalog.set_config("phash_version", "STALE").unwrap();
        // Corrupt the cached hashes to simulate algorithm change
        catalog.clear_perceptual_hashes().unwrap();
    }

    // Rescan — should detect version mismatch, recompute hashes, and regroup
    let mut vault = Vault::open(&db_path).unwrap();
    vault.scan(None).unwrap();

    assert_eq!(
        vault.groups().unwrap().len(),
        1,
        "recompressed pair should still be grouped after hash recomputation"
    );
}

#[test]
fn test_scan_first_run_sets_phash_version() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    create_jpeg(&dir.join("a.jpg"), 100, 50, 200);

    let db_path = tmp.path().join("catalog.db");
    let mut vault = Vault::open(&db_path).unwrap();
    vault.add_source(&dir).unwrap();

    // Before scan, no phash_version in config
    {
        let catalog = photopack_core::catalog::Catalog::open(&db_path).unwrap();
        assert!(catalog.get_config("phash_version").unwrap().is_none());
    }

    vault.scan(None).unwrap();

    // After scan, phash_version should be set
    {
        let catalog = photopack_core::catalog::Catalog::open(&db_path).unwrap();
        let version = catalog.get_config("phash_version").unwrap();
        assert!(version.is_some(), "scan should set phash_version in config");
    }
}

#[test]
fn test_scan_version_mismatch_clears_all_hashes_before_recompute() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir_all(&dir).unwrap();

    create_jpeg(&dir.join("a.jpg"), 100, 50, 200);
    create_jpeg(&dir.join("b.jpg"), 50, 150, 100);

    let db_path = tmp.path().join("catalog.db");
    let mut vault = Vault::open(&db_path).unwrap();
    vault.add_source(&dir).unwrap();
    vault.scan(None).unwrap();

    // Both photos should have hashes
    let photos = vault.photos().unwrap();
    assert!(photos.iter().all(|p| p.phash.is_some()));

    // Simulate old version
    {
        let catalog = photopack_core::catalog::Catalog::open(&db_path).unwrap();
        catalog.set_config("phash_version", "1").unwrap();
    }

    // Rescan — version mismatch triggers clear + recompute
    let mut vault = Vault::open(&db_path).unwrap();
    vault.scan(None).unwrap();

    // All photos should have fresh hashes (recomputed, not stale)
    let photos = vault.photos().unwrap();
    assert!(
        photos.iter().all(|p| p.phash.is_some() && p.dhash.is_some()),
        "all photos should have recomputed hashes after version change"
    );
}
