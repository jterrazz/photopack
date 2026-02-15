pub mod confidence;

use std::collections::{HashMap, HashSet};

use crate::domain::{Confidence, PhotoFile};
use crate::hasher::perceptual::hamming_distance;
use confidence::confidence_from_hamming;

/// BK-tree for efficient Hamming distance nearest-neighbor search.
/// Allows finding all items within a given distance in O(n^α) where α < 1,
/// much faster than brute-force O(n) for each query.
struct BkTree {
    root: Option<BkNode>,
}

struct BkNode {
    hash: u64,
    photo_id: i64,
    children: HashMap<u32, BkNode>,
}

impl BkTree {
    fn new() -> Self {
        Self { root: None }
    }

    fn insert(&mut self, hash: u64, photo_id: i64) {
        match self.root {
            None => {
                self.root = Some(BkNode {
                    hash,
                    photo_id,
                    children: HashMap::new(),
                });
            }
            Some(ref mut root) => {
                Self::insert_into(root, hash, photo_id);
            }
        }
    }

    fn insert_into(node: &mut BkNode, hash: u64, photo_id: i64) {
        let dist = hamming_distance(node.hash, hash);
        if let Some(child) = node.children.get_mut(&dist) {
            Self::insert_into(child, hash, photo_id);
        } else {
            node.children.insert(dist, BkNode {
                hash,
                photo_id,
                children: HashMap::new(),
            });
        }
    }

    /// Find all entries within `max_distance` of `query_hash`.
    fn find_within(&self, query_hash: u64, max_distance: u32) -> Vec<(i64, u32)> {
        let mut results = Vec::new();
        if let Some(ref root) = self.root {
            Self::search(root, query_hash, max_distance, &mut results);
        }
        results
    }

    fn search(node: &BkNode, query_hash: u64, max_distance: u32, results: &mut Vec<(i64, u32)>) {
        let dist = hamming_distance(node.hash, query_hash);
        if dist <= max_distance {
            results.push((node.photo_id, dist));
        }
        let low = dist.saturating_sub(max_distance);
        let high = dist + max_distance;
        for d in low..=high {
            if let Some(child) = node.children.get(&d) {
                Self::search(child, query_hash, max_distance, results);
            }
        }
    }
}

/// A raw match group before final merge.
#[derive(Debug, Clone)]
pub struct MatchGroup {
    pub member_ids: Vec<i64>,
    pub confidence: Confidence,
}

/// Run the full matching pipeline on a set of photos.
/// Returns groups of duplicate photos with confidence levels.
pub fn find_duplicates(photos: &[PhotoFile]) -> Vec<MatchGroup> {
    if photos.len() < 2 {
        return Vec::new();
    }

    let mut groups: Vec<MatchGroup> = Vec::new();

    // Phase 1: Exact SHA-256 match → Certain
    let sha_groups = group_by_sha256(photos);
    for members in sha_groups.values() {
        if members.len() >= 2 {
            groups.push(MatchGroup {
                member_ids: members.iter().map(|p| p.id).collect(),
                confidence: Confidence::Certain,
            });
        }
    }

    // Phase 2: EXIF triangulation + pHash validation → NearCertain/High
    // Note: we do NOT exclude SHA-256 grouped IDs here — EXIF groups may
    // overlap with SHA groups (e.g. same photo in different formats), and
    // Phase 4 will merge them.
    let empty_set = HashSet::new();
    let exif_groups = group_by_exif(photos, &empty_set);
    for mut group in exif_groups {
        // Try to validate with pHash — if at least one pair validates, upgrade
        // to High confidence. Otherwise keep at NearCertain (EXIF-only).
        // Keep ALL EXIF members regardless — pHash is a confidence booster, not a filter.
        let validated = validate_with_phash(&group.member_ids, photos);
        if validated.len() >= 2 {
            group.confidence = Confidence::High;
        } else {
            group.confidence = Confidence::NearCertain;
        }
        groups.push(group);
    }

    // Collect all grouped IDs for Phase 3 exclusion (perceptual-only is the fallback)
    let mut grouped_ids: HashSet<i64> = HashSet::new();
    for g in &groups {
        for &id in &g.member_ids {
            grouped_ids.insert(id);
        }
    }

    // Phase 3: pHash/dHash Hamming distance → Probable
    let perceptual_groups = group_by_perceptual_hash(photos, &grouped_ids);
    for group in perceptual_groups {
        for &id in &group.member_ids {
            grouped_ids.insert(id);
        }
        groups.push(group);
    }

    // Phase 4: Merge overlapping groups
    merge_overlapping(&mut groups)
}

