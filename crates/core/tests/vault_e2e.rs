use std::fs;
use std::path::{Path, PathBuf};

use losslessvault_core::Vault;

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
        let mut vault = Vault::open(&db_path).unwrap();
        vault.add_source(&photos_dir).unwrap();
    }

    // Reopen — source should still be there
    let mut vault = Vault::open(&db_path).unwrap();
    let sources = vault.sources().unwrap();
    assert_eq!(sources.len(), 1);
}

// ── Vault::add_source ────────────────────────────────────────────

#[test]
fn test_add_source_valid_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let source = vault.add_source(&photos_dir).unwrap();
    assert_eq!(source.path, photos_dir.canonicalize().unwrap());
}

#[test]
fn test_add_source_nonexistent_path() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let err = vault.add_source(Path::new("/nonexistent/path")).unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn test_add_source_file_not_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("file.txt");
    fs::write(&file_path, b"not a dir").unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let err = vault.add_source(&file_path).unwrap_err();
    assert!(err.to_string().contains("not a directory"));
}

#[test]
fn test_add_source_duplicate_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    fs::create_dir_all(&photos_dir).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.add_source(&photos_dir).unwrap();
    assert!(vault.add_source(&photos_dir).is_err());
}

// ── Vault::status (empty) ────────────────────────────────────────

#[test]
fn test_status_empty_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

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
        losslessvault_core::domain::Confidence::Certain
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
    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

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
                losslessvault_core::ScanProgress::SourceStart { file_count, .. } => {
                    events.push(format!("start:{file_count}"));
                }
                losslessvault_core::ScanProgress::FileProcessed { .. } => {
                    events.push("file".to_string());
                }
                losslessvault_core::ScanProgress::PhaseComplete { phase } => {
                    events.push(format!("phase:{phase}"));
                }
            }
        }))
        .unwrap();

    // Should have: start, 2 files, indexing phase, matching phase
    assert!(events.iter().any(|e| e.starts_with("start:")));
    assert_eq!(events.iter().filter(|e| *e == "file").count(), 2);
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
        losslessvault_core::domain::PhotoFormat::Png,
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
    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

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
        losslessvault_core::domain::PhotoFormat::Cr2,
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
        losslessvault_core::domain::PhotoFormat::Dng,
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
        losslessvault_core::domain::PhotoFormat::Cr2,
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
        losslessvault_core::domain::PhotoFormat::Tiff,
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
        losslessvault_core::domain::PhotoFormat::Png,
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
        losslessvault_core::domain::PhotoFormat::Jpeg,
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
        losslessvault_core::domain::PhotoFormat::Cr2,
        "CR2 must win over HEIC and JPEG"
    );
}

/// Vault save must export only the RAW, not the HEIC.
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

    let files: Vec<_> = walkdir::WalkDir::new(&vault_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    assert_eq!(files.len(), 1, "only SOT should be exported");
    assert_eq!(
        files[0].path().extension().unwrap(),
        "cr2",
        "the exported file must be the RAW (CR2), not the HEIC"
    );
}

/// When the vault directory is also a registered source containing the RAW
/// original, and another source has a lossy copy, vault save should still
/// export correctly (the RAW is already "in" the vault as a source, but gets
/// organized into YYYY/MM/DD).
#[test]
fn test_vault_as_source_preserves_raw_original() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("vault");
    let icloud = tmp.path().join("icloud");
    fs::create_dir_all(&vault_dir).unwrap();
    fs::create_dir_all(&icloud).unwrap();

    // RAW lives in the vault dir itself
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
    assert_eq!(sot.format, losslessvault_core::domain::PhotoFormat::Cr2);

    // Vault save should export the RAW into date-organized structure
    vault.set_vault_path(&vault_dir).unwrap();
    vault.vault_save(None).unwrap();

    let exported: Vec<_> = walkdir::WalkDir::new(&vault_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    // Original RAW at vault/sunset.cr2 + organized copy at vault/YYYY/MM/DD/sunset.cr2
    let cr2_files: Vec<_> = exported
        .iter()
        .filter(|e| e.path().extension().map(|x| x == "cr2").unwrap_or(false))
        .collect();
    assert!(
        cr2_files.len() >= 1,
        "vault must contain at least the CR2 file"
    );
    // No HEIC should appear in the vault
    let heic_files: Vec<_> = exported
        .iter()
        .filter(|e| e.path().extension().map(|x| x == "heic").unwrap_or(false))
        .collect();
    assert_eq!(
        heic_files.len(),
        0,
        "HEIC must NOT be exported — only the RAW is SOT"
    );
}

/// Multiple groups, each with different format combinations. Vault save must
/// export the best quality from EACH group independently.
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

    let files: Vec<_> = walkdir::WalkDir::new(&vault_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    assert_eq!(files.len(), 3, "one SOT per group");

    let extensions: std::collections::HashSet<String> = files
        .iter()
        .filter_map(|f| f.path().extension().map(|e| e.to_string_lossy().to_string()))
        .collect();
    assert!(extensions.contains("cr2"), "RAW must be exported for group 1");
    assert!(
        extensions.contains("tiff"),
        "TIFF must be exported for group 2"
    );
    assert!(extensions.contains("png"), "PNG must be exported for group 3");
}

