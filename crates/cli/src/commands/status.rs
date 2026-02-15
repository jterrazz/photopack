use std::collections::{HashMap, HashSet};

use anyhow::Result;
use comfy_table::{presets::UTF8_FULL, Cell, Color, ContentArrangement, Table};
use photopack_core::domain::{DuplicateGroup, PhotoFile, Source};
use photopack_core::Vault;

/// Precomputed lookup data for rendering the status dashboard.
pub(crate) struct StatusData {
    /// photo_id → group_id
    pub(crate) photo_group: HashMap<i64, i64>,
    /// photo_id → true if source-of-truth
    pub(crate) photo_is_sot: HashMap<i64, bool>,
    /// Set of all photo IDs that belong to a group
    pub(crate) grouped_ids: HashSet<i64>,
}

/// Aggregated statistics derived from photos and groups.
#[derive(Debug, PartialEq)]
pub(crate) struct Aggregates {
    pub(crate) total_photos: usize,
    pub(crate) total_groups: usize,
    pub(crate) total_duplicates: usize,
    pub(crate) total_unique: usize,
    pub(crate) total_disk: u64,
    pub(crate) savings: u64,
}

/// Per-source statistics.
#[derive(Debug, PartialEq)]
pub(crate) struct SourceStats {
    pub(crate) photo_count: usize,
    pub(crate) total_size: u64,
}

impl StatusData {
    pub(crate) fn build(groups: &[DuplicateGroup]) -> Self {
        let mut photo_group: HashMap<i64, i64> = HashMap::new();
        let mut photo_is_sot: HashMap<i64, bool> = HashMap::new();
        let mut grouped_ids: HashSet<i64> = HashSet::new();

        for group in groups {
            for member in &group.members {
                photo_group.insert(member.id, group.id);
                photo_is_sot.insert(member.id, member.id == group.source_of_truth_id);
                grouped_ids.insert(member.id);
            }
        }

        Self {
            photo_group,
            photo_is_sot,
            grouped_ids,
        }
    }

    pub(crate) fn is_duplicate(&self, photo_id: i64) -> bool {
        self.grouped_ids.contains(&photo_id)
            && !self.photo_is_sot.get(&photo_id).copied().unwrap_or(false)
    }

    pub(crate) fn vault_eligible(&self, photo_id: i64) -> bool {
        if self.grouped_ids.contains(&photo_id) {
            self.photo_is_sot.get(&photo_id).copied().unwrap_or(false)
        } else {
            true
        }
    }
}

pub(crate) fn compute_aggregates(photos: &[PhotoFile], groups: &[DuplicateGroup], data: &StatusData) -> Aggregates {
    let total_photos = photos.len();
    let total_groups = groups.len();
    let total_duplicates = photos.iter().filter(|p| data.is_duplicate(p.id)).count();
    let total_unique = total_photos - total_duplicates;
    let total_disk: u64 = photos.iter().map(|p| p.size).sum();
    let savings: u64 = photos
        .iter()
        .filter(|p| data.is_duplicate(p.id))
        .map(|p| p.size)
        .sum();

    Aggregates {
        total_photos,
        total_groups,
        total_duplicates,
        total_unique,
        total_disk,
        savings,
    }
}

pub(crate) fn compute_source_stats(photos: &[PhotoFile]) -> HashMap<i64, SourceStats> {
    let mut stats: HashMap<i64, SourceStats> = HashMap::new();
    for photo in photos {
        let entry = stats.entry(photo.source_id).or_insert(SourceStats {
            photo_count: 0,
            total_size: 0,
        });
        entry.photo_count += 1;
        entry.total_size += photo.size;
    }
    stats
}