/// Phase 1: Group photos by identical SHA-256 hash.
fn group_by_sha256(photos: &[PhotoFile]) -> HashMap<String, Vec<&PhotoFile>> {
    let mut map: HashMap<String, Vec<&PhotoFile>> = HashMap::new();
    for photo in photos {
        map.entry(photo.sha256.clone()).or_default().push(photo);
    }
    map
}

/// Phase 2: Group photos by EXIF date + camera, producing clusters of potential duplicates.
fn group_by_exif(photos: &[PhotoFile], excluded: &HashSet<i64>) -> Vec<MatchGroup> {
    let mut date_camera_map: HashMap<String, Vec<i64>> = HashMap::new();

    for photo in photos {
        if excluded.contains(&photo.id) {
            continue;
        }

        if let Some(ref exif) = photo.exif {
            if let Some(ref date) = exif.date {
                let key = format!(
                    "{}|{}",
                    date,
                    exif.camera_model.as_deref().unwrap_or("unknown")
                );
                date_camera_map.entry(key).or_default().push(photo.id);
            }
        }
    }

    date_camera_map
        .into_values()
        .filter(|ids| ids.len() >= 2)
        .map(|member_ids| MatchGroup {
            member_ids,
            confidence: Confidence::High,
        })
        .collect()
}

/// Validate a group of photo IDs using perceptual hash distance.
/// Keeps only photos that are perceptually close to at least one other member.
fn validate_with_phash(ids: &[i64], photos: &[PhotoFile]) -> Vec<i64> {
    let photo_map: HashMap<i64, &PhotoFile> = photos.iter().map(|p| (p.id, p)).collect();
    let mut valid = HashSet::new();

    for (i, &id_a) in ids.iter().enumerate() {
        for &id_b in &ids[i + 1..] {
            if let (Some(pa), Some(pb)) = (photo_map.get(&id_a), photo_map.get(&id_b)) {
                if let (Some(phash_a), Some(phash_b)) = (pa.phash, pb.phash) {
                    let dist = hamming_distance(phash_a, phash_b);
                    if confidence_from_hamming(dist).is_some() {
                        valid.insert(id_a);
                        valid.insert(id_b);
                    }
                }
            }
        }
    }

    valid.into_iter().collect()
}

/// Phase 3: Group ungrouped photos by perceptual hash similarity.
/// Ungrouped photos are compared against ALL photos (including already-grouped
/// ones) so that cross-format duplicates create bridge groups that Phase 4 merges.
/// Uses a BK-tree for O(n log n) Hamming distance lookups instead of O(n²).
fn group_by_perceptual_hash(photos: &[PhotoFile], excluded: &HashSet<i64>) -> Vec<MatchGroup> {
    use confidence::PHASH_PROBABLE_THRESHOLD;

    // Build BK-tree from ALL photos with a perceptual hash
    let mut tree = BkTree::new();
    for photo in photos {
        if let Some(phash) = photo.phash {
            tree.insert(phash, photo.id);
        }
    }

    // Ungrouped photos that have a perceptual hash — these seed new groups.
    let ungrouped: Vec<&PhotoFile> = photos
        .iter()
        .filter(|p| !excluded.contains(&p.id) && p.phash.is_some())
        .collect();

    let mut groups: Vec<MatchGroup> = Vec::new();
    let mut used: HashSet<i64> = HashSet::new();

    for &photo_a in &ungrouped {
        if used.contains(&photo_a.id) {
            continue;
        }

        let phash_a = photo_a.phash.unwrap();
        let neighbors = tree.find_within(phash_a, PHASH_PROBABLE_THRESHOLD);

        let mut members = vec![photo_a.id];
        let mut worst_confidence = Confidence::Certain;

        for (neighbor_id, dist) in &neighbors {
            if *neighbor_id == photo_a.id || used.contains(neighbor_id) {
                continue;
            }

            if let Some(conf) = confidence_from_hamming(*dist) {
                members.push(*neighbor_id);
                if conf < worst_confidence {
                    worst_confidence = conf;
                }
            }
        }

        if members.len() >= 2 {
            for &id in &members {
                used.insert(id);
            }
            groups.push(MatchGroup {
                member_ids: members,
                confidence: worst_confidence,
            });
        }
    }

    groups
}