/// Vault save must preserve the exact file content of the highest-quality version.
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

    let exported: Vec<_> = walkdir::WalkDir::new(&vault_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    assert_eq!(exported.len(), 1);
    let saved_bytes = fs::read(exported[0].path()).unwrap();
    assert_eq!(
        original_bytes, saved_bytes,
        "exported file content must be byte-identical to the RAW original"
    );
}

/// Incremental vault save: when the RAW has already been exported, a second
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
            if let losslessvault_core::vault_save::VaultSaveProgress::Complete {
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
            if let losslessvault_core::vault_save::VaultSaveProgress::Complete {
                copied,
                skipped,
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
            losslessvault_core::domain::PhotoFormat::Jpeg,
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
        .count()
}

#[test]
fn test_vault_set_and_get_path() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("my_vault");
    fs::create_dir_all(&vault_dir).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
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
fn test_vault_save_creates_yyyy_mm_dd_structure() {
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

    let exported_files: Vec<_> = walkdir::WalkDir::new(&vault_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    assert_eq!(exported_files.len(), 1);

    let relative = exported_files[0]
        .path()
        .strip_prefix(&vault_dir)
        .unwrap();
    let components: Vec<_> = relative.components().collect();
    // Should be: YYYY / MM / DD / filename.ext (4 components)
    assert_eq!(
        components.len(),
        4,
        "Path should be vault/YYYY/MM/DD/file.ext, got: {}",
        relative.display()
    );
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
            if let losslessvault_core::vault_save::VaultSaveProgress::Complete {
                copied, ..
            } = progress
            {
                first_copied = copied;
            }
        }))
        .unwrap();
    assert_eq!(first_copied, 1, "First save should copy 1 file");

    // Second save — file already exists with same size, should skip
    let mut second_skipped = 0;
    vault
        .vault_save(Some(&mut |progress| {
            if let losslessvault_core::vault_save::VaultSaveProgress::Complete {
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
    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

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

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
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

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
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
        let mut vault = Vault::open(&db_path).unwrap();
        vault.set_vault_path(&vault_dir).unwrap();
    }

    // Reopen — vault path should persist
    let mut vault = Vault::open(&db_path).unwrap();
    let stored = vault.get_vault_path().unwrap().unwrap();
    assert_eq!(stored, vault_dir.canonicalize().unwrap());
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
                losslessvault_core::vault_save::VaultSaveProgress::Start { total: t } => {
                    total = t;
                }
                losslessvault_core::vault_save::VaultSaveProgress::Complete { copied: c, skipped: s } => {
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

    // Should only save 1 file (the PNG, not both)
    let files: Vec<_> = walkdir::WalkDir::new(&vault_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    assert_eq!(files.len(), 1, "only SoT should be saved");
    assert!(
        files[0].path().extension().unwrap() == "png",
        "PNG should be elected as SoT over JPEG"
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
fn test_vault_save_multiple_photos_same_date_no_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let photos_dir = tmp.path().join("photos");
    let vault_dir = tmp.path().join("vault");
    fs::create_dir_all(&photos_dir).unwrap();
    fs::create_dir_all(&vault_dir).unwrap();

    // Create 3 different photos — they'll share the same mtime-derived date
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

    // All 3 should be saved (different filenames, same date dir)
    assert_eq!(
        count_files_recursive(&vault_dir),
        3,
        "all unique photos should be saved even on same date"
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
                losslessvault_core::vault_save::VaultSaveProgress::Start { total } => {
                    events.push(format!("start:{total}"));
                }
                losslessvault_core::vault_save::VaultSaveProgress::Copied { .. } => {
                    events.push("copied".to_string());
                }
                losslessvault_core::vault_save::VaultSaveProgress::Skipped { .. } => {
                    events.push("skipped".to_string());
                }
                losslessvault_core::vault_save::VaultSaveProgress::Complete {
                    copied,
                    skipped,
                } => {
                    events.push(format!("complete:{copied}:{skipped}"));
                }
            }
        }))
        .unwrap();

    // Should be: start → copied × 2 → complete
    assert_eq!(events[0], "start:2");
    assert_eq!(events.iter().filter(|e| *e == "copied").count(), 2);
    assert!(events.last().unwrap().starts_with("complete:2:0"));
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

    let saved_files: Vec<_> = walkdir::WalkDir::new(&vault_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    assert_eq!(saved_files.len(), 1);

    let saved_bytes = fs::read(saved_files[0].path()).unwrap();
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

// ── Export (HEIC conversion) tests ──────────────────────────────

#[test]
fn test_export_set_and_get_path() {
    let tmp = tempfile::tempdir().unwrap();
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&export_dir).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    // Initially unset
    assert!(vault.get_export_path().unwrap().is_none());

    // Set and verify
    vault.set_export_path(&export_dir).unwrap();
    let retrieved = vault.get_export_path().unwrap().unwrap();
    assert_eq!(retrieved, export_dir.canonicalize().unwrap());
}

#[test]
fn test_export_set_overwrite_path() {
    let tmp = tempfile::tempdir().unwrap();
    let dir1 = tmp.path().join("export1");
    let dir2 = tmp.path().join("export2");
    fs::create_dir_all(&dir1).unwrap();
    fs::create_dir_all(&dir2).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.set_export_path(&dir1).unwrap();
    vault.set_export_path(&dir2).unwrap();

    let retrieved = vault.get_export_path().unwrap().unwrap();
    assert_eq!(retrieved, dir2.canonicalize().unwrap());
}

#[test]
fn test_export_set_nonexistent_path_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let err = vault
        .set_export_path(Path::new("/nonexistent/export/path"))
        .unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn test_export_path_not_set_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let err = vault.export(85, None).unwrap_err();
    // On macOS this hits SipsNotAvailable or ExportPathNotSet depending on order
    // On non-macOS it hits SipsNotAvailable first
    assert!(
        err.to_string().contains("export path not configured")
            || err.to_string().contains("sips")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_path_not_set_errors_macos() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();

    let err = vault.export(85, None).unwrap_err();
    assert!(err.to_string().contains("export path not configured"));
}

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
    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

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
    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

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
    vault.set_export_path(&export_dir).unwrap();

    // First export
    use losslessvault_core::export::ExportProgress;
    let mut first_converted = 0;
    vault
        .export(
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
    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

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
    vault.set_export_path(&export_dir).unwrap();

    use losslessvault_core::export::ExportProgress;
    let mut events = Vec::new();
    vault
        .export(
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
fn test_export_deleted_export_path_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&export_dir).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.set_export_path(&export_dir).unwrap();

    // Delete the export directory after setting it
    fs::remove_dir_all(&export_dir).unwrap();

    let err = vault.export(85, None).unwrap_err();
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
    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

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
    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

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
    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

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
    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

    assert_eq!(count_files_recursive(&export_dir), 2);
}

#[cfg(target_os = "macos")]
#[test]
fn test_export_empty_catalog_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&export_dir).unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    vault.set_export_path(&export_dir).unwrap();

    use losslessvault_core::export::ExportProgress;
    let mut total = 999;
    let mut converted = 999;
    vault
        .export(
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

    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

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
    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

    assert_eq!(count_files_recursive(&export_dir), 1);

    // Add new photo and rescan
    create_jpeg(&photos_dir.join("second.jpg"), 200, 50, 175);
    vault.scan(None).unwrap();

    use losslessvault_core::export::ExportProgress;
    let mut converted = 0;
    let mut skipped = 0;
    vault
        .export(
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

    // Set both paths
    vault.set_vault_path(&vault_dir).unwrap();
    vault.set_export_path(&export_dir).unwrap();

    // Both operations work independently
    vault.vault_save(None).unwrap();
    vault.export(85, None).unwrap();

    // Vault has original .jpg, export has .heic
    let vault_files: Vec<_> = walkdir::WalkDir::new(&vault_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    let export_files: Vec<_> = walkdir::WalkDir::new(&export_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    assert_eq!(vault_files.len(), 1);
    assert_eq!(export_files.len(), 1);
    assert_eq!(vault_files[0].path().extension().unwrap(), "jpg");
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
    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

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

    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

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
    vault.set_export_path(&export_dir).unwrap();

    use losslessvault_core::export::ExportProgress;
    let mut source_path = PathBuf::new();
    let mut target_path = PathBuf::new();
    vault
        .export(
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
    let canonical_export = export_dir.canonicalize().unwrap();
    assert!(
        target_path.starts_with(&canonical_export),
        "target should be in export dir: {}",
        target_path.display()
    );
    assert_eq!(target_path.extension().unwrap(), "heic");
}

#[test]
fn test_export_set_file_not_directory_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("not_a_dir.txt");
    fs::write(&file_path, b"i am a file").unwrap();

    let mut vault = Vault::open(&tmp.path().join("catalog.db")).unwrap();
    let err = vault.set_export_path(&file_path).unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn test_export_path_persists_across_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let export_dir = tmp.path().join("export");
    fs::create_dir_all(&export_dir).unwrap();
    let db_path = tmp.path().join("catalog.db");

    {
        let mut vault = Vault::open(&db_path).unwrap();
        vault.set_export_path(&export_dir).unwrap();
    }

    // Reopen vault and verify path persisted
    let mut vault = Vault::open(&db_path).unwrap();
    let retrieved = vault.get_export_path().unwrap().unwrap();
    assert_eq!(retrieved, export_dir.canonicalize().unwrap());
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

    vault.set_export_path(&export_dir).unwrap();
    vault.export(85, None).unwrap();

    // 1 SOT from group1 + 1 SOT from group2 + 2 unique = 4
    assert_eq!(count_files_recursive(&export_dir), 4);
}