pub fn run(vault: &Vault) -> Result<()> {
    let sources = vault.sources()?;
    let photos = vault.photos()?;
    let groups = vault.groups()?;
    let vault_path = vault.get_vault_path()?;

    let data = StatusData::build(&groups);
    let agg = compute_aggregates(&photos, &groups, &data);
    let source_stats = compute_source_stats(&photos);

    let vault_display = match &vault_path {
        Some(p) => p.display().to_string(),
        None => "not configured".to_string(),
    };

    // Overview
    println!();
    println!("  Photopack Status");
    println!("  ====================");
    println!();
    println!("  Overview");
    println!("  --------");
    println!(
        "   Photos:     {:>8}        Disk Usage:  {}",
        agg.total_photos,
        format_size(agg.total_disk)
    );
    println!(
        "   Unique:     {:>8}        Savings:     {}",
        agg.total_unique,
        format_size(agg.savings)
    );
    println!(
        "   Groups:     {:>8}        Sources:     {:>8}",
        agg.total_groups,
        sources.len()
    );
    println!(
        "   Duplicates: {:>8}        Vault:       {}",
        agg.total_duplicates, vault_display
    );

    // Sources table
    let mut sources_table = Table::new();
    sources_table.load_preset(UTF8_FULL);
    sources_table.set_content_arrangement(ContentArrangement::Dynamic);
    sources_table.set_header(vec![
        Cell::new("ID"),
        Cell::new("Name"),
        Cell::new("Photos"),
        Cell::new("Size"),
        Cell::new("Last Scanned"),
    ]);

    for source in &sources {
        let name = source_display_name(source);
        let ss = source_stats.get(&source.id);
        let count = ss.map(|s| s.photo_count).unwrap_or(0);
        let size = ss.map(|s| s.total_size).unwrap_or(0);
        let scanned = match source.last_scanned {
            Some(ts) => chrono::DateTime::from_timestamp(ts, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            None => "never".to_string(),
        };
        sources_table.add_row(vec![
            Cell::new(source.id),
            Cell::new(&name),
            Cell::new(count),
            Cell::new(format_size(size)),
            Cell::new(scanned),
        ]);
    }

    println!();
    println!("  Sources");
    println!("  -------");
    println!("{sources_table}");

    println!();
    println!("  Run 'photopack ls' to show the full files table.");
    println!();

    Ok(())
}

/// Sort photos for display: grouped first (by group ID, SOT first), then ungrouped (by path).
pub(crate) fn sort_photos_for_display<'a>(
    photos: &'a [PhotoFile],
    data: &StatusData,
) -> (Vec<&'a PhotoFile>, Vec<&'a PhotoFile>) {
    let mut grouped: Vec<&PhotoFile> = Vec::new();
    let mut ungrouped: Vec<&PhotoFile> = Vec::new();

    for photo in photos {
        if data.grouped_ids.contains(&photo.id) {
            grouped.push(photo);
        } else {
            ungrouped.push(photo);
        }
    }

    grouped.sort_by(|a, b| {
        let ga = data.photo_group.get(&a.id).unwrap();
        let gb = data.photo_group.get(&b.id).unwrap();
        ga.cmp(gb)
            .then_with(|| {
                let a_sot = data.photo_is_sot.get(&a.id).copied().unwrap_or(false);
                let b_sot = data.photo_is_sot.get(&b.id).copied().unwrap_or(false);
                b_sot.cmp(&a_sot)
            })
            .then_with(|| a.path.cmp(&b.path))
    });

    ungrouped.sort_by(|a, b| a.path.cmp(&b.path));

    (grouped, ungrouped)
}

pub(crate) fn add_photo_row(
    table: &mut Table,
    photo: &PhotoFile,
    source_name_map: &HashMap<i64, String>,
    data: &StatusData,
) {
    let filename = photo
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| photo.path.display().to_string());

    let source_name = source_name_map
        .get(&photo.source_id)
        .cloned()
        .unwrap_or_else(|| "?".to_string());

    let mut row: Vec<Cell> = vec![
        Cell::new(&filename),
        Cell::new(&source_name),
        Cell::new(photo.format.as_str()),
        Cell::new(format_size(photo.size)),
    ];

    // Group column
    if let Some(gid) = data.photo_group.get(&photo.id) {
        row.push(Cell::new(gid).fg(Color::Cyan));
    } else {
        row.push(Cell::new("\u{2014}").fg(Color::DarkGrey));
    }

    // Role column
    let is_sot = data.photo_is_sot.get(&photo.id).copied().unwrap_or(false);
    let is_grouped = data.grouped_ids.contains(&photo.id);

    if is_grouped && is_sot {
        row.push(Cell::new("Best Copy").fg(Color::Green));
    } else if is_grouped {
        row.push(Cell::new("Duplicate").fg(Color::Yellow));
    } else {
        row.push(Cell::new("Unique"));
    }

    // Vault column
    if data.vault_eligible(photo.id) {
        row.push(Cell::new("\u{2714}").fg(Color::Green));
    } else {
        row.push(Cell::new(""));
    }

    table.add_row(row);
}