/// Phase 4: Merge groups that share any member IDs.
/// Iterates until no more merges are possible (handles transitive overlaps).
fn merge_overlapping(groups: &mut Vec<MatchGroup>) -> Vec<MatchGroup> {
    let mut merged: Vec<MatchGroup> = Vec::new();

    for group in groups.drain(..) {
        let group_set: HashSet<i64> = group.member_ids.iter().copied().collect();

        // Find ALL existing merged groups that overlap with this one
        let overlap_indices: Vec<usize> = merged
            .iter()
            .enumerate()
            .filter(|(_, mg)| mg.member_ids.iter().any(|id| group_set.contains(id)))
            .map(|(i, _)| i)
            .collect();

        if overlap_indices.is_empty() {
            merged.push(group);
        } else {
            // Merge this group and all overlapping groups into one
            let mut combined_ids: HashSet<i64> = group_set;
            let mut worst_confidence = group.confidence;

            // Remove overlapping groups in reverse order to preserve indices
            for &idx in overlap_indices.iter().rev() {
                let removed = merged.remove(idx);
                combined_ids.extend(removed.member_ids);
                if removed.confidence < worst_confidence {
                    worst_confidence = removed.confidence;
                }
            }

            merged.push(MatchGroup {
                member_ids: combined_ids.into_iter().collect(),
                confidence: worst_confidence,
            });
        }
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExifData, PhotoFormat};
    use std::path::PathBuf;

    fn make_photo(id: i64, sha: &str, phash: Option<u64>) -> PhotoFile {
        PhotoFile {
            id,
            source_id: 1,
            path: PathBuf::from(format!("/test/{id}.jpg")),
            size: 1000,
            format: PhotoFormat::Jpeg,
            sha256: sha.to_string(),
            phash,
            dhash: None,
            exif: None,
            mtime: 1000,
        }
    }

    fn make_photo_with_exif(
        id: i64,
        sha: &str,
        phash: Option<u64>,
        date: &str,
        camera: &str,
    ) -> PhotoFile {
        let mut p = make_photo(id, sha, phash);
        p.exif = Some(ExifData {
            date: Some(date.to_string()),
            camera_make: None,
            camera_model: Some(camera.to_string()),
            gps_lat: None,
            gps_lon: None,
            width: None,
            height: None,
        });
        p
    }

    // ── Phase 1: SHA-256 ─────────────────────────────────────────

    #[test]
    fn test_exact_sha256_match() {
        let photos = vec![
            make_photo(1, "aaa", Some(100)),
            make_photo(2, "aaa", Some(100)),
            make_photo(3, "bbb", Some(u64::MAX)), // far from 100 in Hamming distance
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].confidence, Confidence::Certain);
        assert_eq!(groups[0].member_ids.len(), 2);
    }

    #[test]
    fn test_sha256_three_way_match() {
        let photos = vec![
            make_photo(1, "aaa", Some(100)),
            make_photo(2, "aaa", Some(100)),
            make_photo(3, "aaa", Some(100)),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].member_ids.len(), 3);
    }

    #[test]
    fn test_multiple_sha256_groups() {
        let photos = vec![
            make_photo(1, "aaa", Some(100)),
            make_photo(2, "aaa", Some(100)),
            make_photo(3, "bbb", Some(u64::MAX)),
            make_photo(4, "bbb", Some(u64::MAX)),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.confidence == Confidence::Certain));
    }

    // ── Phase 2: EXIF triangulation ──────────────────────────────

    #[test]
    fn test_exif_match_without_phash() {
        // Same date + camera, no pHash → should still group at NearCertain
        let photos = vec![
            make_photo_with_exif(1, "aaa", None, "2024-01-15 12:00:00", "iPhone 16"),
            make_photo_with_exif(2, "bbb", None, "2024-01-15 12:00:00", "iPhone 16"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].confidence, Confidence::NearCertain);
        assert_eq!(groups[0].member_ids.len(), 2);
    }

    #[test]
    fn test_exif_match_with_phash_validation() {
        // Same date + camera, close pHash → High confidence
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(100), "2024-01-15 12:00:00", "Canon R5"),
            make_photo_with_exif(2, "bbb", Some(101), "2024-01-15 12:00:00", "Canon R5"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].confidence, Confidence::High);
    }

    #[test]
    fn test_exif_different_dates_no_group() {
        let photos = vec![
            make_photo_with_exif(1, "aaa", None, "2024-01-15 12:00:00", "iPhone 16"),
            make_photo_with_exif(2, "bbb", None, "2024-01-16 12:00:00", "iPhone 16"),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_exif_different_cameras_no_group() {
        let photos = vec![
            make_photo_with_exif(1, "aaa", None, "2024-01-15 12:00:00", "iPhone 16"),
            make_photo_with_exif(2, "bbb", None, "2024-01-15 12:00:00", "Canon R5"),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty());
    }

    // ── Phase 3: Perceptual hash ─────────────────────────────────

    #[test]
    fn test_perceptual_hash_close_match() {
        // Different SHA, no EXIF, but very close pHash
        let photos = vec![
            make_photo(1, "aaa", Some(0b1111_0000)),
            make_photo(2, "bbb", Some(0b1111_0001)), // 1 bit different
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].confidence >= Confidence::Probable);
    }

    #[test]
    fn test_perceptual_hash_distant_no_match() {
        // pHash too far apart
        let photos = vec![
            make_photo(1, "aaa", Some(0)),
            make_photo(2, "bbb", Some(u64::MAX)),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_no_phash_no_exif_no_sha_match() {
        // Completely different photos with no pHash
        let photos = vec![
            make_photo(1, "aaa", None),
            make_photo(2, "bbb", None),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty());
    }

    // ── Phase 4: Merge ───────────────────────────────────────────

    #[test]
    fn test_merge_overlapping_groups() {
        let mut groups = vec![
            MatchGroup {
                member_ids: vec![1, 2],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![2, 3],
                confidence: Confidence::High,
            },
        ];

        let merged = merge_overlapping(&mut groups);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].member_ids.len(), 3);
        // Takes the lower confidence
        assert_eq!(merged[0].confidence, Confidence::High);
    }

    #[test]
    fn test_no_overlap_stays_separate() {
        let mut groups = vec![
            MatchGroup {
                member_ids: vec![1, 2],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![3, 4],
                confidence: Confidence::High,
            },
        ];

        let merged = merge_overlapping(&mut groups);
        assert_eq!(merged.len(), 2);
    }

    // ── Edge cases ───────────────────────────────────────────────

    #[test]
    fn test_no_duplicates() {
        let photos = vec![
            make_photo(1, "aaa", Some(100)),
            make_photo(2, "bbb", Some(u64::MAX)),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_single_photo_no_group() {
        let photos = vec![make_photo(1, "aaa", Some(100))];
        let groups = find_duplicates(&photos);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_empty_input() {
        let groups = find_duplicates(&[]);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_sha256_and_exif_groups_merge() {
        // Photos 1 and 2 match by SHA256. Photo 3 has same EXIF as 1 but different SHA.
        // Phase 2 creates an EXIF group {1,3}, which overlaps with SHA group {1,2}.
        // Phase 4 should merge them into a single group {1,2,3}.
        let mut p1 = make_photo_with_exif(1, "aaa", None, "2024-01-15 12:00:00", "iPhone");
        let p2 = make_photo(2, "aaa", None);
        let p3 = make_photo_with_exif(3, "ccc", None, "2024-01-15 12:00:00", "iPhone");
        p1.sha256 = "aaa".to_string();

        let photos = vec![p1, p2, p3];
        let groups = find_duplicates(&photos);

        // All three should end up in a single merged group
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].member_ids.len(), 3);
    }

    // ── Cross-format + cross-directory scenarios (regression tests) ──

    #[test]
    fn test_cross_format_same_exif_grouped() {
        // Simulates: IMG_1234.jpeg and IMG_1234.heic — different SHA, same EXIF date+camera,
        // HEIC has no pHash. Both should be in one group.
        let p1 = make_photo_with_exif(1, "sha_jpeg", Some(100), "2024-01-15 12:00:00", "iPhone 16 Pro");
        let mut p2 = make_photo_with_exif(2, "sha_heic", None, "2024-01-15 12:00:00", "iPhone 16 Pro");
        p2.format = PhotoFormat::Heic;

        let photos = vec![p1, p2];
        let groups = find_duplicates(&photos);

        assert_eq!(groups.len(), 1, "JPEG+HEIC with same EXIF should group");
        assert_eq!(groups[0].member_ids.len(), 2);
    }

    #[test]
    fn test_cross_format_cross_directory_all_merge() {
        // Real-world scenario: IMG_3234.jpeg and IMG_3234.heic in both test/ and test2/.
        // That's 4 files total. SHA pairs: {jpeg1, jpeg2} and {heic1, heic2}.
        // EXIF group: all 4 have same date + camera.
        // Should merge into a single group of 4.
        let p1 = make_photo_with_exif(1, "sha_jpeg", Some(100), "2024-01-15 12:00:00", "iPhone 16 Pro");
        let p2 = make_photo_with_exif(2, "sha_jpeg", Some(100), "2024-01-15 12:00:00", "iPhone 16 Pro");
        let mut p3 = make_photo_with_exif(3, "sha_heic", None, "2024-01-15 12:00:00", "iPhone 16 Pro");
        let mut p4 = make_photo_with_exif(4, "sha_heic", None, "2024-01-15 12:00:00", "iPhone 16 Pro");
        p3.format = PhotoFormat::Heic;
        p4.format = PhotoFormat::Heic;

        let photos = vec![p1, p2, p3, p4];
        let groups = find_duplicates(&photos);

        assert_eq!(groups.len(), 1, "All 4 files should merge into one group");
        assert_eq!(groups[0].member_ids.len(), 4);
    }

    #[test]
    fn test_three_image_pairs_three_groups() {
        // 3 different photos, each with a JPEG+HEIC pair → 3 separate groups of 2.
        let photos = vec![
            make_photo_with_exif(1, "sha_a_jpg", Some(100), "2024-01-10 10:00:00", "iPhone"),
            make_photo_with_exif(2, "sha_a_heic", None, "2024-01-10 10:00:00", "iPhone"),
            make_photo_with_exif(3, "sha_b_jpg", Some(200), "2024-01-11 11:00:00", "iPhone"),
            make_photo_with_exif(4, "sha_b_heic", None, "2024-01-11 11:00:00", "iPhone"),
            make_photo_with_exif(5, "sha_c_jpg", Some(300), "2024-01-12 12:00:00", "iPhone"),
            make_photo_with_exif(6, "sha_c_heic", None, "2024-01-12 12:00:00", "iPhone"),
        ];

        let groups = find_duplicates(&photos);

        assert_eq!(groups.len(), 3, "Should have 3 separate groups");
        for group in &groups {
            assert_eq!(group.member_ids.len(), 2, "Each group should have 2 members");
        }
    }

    #[test]
    fn test_exif_keeps_members_without_phash() {
        // 3 files: same EXIF. Only photo 1 has pHash. All 3 should stay in the group,
        // not just the ones with pHash.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(100), "2024-01-15 12:00:00", "Canon R5"),
            make_photo_with_exif(2, "bbb", None, "2024-01-15 12:00:00", "Canon R5"),
            make_photo_with_exif(3, "ccc", None, "2024-01-15 12:00:00", "Canon R5"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].member_ids.len(), 3, "All EXIF-matching members should be kept");
    }

    // ── Transitive merge regression tests ────────────────────────────

    #[test]
    fn test_transitive_merge_three_chains() {
        // Group A: {1,2}, Group B: {2,3}, Group C: {3,4}
        // After merge: single group {1,2,3,4}
        let mut groups = vec![
            MatchGroup {
                member_ids: vec![1, 2],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![2, 3],
                confidence: Confidence::High,
            },
            MatchGroup {
                member_ids: vec![3, 4],
                confidence: Confidence::NearCertain,
            },
        ];

        let merged = merge_overlapping(&mut groups);
        assert_eq!(merged.len(), 1, "Transitive chain should collapse to 1 group");
        assert_eq!(merged[0].member_ids.len(), 4);
        assert_eq!(merged[0].confidence, Confidence::High, "Worst confidence wins");
    }

    #[test]
    fn test_transitive_merge_bridge_two_disjoint_groups() {
        // Groups {1,2} and {3,4} are disjoint. Then group {2,3} bridges them.
        // All should merge into {1,2,3,4}.
        let mut groups = vec![
            MatchGroup {
                member_ids: vec![1, 2],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![3, 4],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![2, 3],
                confidence: Confidence::High,
            },
        ];

        let merged = merge_overlapping(&mut groups);
        assert_eq!(merged.len(), 1, "Bridge group should merge the two disjoint groups");
        assert_eq!(merged[0].member_ids.len(), 4);
    }

    #[test]
    fn test_transitive_merge_multiple_bridges() {
        // 3 disjoint groups linked by a group that touches all of them.
        let mut groups = vec![
            MatchGroup {
                member_ids: vec![1, 2],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![3, 4],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![5, 6],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![2, 4, 6],
                confidence: Confidence::Probable,
            },
        ];

        let merged = merge_overlapping(&mut groups);
        assert_eq!(merged.len(), 1, "Single bridge touching all groups should merge everything");
        assert_eq!(merged[0].member_ids.len(), 6);
        assert_eq!(merged[0].confidence, Confidence::Probable);
    }

    #[test]
    fn test_merge_preserves_independent_groups() {
        // Two completely independent chains that should NOT merge.
        let mut groups = vec![
            MatchGroup {
                member_ids: vec![1, 2],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![2, 3],
                confidence: Confidence::High,
            },
            MatchGroup {
                member_ids: vec![10, 11],
                confidence: Confidence::Certain,
            },
            MatchGroup {
                member_ids: vec![11, 12],
                confidence: Confidence::High,
            },
        ];

        let merged = merge_overlapping(&mut groups);
        assert_eq!(merged.len(), 2, "Two independent chains should stay separate");
    }

    // ── Full pipeline: cross-format + cross-directory ────────────────

    #[test]
    fn test_full_pipeline_two_sources_jpeg_heic_pairs() {
        // Reproduces the real-world scenario:
        // Source A: IMG_3234.jpeg (sha_j), IMG_3234.heic (sha_h)
        // Source B: IMG_3234.jpeg (sha_j), IMG_3234.heic (sha_h)
        // SHA groups: {1,3} and {2,4}. EXIF group: {1,2,3,4}. All merge → 1 group.
        let photos = vec![
            // Source A
            make_photo_with_exif(1, "sha_jpeg_3234", Some(500), "2024-01-12 20:30:48", "iPhone 16 Pro Max"),
            {
                let mut p = make_photo_with_exif(2, "sha_heic_3234", None, "2024-01-12 20:30:48", "iPhone 16 Pro Max");
                p.format = PhotoFormat::Heic;
                p.size = 900_000;
                p
            },
            // Source B (exact copies)
            make_photo_with_exif(3, "sha_jpeg_3234", Some(500), "2024-01-12 20:30:48", "iPhone 16 Pro Max"),
            {
                let mut p = make_photo_with_exif(4, "sha_heic_3234", None, "2024-01-12 20:30:48", "iPhone 16 Pro Max");
                p.format = PhotoFormat::Heic;
                p.size = 900_000;
                p
            },
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "All 4 files should be in one merged group");
        assert_eq!(groups[0].member_ids.len(), 4);
    }

    #[test]
    fn test_full_pipeline_three_images_two_sources() {
        // 3 different photos × 2 formats × 2 sources = 12 files → 3 groups of 4.
        let dates = ["2024-01-10 10:00:00", "2024-01-11 11:00:00", "2024-01-12 12:00:00"];
        let jpeg_shas = ["sha_j_a", "sha_j_b", "sha_j_c"];
        let heic_shas = ["sha_h_a", "sha_h_b", "sha_h_c"];
        let phashes: [u64; 3] = [100, 200, 300]; // far apart

        let mut photos = Vec::new();
        let mut id = 1;
        for i in 0..3 {
            // Source 1 JPEG
            photos.push(make_photo_with_exif(id, jpeg_shas[i], Some(phashes[i]), dates[i], "iPhone"));
            id += 1;
            // Source 1 HEIC
            let mut p = make_photo_with_exif(id, heic_shas[i], None, dates[i], "iPhone");
            p.format = PhotoFormat::Heic;
            photos.push(p);
            id += 1;
            // Source 2 JPEG (exact copy)
            photos.push(make_photo_with_exif(id, jpeg_shas[i], Some(phashes[i]), dates[i], "iPhone"));
            id += 1;
            // Source 2 HEIC (exact copy)
            let mut p = make_photo_with_exif(id, heic_shas[i], None, dates[i], "iPhone");
            p.format = PhotoFormat::Heic;
            photos.push(p);
            id += 1;
        }

        assert_eq!(photos.len(), 12);
        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 3, "Should have exactly 3 groups");
        for group in &groups {
            assert_eq!(group.member_ids.len(), 4, "Each group should have 4 members");
        }
    }
}
