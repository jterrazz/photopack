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

    // Phase 2: EXIF triangulation + perceptual hash validation → NearCertain/High
    // Note: we do NOT exclude SHA-256 grouped IDs here — EXIF groups may
    // overlap with SHA groups (e.g. same photo in different formats), and
    // Phase 4 will merge them.
    let empty_set = HashSet::new();
    let exif_groups = group_by_exif(photos, &empty_set);
    let photo_map: HashMap<i64, &PhotoFile> = photos.iter().map(|p| (p.id, p)).collect();
    for group in exif_groups {
        let validated = validate_with_perceptual_hash(&group.member_ids, photos);

        // Filter: keep members that either (a) passed visual validation, or
        // (b) lack perceptual hashes entirely (HEIC/RAW — EXIF is our best signal), or
        // (c) have phash but had no comparison partner (only 1 member with phash).
        // Remove members that HAVE hashes AND had comparison partners but FAILED.
        let ids_with_phash: usize = group
            .member_ids
            .iter()
            .filter(|&&id| {
                photo_map
                    .get(&id)
                    .and_then(|p| p.phash)
                    .is_some()
            })
            .count();
        let has_comparison_partner = ids_with_phash >= 2;

        let filtered: Vec<i64> = group
            .member_ids
            .iter()
            .filter(|&&id| {
                if validated.contains(&id) {
                    return true; // visually validated
                }
                let has_phash = photo_map
                    .get(&id)
                    .and_then(|p| p.phash)
                    .is_some();
                if !has_phash {
                    return true; // no phash (HEIC/RAW) — can't validate, keep
                }
                // Has phash but not validated — keep only if no comparison partner
                !has_comparison_partner
            })
            .copied()
            .collect();

        if filtered.len() >= 2 {
            let confidence = if validated.len() >= 2 {
                Confidence::High
            } else {
                Confidence::NearCertain
            };
            groups.push(MatchGroup {
                member_ids: filtered,
                confidence,
            });
        }
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

    // Phase 4: Merge overlapping groups (with cross-group visual validation)
    merge_overlapping(&mut groups, photos)
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

/// Validate a group of photo IDs using perceptual hash distance (strict dual-hash consensus).
/// Returns IDs of photos that are perceptually close to at least one other member.
/// Uses NEAR_CERTAIN threshold (≤2 bits) for EXIF validation — only true duplicates pass.
/// Sequential/burst shots (distance 3+) are rejected.
fn validate_with_perceptual_hash(ids: &[i64], photos: &[PhotoFile]) -> HashSet<i64> {
    use confidence::PHASH_NEAR_CERTAIN_THRESHOLD;

    let photo_map: HashMap<i64, &PhotoFile> = photos.iter().map(|p| (p.id, p)).collect();
    let mut valid = HashSet::new();

    for (i, &id_a) in ids.iter().enumerate() {
        for &id_b in &ids[i + 1..] {
            if let (Some(pa), Some(pb)) = (photo_map.get(&id_a), photo_map.get(&id_b)) {
                if let (Some(phash_a), Some(phash_b)) = (pa.phash, pb.phash) {
                    let phash_dist = hamming_distance(phash_a, phash_b);
                    let is_match = match (pa.dhash, pb.dhash) {
                        (Some(da), Some(db)) => {
                            let dhash_dist = hamming_distance(da, db);
                            phash_dist <= PHASH_NEAR_CERTAIN_THRESHOLD
                                && dhash_dist <= PHASH_NEAR_CERTAIN_THRESHOLD
                        }
                        _ => phash_dist <= PHASH_NEAR_CERTAIN_THRESHOLD,
                    };
                    if is_match {
                        valid.insert(id_a);
                        valid.insert(id_b);
                    }
                }
            }
        }
    }

    valid
}

/// Phase 3: Group ungrouped photos by perceptual hash similarity.
/// Ungrouped photos are compared against ALL photos (including already-grouped
/// ones) so that cross-format duplicates create bridge groups that Phase 4 merges.
/// Uses a BK-tree for O(n log n) Hamming distance lookups instead of O(n²).
///
/// Dual-hash consensus: when both photos have phash AND dhash, both must be
/// within threshold. When dhash is missing (cross-format), phash alone is
/// accepted only at the stricter HIGH threshold.
fn group_by_perceptual_hash(photos: &[PhotoFile], excluded: &HashSet<i64>) -> Vec<MatchGroup> {
    use confidence::{PHASH_HIGH_THRESHOLD, PHASH_PROBABLE_THRESHOLD};

    // Build lookup map for dhash access
    let photo_map: HashMap<i64, &PhotoFile> = photos.iter().map(|p| (p.id, p)).collect();

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

        for (neighbor_id, phash_dist) in &neighbors {
            if *neighbor_id == photo_a.id || used.contains(neighbor_id) {
                continue;
            }

            let phash_conf = match confidence_from_hamming(*phash_dist) {
                Some(c) => c,
                None => continue,
            };

            // Dual-hash consensus: check dhash when both photos have it
            let neighbor = photo_map.get(neighbor_id);
            let conf = match (photo_a.dhash, neighbor.and_then(|p| p.dhash)) {
                (Some(da), Some(db)) => {
                    let dhash_dist = hamming_distance(da, db);
                    match confidence_from_hamming(dhash_dist) {
                        Some(dc) => confidence::combine_confidence(phash_conf, dc),
                        None => continue, // dhash too far → reject
                    }
                }
                _ => {
                    // One or both lack dhash (cross-format) — require stricter phash
                    if *phash_dist > PHASH_HIGH_THRESHOLD {
                        continue;
                    }
                    phash_conf
                }
            };

            members.push(*neighbor_id);
            if conf < worst_confidence {
                worst_confidence = conf;
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
/// Before merging, validates that the groups are visually related — at least one
/// pair of exclusive members (one from each group) must have perceptual hashes
/// within threshold. This prevents cascading false merges through bridge photos.
fn merge_overlapping(groups: &mut Vec<MatchGroup>, photos: &[PhotoFile]) -> Vec<MatchGroup> {
    let photo_map: HashMap<i64, &PhotoFile> = photos.iter().map(|p| (p.id, p)).collect();
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
            // Validate cross-group visual similarity before merging.
            // For each overlapping group, check that at least one pair of
            // exclusive members (one from each side) are perceptually close.
            let mut to_merge: Vec<usize> = Vec::new();
            for &idx in &overlap_indices {
                if cross_group_validated(&group_set, &merged[idx], &photo_map) {
                    to_merge.push(idx);
                }
            }

            if to_merge.is_empty() {
                // Overlap exists but groups are visually unrelated — keep separate
                merged.push(group);
            } else {
                let mut combined_ids: HashSet<i64> = group_set;
                let mut worst_confidence = group.confidence;

                for &idx in to_merge.iter().rev() {
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
    }

    merged
}

/// Check if two groups have at least one pair of perceptually similar exclusive members.
/// "Exclusive" means members not in the overlap (i.e., unique to each group).
/// If there are no exclusive members on one side, allow the merge (pure subset).
fn cross_group_validated(
    new_set: &HashSet<i64>,
    existing: &MatchGroup,
    photo_map: &HashMap<i64, &PhotoFile>,
) -> bool {
    let existing_set: HashSet<i64> = existing.member_ids.iter().copied().collect();

    // Members exclusive to each group
    let new_exclusive: Vec<i64> = new_set.difference(&existing_set).copied().collect();
    let existing_exclusive: Vec<i64> = existing_set.difference(new_set).copied().collect();

    // If either side has no exclusive members, it's a pure subset — allow merge
    if new_exclusive.is_empty() || existing_exclusive.is_empty() {
        return true;
    }

    // If either side lacks photos with phash, can't validate — allow merge
    let new_has_phash = new_exclusive
        .iter()
        .any(|id| photo_map.get(id).and_then(|p| p.phash).is_some());
    let existing_has_phash = existing_exclusive
        .iter()
        .any(|id| photo_map.get(id).and_then(|p| p.phash).is_some());
    if !new_has_phash || !existing_has_phash {
        return true;
    }

    // Check if at least one cross-group pair is perceptually close
    for &id_a in &new_exclusive {
        for &id_b in &existing_exclusive {
            if let (Some(pa), Some(pb)) = (photo_map.get(&id_a), photo_map.get(&id_b)) {
                if let (Some(phash_a), Some(phash_b)) = (pa.phash, pb.phash) {
                    let dist = hamming_distance(phash_a, phash_b);
                    if confidence_from_hamming(dist).is_some() {
                        return true;
                    }
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExifData, PhotoFormat};
    use std::path::PathBuf;

    fn make_photo(id: i64, sha: &str, phash: Option<u64>) -> PhotoFile {
        make_photo_full(id, sha, phash, phash) // dhash defaults to same as phash
    }

    fn make_photo_full(
        id: i64,
        sha: &str,
        phash: Option<u64>,
        dhash: Option<u64>,
    ) -> PhotoFile {
        PhotoFile {
            id,
            source_id: 1,
            path: PathBuf::from(format!("/test/{id}.jpg")),
            size: 1000,
            format: PhotoFormat::Jpeg,
            sha256: sha.to_string(),
            phash,
            dhash,
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
    fn test_dual_hash_consensus_rejects_single_hash_match() {
        // phash close (distance 1) but dhash far → should NOT group
        let photos = vec![
            make_photo_full(1, "aaa", Some(0b1111_0000), Some(0)),
            make_photo_full(2, "bbb", Some(0b1111_0001), Some(u64::MAX)),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Dual-hash: close phash + far dhash should reject");
    }

    #[test]
    fn test_dual_hash_consensus_accepts_when_both_close() {
        // Both phash and dhash close → should group
        let photos = vec![
            make_photo_full(1, "aaa", Some(0b1111_0000), Some(0b1010_0000)),
            make_photo_full(2, "bbb", Some(0b1111_0001), Some(0b1010_0001)),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Dual-hash: both close should group");
    }

    #[test]
    fn test_exif_rejects_sequential_shots() {
        // Regression test: sequential birthday photos with same EXIF.
        // aHash similar (distance 3) but dHash divergent (distance 6) — realistic for
        // sequential shots where the scene is similar but composition differs.
        // Phase 2 EXIF validation uses NEAR_CERTAIN (≤2) → rejects.
        // Phase 3 dual-hash consensus: dhash distance 6 > PROBABLE (3) → rejects.
        let photos = vec![
            {
                let mut p = make_photo_with_exif(1, "aaa", Some(0b1111_0000), "2024-01-15 12:00:00", "iPhone 16");
                p.dhash = Some(0b0000_0000);
                p
            },
            {
                let mut p = make_photo_with_exif(2, "bbb", Some(0b1111_0111), "2024-01-15 12:00:00", "iPhone 16");
                p.dhash = Some(0b0011_1111); // 6 bits different in dhash
                p
            },
        ];

        let groups = find_duplicates(&photos);
        assert!(
            groups.is_empty(),
            "Sequential shots with divergent dHash must NOT be grouped (birthday photo bug)"
        );
    }

    #[test]
    fn test_exif_rejects_when_phash_distance_3() {
        // Same EXIF, phash distance 3, dhash distance 3.
        // Phase 2 EXIF validation requires NEAR_CERTAIN (≤2) → distance 3 rejected.
        // Phase 3 dual-hash: both distance 3 = PROBABLE → grouped by Phase 3.
        // This is acceptable because Phase 3 requires BOTH hashes to agree at distance 3,
        // which is very rare for truly different photos in practice.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0b1111_0000), "2024-01-15 12:00:00", "iPhone 16"),
            make_photo_with_exif(2, "bbb", Some(0b1111_0111), "2024-01-15 12:00:00", "iPhone 16"),
        ];

        let groups = find_duplicates(&photos);
        // Phase 3 catches these because both phash AND dhash are distance 3
        // (dhash defaults to same as phash in make_photo). In reality, different photos
        // have divergent dHash values (gradient patterns differ), so dual-hash consensus blocks them.
        assert_eq!(groups.len(), 1, "Phase 3 dual-hash at distance 3 groups when both agree");
    }

    #[test]
    fn test_phase3_rejects_distance_4() {
        // Two photos with no EXIF, phash distance 4 → NOT grouped by Phase 3 (PROBABLE=3)
        let photos = vec![
            make_photo(1, "aaa", Some(0b1111_0000)),
            make_photo(2, "bbb", Some(0b1111_1111)), // 4 bits different
        ];

        let groups = find_duplicates(&photos);
        assert!(
            groups.is_empty(),
            "Phase 3 should reject photos with phash distance 4"
        );
    }

    #[test]
    fn test_phase3_dual_hash_rejects_mixed_distances() {
        // phash distance 2 (within threshold) but dhash distance 4 (beyond threshold)
        // Dual-hash consensus should reject.
        let photos = vec![
            make_photo_full(1, "aaa", Some(0b1111_0000), Some(0b0000_0000)),
            make_photo_full(2, "bbb", Some(0b1111_0011), Some(0b0000_1111)), // phash=2, dhash=4
        ];

        let groups = find_duplicates(&photos);
        assert!(
            groups.is_empty(),
            "Dual-hash should reject when dhash exceeds threshold even if phash passes"
        );
    }

    #[test]
    fn test_exif_filters_visually_different_members() {
        // 3 photos: same EXIF. Photos 1 and 2 visually similar, photo 3 visually different.
        // Photo 3 should be filtered out.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(100), "2024-01-15 12:00:00", "Canon R5"),
            make_photo_with_exif(2, "bbb", Some(101), "2024-01-15 12:00:00", "Canon R5"),
            make_photo_with_exif(3, "ccc", Some(u64::MAX), "2024-01-15 12:00:00", "Canon R5"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].member_ids.len(), 2, "Visually different member should be filtered");
        assert!(!groups[0].member_ids.contains(&3));
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
        // All photos have close phash so cross-group validation passes
        let photos = vec![
            make_photo(1, "a", Some(100)),
            make_photo(2, "b", Some(101)),
            make_photo(3, "c", Some(102)),
        ];
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

        let merged = merge_overlapping(&mut groups, &photos);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].member_ids.len(), 3);
        assert_eq!(merged[0].confidence, Confidence::High);
    }

    #[test]
    fn test_no_overlap_stays_separate() {
        let photos = vec![
            make_photo(1, "a", Some(100)),
            make_photo(2, "b", Some(101)),
            make_photo(3, "c", Some(200)),
            make_photo(4, "d", Some(201)),
        ];
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

        let merged = merge_overlapping(&mut groups, &photos);
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
        // All photos visually similar (close phash) so cross-group validation passes
        let photos = vec![
            make_photo(1, "a", Some(100)),
            make_photo(2, "b", Some(101)),
            make_photo(3, "c", Some(102)),
            make_photo(4, "d", Some(100)),
        ];
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

        let merged = merge_overlapping(&mut groups, &photos);
        assert_eq!(merged.len(), 1, "Transitive chain should collapse to 1 group");
        assert_eq!(merged[0].member_ids.len(), 4);
        assert_eq!(merged[0].confidence, Confidence::High, "Worst confidence wins");
    }

    #[test]
    fn test_transitive_merge_bridge_two_disjoint_groups() {
        let photos = vec![
            make_photo(1, "a", Some(100)),
            make_photo(2, "b", Some(101)),
            make_photo(3, "c", Some(102)),
            make_photo(4, "d", Some(100)),
        ];
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

        let merged = merge_overlapping(&mut groups, &photos);
        assert_eq!(merged.len(), 1, "Bridge group should merge the two disjoint groups");
        assert_eq!(merged[0].member_ids.len(), 4);
    }

    #[test]
    fn test_transitive_merge_multiple_bridges() {
        let photos = vec![
            make_photo(1, "a", Some(100)),
            make_photo(2, "b", Some(101)),
            make_photo(3, "c", Some(100)),
            make_photo(4, "d", Some(101)),
            make_photo(5, "e", Some(100)),
            make_photo(6, "f", Some(101)),
        ];
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

        let merged = merge_overlapping(&mut groups, &photos);
        assert_eq!(merged.len(), 1, "Single bridge touching all groups should merge everything");
        assert_eq!(merged[0].member_ids.len(), 6);
        assert_eq!(merged[0].confidence, Confidence::Probable);
    }

    #[test]
    fn test_merge_preserves_independent_groups() {
        let photos = vec![
            make_photo(1, "a", Some(100)),
            make_photo(2, "b", Some(101)),
            make_photo(3, "c", Some(102)),
            make_photo(10, "d", Some(200)),
            make_photo(11, "e", Some(201)),
            make_photo(12, "f", Some(202)),
        ];
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

        let merged = merge_overlapping(&mut groups, &photos);
        assert_eq!(merged.len(), 2, "Two independent chains should stay separate");
    }

    #[test]
    fn test_merge_rejects_visually_unrelated_groups() {
        // Groups {1,2} and {2,3} share member 2, but exclusive members 1 and 3
        // have distant phash → should NOT merge
        let photos = vec![
            make_photo(1, "a", Some(100)),        // close to 2
            make_photo(2, "b", Some(101)),        // bridge
            make_photo(3, "c", Some(u64::MAX)),   // far from 1
        ];
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

        let merged = merge_overlapping(&mut groups, &photos);
        assert_eq!(merged.len(), 2, "Visually unrelated groups should NOT merge");
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