pub(crate) fn source_display_name(source: &Source) -> String {
    source
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| source.path.display().to_string())
}

pub(crate) fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    match bytes {
        b if b >= GB => format!("{:.1} GB", b as f64 / GB as f64),
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.1} KB", b as f64 / KB as f64),
        b => format!("{} B", b),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use photopack_core::domain::{Confidence, PhotoFormat};

    // ── format_size ─────────────────────────────────────────────────

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn test_format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn test_format_size_megabytes() {
        assert_eq!(format_size(1_048_576), "1.0 MB");
        assert_eq!(format_size(1_500_000), "1.4 MB");
    }

    #[test]
    fn test_format_size_gigabytes() {
        assert_eq!(format_size(1_073_741_824), "1.0 GB");
        assert_eq!(format_size(2_500_000_000), "2.3 GB");
    }

    #[test]
    fn test_format_size_large_value() {
        // 1 TB displayed as GB
        assert_eq!(format_size(1_099_511_627_776), "1024.0 GB");
    }

    // ── source_display_name ─────────────────────────────────────────

    #[test]
    fn test_source_display_name_normal_path() {
        let source = Source {
            id: 1,
            path: PathBuf::from("/home/user/photos"),
            last_scanned: None,
        };
        assert_eq!(source_display_name(&source), "photos");
    }

    #[test]
    fn test_source_display_name_nested_path() {
        let source = Source {
            id: 1,
            path: PathBuf::from("/mnt/external/camera/2024"),
            last_scanned: None,
        };
        assert_eq!(source_display_name(&source), "2024");
    }

    #[test]
    fn test_source_display_name_root_path() {
        let source = Source {
            id: 1,
            path: PathBuf::from("/"),
            last_scanned: None,
        };
        // Root has no file_name(), falls back to display()
        assert_eq!(source_display_name(&source), "/");
    }

    // ── Helper to build PhotoFile for tests ─────────────────────────

    fn make_photo(id: i64, source_id: i64, path: &str, size: u64) -> PhotoFile {
        PhotoFile {
            id,
            source_id,
            path: PathBuf::from(path),
            size,
            format: PhotoFormat::Jpeg,
            sha256: format!("sha_{id}"),
            phash: None,
            dhash: None,
            exif: None,
            mtime: 1000 + id,
        }
    }

    fn make_group(id: i64, sot_id: i64, member_ids: &[i64]) -> DuplicateGroup {
        DuplicateGroup {
            id,
            source_of_truth_id: sot_id,
            confidence: Confidence::Certain,
            members: member_ids
                .iter()
                .map(|&mid| make_photo(mid, 1, &format!("/photos/{mid}.jpg"), 1000))
                .collect(),
        }
    }

    // ── StatusData ──────────────────────────────────────────────────

    #[test]
    fn test_status_data_empty_groups() {
        let data = StatusData::build(&[]);
        assert!(data.photo_group.is_empty());
        assert!(data.photo_is_sot.is_empty());
        assert!(data.grouped_ids.is_empty());
    }

    #[test]
    fn test_status_data_single_group() {
        let groups = vec![make_group(1, 10, &[10, 11, 12])];
        let data = StatusData::build(&groups);

        assert_eq!(data.photo_group.get(&10), Some(&1));
        assert_eq!(data.photo_group.get(&11), Some(&1));
        assert_eq!(data.photo_group.get(&12), Some(&1));
        assert_eq!(data.photo_is_sot.get(&10), Some(&true));
        assert_eq!(data.photo_is_sot.get(&11), Some(&false));
        assert_eq!(data.photo_is_sot.get(&12), Some(&false));
        assert_eq!(data.grouped_ids.len(), 3);
    }

    #[test]
    fn test_status_data_multiple_groups() {
        let groups = vec![
            make_group(1, 10, &[10, 11]),
            make_group(2, 20, &[20, 21]),
        ];
        let data = StatusData::build(&groups);

        assert_eq!(data.photo_group.get(&10), Some(&1));
        assert_eq!(data.photo_group.get(&20), Some(&2));
        assert_eq!(data.grouped_ids.len(), 4);
    }

    // ── is_duplicate ────────────────────────────────────────────────

    #[test]
    fn test_is_duplicate_sot_is_not_duplicate() {
        let groups = vec![make_group(1, 10, &[10, 11])];
        let data = StatusData::build(&groups);

        assert!(!data.is_duplicate(10)); // SOT
        assert!(data.is_duplicate(11));  // duplicate
    }

    #[test]
    fn test_is_duplicate_ungrouped_is_not_duplicate() {
        let data = StatusData::build(&[]);
        assert!(!data.is_duplicate(99));
    }

    // ── vault_eligible ──────────────────────────────────────────────

    #[test]
    fn test_vault_eligible_sot_is_eligible() {
        let groups = vec![make_group(1, 10, &[10, 11])];
        let data = StatusData::build(&groups);

        assert!(data.vault_eligible(10));  // SOT → eligible
    }

    #[test]
    fn test_vault_eligible_duplicate_not_eligible() {
        let groups = vec![make_group(1, 10, &[10, 11])];
        let data = StatusData::build(&groups);

        assert!(!data.vault_eligible(11)); // duplicate → not eligible
    }

    #[test]
    fn test_vault_eligible_ungrouped_is_eligible() {
        let data = StatusData::build(&[]);
        assert!(data.vault_eligible(99)); // ungrouped → eligible
    }

    // ── compute_aggregates ──────────────────────────────────────────

    #[test]
    fn test_aggregates_empty() {
        let data = StatusData::build(&[]);
        let agg = compute_aggregates(&[], &[], &data);

        assert_eq!(agg, Aggregates {
            total_photos: 0,
            total_groups: 0,
            total_duplicates: 0,
            total_unique: 0,
            total_disk: 0,
            savings: 0,
        });
    }

    #[test]
    fn test_aggregates_all_unique() {
        let photos = vec![
            make_photo(1, 1, "/a.jpg", 1000),
            make_photo(2, 1, "/b.jpg", 2000),
            make_photo(3, 1, "/c.jpg", 3000),
        ];
        let data = StatusData::build(&[]);
        let agg = compute_aggregates(&photos, &[], &data);

        assert_eq!(agg, Aggregates {
            total_photos: 3,
            total_groups: 0,
            total_duplicates: 0,
            total_unique: 3,
            total_disk: 6000,
            savings: 0,
        });
    }

    #[test]
    fn test_aggregates_with_duplicates() {
        let photos = vec![
            make_photo(10, 1, "/a.jpg", 5000),  // SOT
            make_photo(11, 1, "/b.jpg", 3000),  // duplicate
            make_photo(12, 1, "/c.jpg", 4000),  // duplicate
            make_photo(20, 1, "/d.jpg", 2000),  // unique
        ];
        let groups = vec![make_group(1, 10, &[10, 11, 12])];
        let data = StatusData::build(&groups);
        let agg = compute_aggregates(&photos, &groups, &data);

        assert_eq!(agg, Aggregates {
            total_photos: 4,
            total_groups: 1,
            total_duplicates: 2,
            total_unique: 2, // SOT(10) + unique(20)
            total_disk: 14000,
            savings: 7000, // 3000 + 4000 (duplicate sizes)
        });
    }

    #[test]
    fn test_aggregates_multiple_groups() {
        let photos = vec![
            make_photo(10, 1, "/a.jpg", 1000),
            make_photo(11, 1, "/b.jpg", 1000),
            make_photo(20, 1, "/c.jpg", 2000),
            make_photo(21, 1, "/d.jpg", 2000),
        ];
        let groups = vec![
            make_group(1, 10, &[10, 11]),
            make_group(2, 20, &[20, 21]),
        ];
        let data = StatusData::build(&groups);
        let agg = compute_aggregates(&photos, &groups, &data);

        assert_eq!(agg, Aggregates {
            total_photos: 4,
            total_groups: 2,
            total_duplicates: 2,
            total_unique: 2,
            total_disk: 6000,
            savings: 3000, // 1000 + 2000
        });
    }

    #[test]
    fn test_aggregates_all_duplicates_except_sot() {
        // Edge case: every photo is in a group
        let photos = vec![
            make_photo(10, 1, "/a.jpg", 5000),
            make_photo(11, 1, "/b.jpg", 5000),
        ];
        let groups = vec![make_group(1, 10, &[10, 11])];
        let data = StatusData::build(&groups);
        let agg = compute_aggregates(&photos, &groups, &data);

        assert_eq!(agg.total_duplicates, 1);
        assert_eq!(agg.total_unique, 1); // just the SOT
        assert_eq!(agg.savings, 5000);
    }

    // ── compute_source_stats ────────────────────────────────────────

    #[test]
    fn test_source_stats_empty() {
        let stats = compute_source_stats(&[]);
        assert!(stats.is_empty());
    }

    #[test]
    fn test_source_stats_single_source() {
        let photos = vec![
            make_photo(1, 1, "/a.jpg", 1000),
            make_photo(2, 1, "/b.jpg", 2000),
        ];
        let stats = compute_source_stats(&photos);

        assert_eq!(stats.len(), 1);
        assert_eq!(stats[&1].photo_count, 2);
        assert_eq!(stats[&1].total_size, 3000);
    }

    #[test]
    fn test_source_stats_multiple_sources() {
        let photos = vec![
            make_photo(1, 1, "/a.jpg", 1000),
            make_photo(2, 1, "/b.jpg", 2000),
            make_photo(3, 2, "/c.jpg", 5000),
        ];
        let stats = compute_source_stats(&photos);

        assert_eq!(stats.len(), 2);
        assert_eq!(stats[&1].photo_count, 2);
        assert_eq!(stats[&1].total_size, 3000);
        assert_eq!(stats[&2].photo_count, 1);
        assert_eq!(stats[&2].total_size, 5000);
    }

    // ── sort_photos_for_display ─────────────────────────────────────

    #[test]
    fn test_sort_all_ungrouped() {
        let photos = vec![
            make_photo(1, 1, "/z.jpg", 100),
            make_photo(2, 1, "/a.jpg", 200),
            make_photo(3, 1, "/m.jpg", 300),
        ];
        let data = StatusData::build(&[]);
        let (grouped, ungrouped) = sort_photos_for_display(&photos, &data);

        assert!(grouped.is_empty());
        assert_eq!(ungrouped.len(), 3);
        // Sorted by path
        assert_eq!(ungrouped[0].path, PathBuf::from("/a.jpg"));
        assert_eq!(ungrouped[1].path, PathBuf::from("/m.jpg"));
        assert_eq!(ungrouped[2].path, PathBuf::from("/z.jpg"));
    }

    #[test]
    fn test_sort_grouped_sot_first() {
        let photos = vec![
            make_photo(11, 1, "/dup.jpg", 100),
            make_photo(10, 1, "/sot.jpg", 200),
        ];
        let groups = vec![make_group(1, 10, &[10, 11])];
        let data = StatusData::build(&groups);
        let (grouped, ungrouped) = sort_photos_for_display(&photos, &data);

        assert!(ungrouped.is_empty());
        assert_eq!(grouped.len(), 2);
        // SOT (id=10) should come first
        assert_eq!(grouped[0].id, 10);
        assert_eq!(grouped[1].id, 11);
    }

    #[test]
    fn test_sort_groups_by_id() {
        let photos = vec![
            make_photo(20, 1, "/g2_sot.jpg", 100),
            make_photo(21, 1, "/g2_dup.jpg", 100),
            make_photo(10, 1, "/g1_sot.jpg", 100),
            make_photo(11, 1, "/g1_dup.jpg", 100),
        ];
        let groups = vec![
            make_group(1, 10, &[10, 11]),
            make_group(2, 20, &[20, 21]),
        ];
        let data = StatusData::build(&groups);
        let (grouped, _) = sort_photos_for_display(&photos, &data);

        // Group 1 first, then group 2; SOT first in each
        assert_eq!(grouped[0].id, 10);
        assert_eq!(grouped[1].id, 11);
        assert_eq!(grouped[2].id, 20);
        assert_eq!(grouped[3].id, 21);
    }

    #[test]
    fn test_sort_mixed_grouped_and_ungrouped() {
        let photos = vec![
            make_photo(30, 1, "/unique.jpg", 100),
            make_photo(11, 1, "/dup.jpg", 100),
            make_photo(10, 1, "/sot.jpg", 100),
        ];
        let groups = vec![make_group(1, 10, &[10, 11])];
        let data = StatusData::build(&groups);
        let (grouped, ungrouped) = sort_photos_for_display(&photos, &data);

        assert_eq!(grouped.len(), 2);
        assert_eq!(ungrouped.len(), 1);
        assert_eq!(grouped[0].id, 10); // SOT first
        assert_eq!(grouped[1].id, 11);
        assert_eq!(ungrouped[0].id, 30);
    }
}
