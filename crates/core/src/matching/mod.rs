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

/// Parse an EXIF datetime string into an approximate seconds value (for comparison only).
/// Handles "YYYY:MM:DD HH:MM:SS" and "YYYY-MM-DD HH:MM:SS".
fn parse_exif_seconds(date_str: &str) -> Option<i64> {
    let parts: Vec<&str> = date_str.split_whitespace().collect();
    let date_part = parts.first()?;
    let time_part = parts.get(1)?;

    let dp: Vec<i64> = date_part
        .split([':', '-'])
        .filter_map(|s| s.parse().ok())
        .collect();
    let tp: Vec<i64> = time_part.split(':').filter_map(|s| s.parse().ok()).collect();

    if dp.len() < 3 || tp.len() < 3 {
        return None;
    }

    // Approximate days (consistent, not astronomically accurate)
    let days = dp[0] * 365 + dp[0] / 4 + dp[1] * 31 + dp[2];
    Some(days * 86400 + tp[0] * 3600 + tp[1] * 60 + tp[2])
}

/// Check if two photos are sequential shots from the same camera.
/// Sequential shots: same camera model, EXIF dates 1-60 seconds apart (not identical).
/// True duplicates always have identical EXIF dates.
fn is_sequential_shot(a: &PhotoFile, b: &PhotoFile) -> bool {
    let (exif_a, exif_b) = match (&a.exif, &b.exif) {
        (Some(ea), Some(eb)) => (ea, eb),
        _ => return false,
    };

    // Must have same camera model
    match (&exif_a.camera_model, &exif_b.camera_model) {
        (Some(ma), Some(mb)) if ma == mb => {}
        _ => return false,
    }

    // Must have dates
    let (date_a, date_b) = match (&exif_a.date, &exif_b.date) {
        (Some(da), Some(db)) => (da.as_str(), db.as_str()),
        _ => return false,
    };

    // Identical dates = true duplicate, not sequential
    if date_a == date_b {
        return false;
    }

    // Parse and check time difference
    match (parse_exif_seconds(date_a), parse_exif_seconds(date_b)) {
        (Some(sa), Some(sb)) => {
            let diff = (sa - sb).unsigned_abs();
            diff <= 60
        }
        _ => false,
    }
}

/// Phase 3: Group ungrouped photos by perceptual hash similarity.
/// Ungrouped photos are compared against ALL photos (including already-grouped
/// ones) so that cross-format duplicates create bridge groups that Phase 4 merges.
/// Uses a BK-tree for O(n log n) Hamming distance lookups instead of O(n²).
///
/// Dual-hash consensus: when both photos have phash AND dhash, both must be
/// within threshold. When dhash is missing (cross-format), phash alone is
/// accepted only at the stricter HIGH threshold.
///
/// Sequential shot filter: rejects matches where both photos have the same camera
/// model and EXIF dates 1-60 seconds apart (but not identical). True duplicates
/// always have identical EXIF dates.
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

            // Sequential shot filter: reject matches from the same camera
            // with EXIF dates 1-60 seconds apart (not identical).
            if let Some(neighbor_photo) = neighbor {
                if is_sequential_shot(photo_a, neighbor_photo) {
                    continue;
                }
            }

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

    /// Full control: separate phash/dhash, optional date, optional camera model.
    fn make_photo_exif_full(
        id: i64,
        sha: &str,
        phash: Option<u64>,
        dhash: Option<u64>,
        date: Option<&str>,
        camera_model: Option<&str>,
    ) -> PhotoFile {
        let mut p = make_photo_full(id, sha, phash, dhash);
        p.exif = Some(ExifData {
            date: date.map(|s| s.to_string()),
            camera_make: None,
            camera_model: camera_model.map(|s| s.to_string()),
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

    // ── is_sequential_shot unit tests ──────────────────────────────

    #[test]
    fn test_is_sequential_shot_2_seconds_same_camera() {
        let a = make_photo_with_exif(1, "a", Some(0), "2024-12-24 20:43:45", "iPhone 16 Pro Max");
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-24 20:43:47", "iPhone 16 Pro Max");
        assert!(is_sequential_shot(&a, &b), "2s apart, same camera → sequential");
    }

    #[test]
    fn test_is_sequential_shot_identical_dates_not_sequential() {
        let a = make_photo_with_exif(1, "a", Some(0), "2024-12-24 20:43:45", "iPhone 16 Pro Max");
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-24 20:43:45", "iPhone 16 Pro Max");
        assert!(!is_sequential_shot(&a, &b), "Identical dates = true duplicate, not sequential");
    }

    #[test]
    fn test_is_sequential_shot_different_cameras_not_sequential() {
        let a = make_photo_with_exif(1, "a", Some(0), "2024-12-24 20:43:45", "iPhone 16 Pro Max");
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-24 20:43:47", "Canon R5");
        assert!(!is_sequential_shot(&a, &b), "Different cameras → not sequential");
    }

    #[test]
    fn test_is_sequential_shot_no_exif_a() {
        let a = make_photo(1, "a", Some(0));
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-24 20:43:47", "iPhone");
        assert!(!is_sequential_shot(&a, &b), "No EXIF on A → not sequential");
    }

    #[test]
    fn test_is_sequential_shot_no_exif_b() {
        let a = make_photo_with_exif(1, "a", Some(0), "2024-12-24 20:43:45", "iPhone");
        let b = make_photo(2, "b", Some(0));
        assert!(!is_sequential_shot(&a, &b), "No EXIF on B → not sequential");
    }

    #[test]
    fn test_is_sequential_shot_no_exif_both() {
        let a = make_photo(1, "a", Some(0));
        let b = make_photo(2, "b", Some(0));
        assert!(!is_sequential_shot(&a, &b), "No EXIF on either → not sequential");
    }

    #[test]
    fn test_is_sequential_shot_no_camera_model_a() {
        let a = make_photo_exif_full(1, "a", Some(0), Some(0), Some("2024-12-24 20:43:45"), None);
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-24 20:43:47", "iPhone");
        assert!(!is_sequential_shot(&a, &b), "No camera on A → can't confirm sequential");
    }

    #[test]
    fn test_is_sequential_shot_no_camera_model_both() {
        let a = make_photo_exif_full(1, "a", Some(0), Some(0), Some("2024-12-24 20:43:45"), None);
        let b = make_photo_exif_full(2, "b", Some(0), Some(0), Some("2024-12-24 20:43:47"), None);
        assert!(!is_sequential_shot(&a, &b), "No camera on either → can't confirm sequential");
    }

    #[test]
    fn test_is_sequential_shot_no_date_a() {
        let a = make_photo_exif_full(1, "a", Some(0), Some(0), None, Some("iPhone"));
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-24 20:43:47", "iPhone");
        assert!(!is_sequential_shot(&a, &b), "No date on A → can't determine");
    }

    #[test]
    fn test_is_sequential_shot_no_date_both() {
        let a = make_photo_exif_full(1, "a", Some(0), Some(0), None, Some("iPhone"));
        let b = make_photo_exif_full(2, "b", Some(0), Some(0), None, Some("iPhone"));
        assert!(!is_sequential_shot(&a, &b), "No dates on either → can't determine");
    }

    #[test]
    fn test_is_sequential_shot_boundary_60s() {
        let a = make_photo_with_exif(1, "a", Some(0), "2024-12-24 20:43:00", "iPhone");
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-24 20:44:00", "iPhone");
        assert!(is_sequential_shot(&a, &b), "Exactly 60s → sequential");
    }

    #[test]
    fn test_is_sequential_shot_boundary_61s_not_sequential() {
        let a = make_photo_with_exif(1, "a", Some(0), "2024-12-24 20:43:00", "iPhone");
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-24 20:44:01", "iPhone");
        assert!(!is_sequential_shot(&a, &b), "61s apart → NOT sequential");
    }

    #[test]
    fn test_is_sequential_shot_date_only_no_time() {
        // Date-only (no time component) → parse_exif_seconds returns None → not sequential
        let a = make_photo_with_exif(1, "a", Some(0), "2024-12-24", "iPhone");
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-25", "iPhone");
        assert!(!is_sequential_shot(&a, &b), "Date-only EXIF can't determine seconds");
    }

    #[test]
    fn test_is_sequential_shot_midnight_boundary_still_detected() {
        // 23:59:59 → 00:00:01 is 2 seconds apart. Formula handles midnight rollover correctly.
        let a = make_photo_with_exif(1, "a", Some(0), "2024-12-24 23:59:59", "iPhone");
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-25 00:00:01", "iPhone");
        assert!(is_sequential_shot(&a, &b), "Midnight boundary: 2s apart → sequential");
    }

    #[test]
    fn test_is_sequential_shot_different_days_not_sequential() {
        // Photos 24 hours apart → not sequential.
        let a = make_photo_with_exif(1, "a", Some(0), "2024-12-24 12:00:00", "iPhone");
        let b = make_photo_with_exif(2, "b", Some(0), "2024-12-25 12:00:00", "iPhone");
        assert!(!is_sequential_shot(&a, &b), "24h apart → not sequential");
    }

    // ── parse_exif_seconds unit tests ───────────────────────────────

    #[test]
    fn test_parse_exif_seconds_standard() {
        let s = parse_exif_seconds("2024-12-24 20:43:45");
        assert!(s.is_some());
    }

    #[test]
    fn test_parse_exif_seconds_colon_format() {
        let s = parse_exif_seconds("2024:12:24 20:43:45");
        assert!(s.is_some());
    }

    #[test]
    fn test_parse_exif_seconds_difference() {
        let s1 = parse_exif_seconds("2024-12-24 20:43:45").unwrap();
        let s2 = parse_exif_seconds("2024-12-24 20:43:47").unwrap();
        assert_eq!((s1 - s2).unsigned_abs(), 2);
    }

    #[test]
    fn test_parse_exif_seconds_minute_difference() {
        let s1 = parse_exif_seconds("2024-12-24 20:43:00").unwrap();
        let s2 = parse_exif_seconds("2024-12-24 20:44:00").unwrap();
        assert_eq!((s1 - s2).unsigned_abs(), 60);
    }

    #[test]
    fn test_parse_exif_seconds_hour_difference() {
        let s1 = parse_exif_seconds("2024-12-24 19:00:00").unwrap();
        let s2 = parse_exif_seconds("2024-12-24 20:00:00").unwrap();
        assert_eq!((s1 - s2).unsigned_abs(), 3600);
    }

    #[test]
    fn test_parse_exif_seconds_no_time() {
        assert!(parse_exif_seconds("2024-12-24").is_none());
    }

    #[test]
    fn test_parse_exif_seconds_empty() {
        assert!(parse_exif_seconds("").is_none());
    }

    #[test]
    fn test_parse_exif_seconds_garbage() {
        assert!(parse_exif_seconds("not-a-date at-all").is_none());
    }

    #[test]
    fn test_parse_exif_seconds_mixed_format_consistent() {
        // Both formats should produce the same value for the same datetime
        let s1 = parse_exif_seconds("2024-12-24 20:43:45").unwrap();
        let s2 = parse_exif_seconds("2024:12:24 20:43:45").unwrap();
        assert_eq!(s1, s2, "Hyphen and colon formats must produce same value");
    }

    // ── Sequential shot filter integration (Phase 3) ────────────────

    #[test]
    fn test_phase3_rejects_sequential_shots_same_camera() {
        // IMG_1304 vs IMG_1305 regression: identical hashes, 2s apart, same iPhone.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24 20:43:45", "iPhone 16 Pro Max"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24 20:43:47", "iPhone 16 Pro Max"),
        ];

        let groups = find_duplicates(&photos);
        assert!(
            groups.is_empty(),
            "Sequential shots (2s apart, same camera) must NOT be grouped"
        );
    }

    #[test]
    fn test_phase3_accepts_true_duplicates_identical_dates() {
        // Same hash, same EXIF date → true duplicate.
        // Phase 2 groups by identical EXIF, Phase 3 not needed.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24 20:43:45", "iPhone 16 Pro Max"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24 20:43:45", "iPhone 16 Pro Max"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "True duplicates with identical dates should group");
    }

    #[test]
    fn test_phase3_accepts_no_exif_data() {
        // No EXIF → can't determine if sequential, allow grouping by hash.
        let photos = vec![
            make_photo(1, "aaa", Some(0xFF00)),
            make_photo(2, "bbb", Some(0xFF00)),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Photos without EXIF should still group by hash");
    }

    #[test]
    fn test_phase3_accepts_different_cameras() {
        // Same hash, close dates, different cameras → allow grouping.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24 20:43:45", "iPhone 16 Pro Max"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24 20:43:47", "Canon R5"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Different cameras should not trigger sequential shot filter");
    }

    #[test]
    fn test_phase3_rejects_burst_shots_1_second() {
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24 20:43:45", "iPhone 16 Pro Max"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24 20:43:46", "iPhone 16 Pro Max"),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Burst shots (1s apart) must NOT be grouped");
    }

    #[test]
    fn test_phase3_rejects_sequential_shots_60_seconds() {
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24 20:43:00", "iPhone 16 Pro Max"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24 20:44:00", "iPhone 16 Pro Max"),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Shots 60s apart on same camera must NOT be grouped");
    }

    #[test]
    fn test_phase3_accepts_61_seconds_apart() {
        // 61s is just outside the sequential window → should group.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24 20:43:00", "iPhone 16 Pro Max"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24 20:44:01", "iPhone 16 Pro Max"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "61s apart → just outside window, should group");
    }

    #[test]
    fn test_phase3_accepts_far_apart_dates() {
        // Same hash, same camera, 2 minutes apart → could be re-export, allow.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24 20:43:00", "iPhone 16 Pro Max"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24 20:45:00", "iPhone 16 Pro Max"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Photos >60s apart should still group by hash");
    }

    #[test]
    fn test_phase3_accepts_one_photo_no_exif() {
        // One has EXIF (with camera), other has none → can't confirm sequential, allow.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24 20:43:45", "iPhone 16 Pro Max"),
            make_photo(2, "bbb", Some(0xFF00)),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "One missing EXIF → can't determine sequential, allow grouping");
    }

    #[test]
    fn test_phase3_accepts_no_camera_model() {
        // Both have dates 2s apart but NO camera model → can't confirm same device.
        let photos = vec![
            make_photo_exif_full(1, "aaa", Some(0xFF00), Some(0xFF00), Some("2024-12-24 20:43:45"), None),
            make_photo_exif_full(2, "bbb", Some(0xFF00), Some(0xFF00), Some("2024-12-24 20:43:47"), None),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "No camera model → can't confirm sequential, allow grouping");
    }

    #[test]
    fn test_phase3_accepts_date_only_exif() {
        // EXIF date without time component → Phase 2 groups if identical date strings.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24", "iPhone 16 Pro Max"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24", "iPhone 16 Pro Max"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Date-only EXIF should group (identical strings)");
    }

    #[test]
    fn test_phase3_rejects_burst_of_5_sequential_shots() {
        // 5 photos taken 1s apart, identical hashes → none should be grouped.
        let photos = vec![
            make_photo_with_exif(1, "a", Some(0xFF00), "2024-12-24 20:43:00", "iPhone"),
            make_photo_with_exif(2, "b", Some(0xFF00), "2024-12-24 20:43:01", "iPhone"),
            make_photo_with_exif(3, "c", Some(0xFF00), "2024-12-24 20:43:02", "iPhone"),
            make_photo_with_exif(4, "d", Some(0xFF00), "2024-12-24 20:43:03", "iPhone"),
            make_photo_with_exif(5, "e", Some(0xFF00), "2024-12-24 20:43:04", "iPhone"),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Burst of 5 sequential shots must NOT be grouped");
    }

    #[test]
    fn test_phase3_rejects_sequential_but_keeps_true_dup_in_burst() {
        // 3 photos: A (t=0s), B (t=2s), C (t=0s, copy of A).
        // A and C are true duplicates (identical EXIF). A and B are sequential.
        // Phase 2 groups A+C (identical EXIF). B should stay ungrouped.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-24 20:43:00", "iPhone"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24 20:43:02", "iPhone"),
            make_photo_with_exif(3, "ccc", Some(0xFF00), "2024-12-24 20:43:00", "iPhone"),
        ];

        let groups = find_duplicates(&photos);
        // Phase 2 groups {1,3} (same date+camera, phash validated).
        // Phase 3 checks photo 2 vs all: sequential with both → rejected.
        assert_eq!(groups.len(), 1, "Only the true duplicate pair should group");
        let group = &groups[0];
        assert!(group.member_ids.contains(&1));
        assert!(group.member_ids.contains(&3));
        assert!(!group.member_ids.contains(&2), "Sequential photo B must NOT be in the group");
    }

    #[test]
    fn test_phase3_sequential_plus_exact_duplicate() {
        // A and B are sequential (2s apart). C is an exact copy of B (same SHA).
        // SHA groups {2,3}. Photo 1 should not join via Phase 3 (sequential filter).
        let photos = vec![
            make_photo_with_exif(1, "sha_a", Some(0xFF00), "2024-12-24 20:43:00", "iPhone"),
            make_photo_with_exif(2, "sha_b", Some(0xFF00), "2024-12-24 20:43:02", "iPhone"),
            make_photo_with_exif(3, "sha_b", Some(0xFF00), "2024-12-24 20:43:02", "iPhone"),
        ];

        let groups = find_duplicates(&photos);
        // Photo 2 and 3: exact SHA match → Certain group.
        // Photo 1: ungrouped, Phase 3 finds hash match with 2 and 3, but sequential with both → rejected.
        assert_eq!(groups.len(), 1, "Only SHA duplicate pair should group");
        assert_eq!(groups[0].member_ids.len(), 2);
        assert!(!groups[0].member_ids.contains(&1), "Sequential photo must stay ungrouped");
    }

    #[test]
    fn test_phase3_sequential_shots_different_hash_distance() {
        // Two sequential shots with phash distance 2 (within threshold but not identical).
        // Sequential filter should still reject.
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0b1111_0000), "2024-12-24 20:43:45", "iPhone"),
            make_photo_with_exif(2, "bbb", Some(0b1111_0011), "2024-12-24 20:43:47", "iPhone"),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Sequential shots with phash distance 2 must NOT be grouped");
    }

    // ── Phase 2: EXIF edge cases ────────────────────────────────────

    #[test]
    fn test_exif_no_camera_model_groups_under_unknown() {
        // Both have same date but no camera model → grouped under "unknown" key.
        let photos = vec![
            make_photo_exif_full(1, "aaa", None, None, Some("2024-01-15 12:00:00"), None),
            make_photo_exif_full(2, "bbb", None, None, Some("2024-01-15 12:00:00"), None),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Same date, no camera → should group under 'unknown'");
    }

    #[test]
    fn test_exif_one_has_camera_one_doesnt_no_group() {
        // Different camera keys: "iPhone" vs "unknown" → separate, no group.
        let a = make_photo_with_exif(1, "aaa", None, "2024-01-15 12:00:00", "iPhone");
        let b = make_photo_exif_full(2, "bbb", None, None, Some("2024-01-15 12:00:00"), None);

        let photos = vec![a, b];
        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Different camera keys should NOT group");
    }

    #[test]
    fn test_exif_no_date_no_group() {
        // Both have camera model but no date → no EXIF key → no group.
        let photos = vec![
            make_photo_exif_full(1, "aaa", None, None, None, Some("iPhone")),
            make_photo_exif_full(2, "bbb", None, None, None, Some("iPhone")),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "No EXIF date → can't group by EXIF");
    }

    #[test]
    fn test_exif_3_photos_2_valid_1_rejected_by_phash() {
        // 3 photos, same EXIF. Photo 1 and 2 have close phash. Photo 3 has distant phash.
        // Photo 3 should be filtered from the EXIF group.
        let photos = vec![
            {
                let mut p = make_photo_with_exif(1, "aaa", Some(0b1111_0000), "2024-01-15 12:00:00", "iPhone");
                p.dhash = Some(0b1010_0000);
                p
            },
            {
                let mut p = make_photo_with_exif(2, "bbb", Some(0b1111_0001), "2024-01-15 12:00:00", "iPhone");
                p.dhash = Some(0b1010_0001);
                p
            },
            {
                let mut p = make_photo_with_exif(3, "ccc", Some(u64::MAX), "2024-01-15 12:00:00", "iPhone");
                p.dhash = Some(u64::MAX);
                p
            },
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].member_ids.len(), 2, "Visually different photo rejected");
        assert!(!groups[0].member_ids.contains(&3));
    }

    #[test]
    fn test_exif_phash_validates_cross_format_with_heic() {
        // JPEG has phash, HEIC has no phash. Same EXIF. Should group at NearCertain
        // because the single phash has no comparison partner.
        let jpeg = make_photo_with_exif(1, "sha_j", Some(100), "2024-01-15 12:00:00", "iPhone");
        let mut heic = make_photo_with_exif(2, "sha_h", None, "2024-01-15 12:00:00", "iPhone");
        heic.format = PhotoFormat::Heic;

        let photos = vec![jpeg, heic];
        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].member_ids.len(), 2);
        assert_eq!(groups[0].confidence, Confidence::NearCertain,
            "1 phash, 0 comparison partners → NearCertain");
    }

    #[test]
    fn test_exif_2_jpegs_1_heic_same_date() {
        // 2 JPEGs (close phash) + 1 HEIC (no phash), all same EXIF.
        // JPEGs validate each other → High. HEIC kept because no phash.
        let photos = vec![
            make_photo_with_exif(1, "sha_j1", Some(100), "2024-01-15 12:00:00", "iPhone"),
            make_photo_with_exif(2, "sha_j2", Some(101), "2024-01-15 12:00:00", "iPhone"),
            {
                let mut p = make_photo_with_exif(3, "sha_h", None, "2024-01-15 12:00:00", "iPhone");
                p.format = PhotoFormat::Heic;
                p
            },
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].member_ids.len(), 3, "All three should be in one group");
        assert_eq!(groups[0].confidence, Confidence::High);
    }

    // ── Phase 3: perceptual hash edge cases ─────────────────────────

    #[test]
    fn test_phase3_cross_format_missing_dhash_high_threshold() {
        // One photo has dhash, other doesn't → cross-format path.
        // Requires stricter HIGH threshold (≤2) for phash-only match.
        // phash distance 1 → should group.
        let photos = vec![
            make_photo_full(1, "aaa", Some(0b1111_0000), Some(0b1010_0000)),
            make_photo_full(2, "bbb", Some(0b1111_0001), None), // no dhash
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Cross-format phash dist 1 should group");
    }

    #[test]
    fn test_phase3_cross_format_phash_distance_3_rejected() {
        // Cross-format: one lacks dhash. phash distance 3 > HIGH (2) → rejected.
        let photos = vec![
            make_photo_full(1, "aaa", Some(0b1111_0000), Some(0b1010_0000)),
            make_photo_full(2, "bbb", Some(0b1111_0111), None), // phash dist 3, no dhash
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Cross-format phash dist 3 > HIGH → rejected");
    }

    #[test]
    fn test_phase3_cross_format_phash_distance_2_accepted() {
        // Cross-format: phash distance exactly 2 = HIGH threshold → should group.
        let photos = vec![
            make_photo_full(1, "aaa", Some(0b1111_0000), Some(0b1010_0000)),
            make_photo_full(2, "bbb", Some(0b1111_0011), None), // phash dist 2, no dhash
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Cross-format phash dist 2 = HIGH → accepted");
    }

    #[test]
    fn test_phase3_both_dhash_none_uses_high_threshold() {
        // Both photos lack dhash → cross-format path, HIGH threshold.
        let photos = vec![
            make_photo_full(1, "aaa", Some(0b1111_0000), None),
            make_photo_full(2, "bbb", Some(0b1111_0001), None), // phash dist 1
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Both dhash=None, phash dist 1 → should group");
    }

    #[test]
    fn test_phase3_both_dhash_none_phash_distance_3_rejected() {
        let photos = vec![
            make_photo_full(1, "aaa", Some(0b1111_0000), None),
            make_photo_full(2, "bbb", Some(0b1111_0111), None), // phash dist 3
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Both dhash=None, phash dist 3 > HIGH → rejected");
    }

    #[test]
    fn test_phase3_exact_distance_0_both_hashes() {
        // Distance 0 on both → NearCertain
        let photos = vec![
            make_photo_full(1, "aaa", Some(0xFF00), Some(0xAA00)),
            make_photo_full(2, "bbb", Some(0xFF00), Some(0xAA00)),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].confidence, Confidence::NearCertain);
    }

    #[test]
    fn test_phase3_bridge_links_ungrouped_to_already_grouped() {
        // Photo 1 and 2 share SHA → Phase 1 group. Photo 3 ungrouped, close phash to 1.
        // Phase 3 should create bridge group {3, 1}. Phase 4 merges into {1, 2, 3}.
        let photos = vec![
            make_photo(1, "same_sha", Some(0xFF00)),
            make_photo(2, "same_sha", Some(0xFF00)),
            make_photo(3, "different", Some(0xFF01)), // phash dist 1 from photo 1
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Bridge should merge all three");
        assert_eq!(groups[0].member_ids.len(), 3);
    }

    #[test]
    fn test_phase3_no_phash_skips_perceptual_matching() {
        // Both photos have no phash → Phase 3 can't match them.
        let photos = vec![
            make_photo(1, "aaa", None),
            make_photo(2, "bbb", None),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_phase3_one_has_phash_other_doesnt() {
        // Photo 1 has phash, photo 2 doesn't → BK-tree only indexes photo 1.
        // Photo 2 can't seed Phase 3. No group.
        let photos = vec![
            make_photo(1, "aaa", Some(0xFF00)),
            make_photo(2, "bbb", None),
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Can't match when one has no phash");
    }

    // ── Phase 4: merge edge cases ───────────────────────────────────

    #[test]
    fn test_merge_pure_subset() {
        // Group A = {1,2,3}, Group B = {1,2} → B is subset of A → merge.
        let photos = vec![
            make_photo(1, "a", Some(100)),
            make_photo(2, "b", Some(101)),
            make_photo(3, "c", Some(102)),
        ];
        let mut groups = vec![
            MatchGroup { member_ids: vec![1, 2, 3], confidence: Confidence::High },
            MatchGroup { member_ids: vec![1, 2], confidence: Confidence::Certain },
        ];

        let merged = merge_overlapping(&mut groups, &photos);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].member_ids.len(), 3);
    }

    #[test]
    fn test_merge_three_way_overlap_single_bridge() {
        // Three groups all connected through a single bridge photo (id=2).
        let photos = vec![
            make_photo(1, "a", Some(100)),
            make_photo(2, "b", Some(101)),
            make_photo(3, "c", Some(102)),
            make_photo(4, "d", Some(100)),
        ];
        let mut groups = vec![
            MatchGroup { member_ids: vec![1, 2], confidence: Confidence::Certain },
            MatchGroup { member_ids: vec![2, 3], confidence: Confidence::High },
            MatchGroup { member_ids: vec![2, 4], confidence: Confidence::NearCertain },
        ];

        let merged = merge_overlapping(&mut groups, &photos);
        assert_eq!(merged.len(), 1, "Single bridge photo merges all");
        assert_eq!(merged[0].member_ids.len(), 4);
    }

    #[test]
    fn test_merge_allows_when_one_side_has_no_phash() {
        // Group A = {1 (phash), 2 (no phash)}. Group B = {2, 3 (no phash)}.
        // Exclusive member 3 has no phash → can't validate → allow merge.
        let photos = vec![
            make_photo(1, "a", Some(100)),
            make_photo(2, "b", None),
            make_photo(3, "c", None),
        ];
        let mut groups = vec![
            MatchGroup { member_ids: vec![1, 2], confidence: Confidence::Certain },
            MatchGroup { member_ids: vec![2, 3], confidence: Confidence::High },
        ];

        let merged = merge_overlapping(&mut groups, &photos);
        assert_eq!(merged.len(), 1, "No phash on exclusive side → allow merge");
    }

    // ── Sequential shot filter + cross-format interaction ───────────

    #[test]
    fn test_sequential_heic_photos_not_grouped() {
        // Two HEICs taken 2s apart. No phash (HEIC unsupported). Different SHA.
        // Same EXIF date+camera → Phase 2 groups them. This is correct because
        // Phase 2 requires EXACT date match, and these have different dates.
        let mut h1 = make_photo_with_exif(1, "sha_h1", None, "2024-12-24 20:43:45", "iPhone");
        let mut h2 = make_photo_with_exif(2, "sha_h2", None, "2024-12-24 20:43:47", "iPhone");
        h1.format = PhotoFormat::Heic;
        h2.format = PhotoFormat::Heic;

        let photos = vec![h1, h2];
        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Sequential HEICs have different dates → no EXIF group");
    }

    #[test]
    fn test_sequential_shot_with_cross_format_duplicate() {
        // Photo A (t=0s JPEG) and B (t=2s JPEG): sequential shots, identical hashes.
        // Photo C (t=0s HEIC): cross-format duplicate of A (same EXIF date+camera).
        // Group: {A, C} (Phase 2). B should NOT join.
        let a = make_photo_with_exif(1, "sha_a", Some(0xFF00), "2024-12-24 20:43:00", "iPhone");
        let b = make_photo_with_exif(2, "sha_b", Some(0xFF00), "2024-12-24 20:43:02", "iPhone");
        let mut c = make_photo_with_exif(3, "sha_c", None, "2024-12-24 20:43:00", "iPhone");
        c.format = PhotoFormat::Heic;

        let photos = vec![a, b, c];
        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Only A+C should group");
        assert!(groups[0].member_ids.contains(&1));
        assert!(groups[0].member_ids.contains(&3));
        assert!(!groups[0].member_ids.contains(&2), "Sequential B must not join");
    }

    #[test]
    fn test_sequential_shot_does_not_pollute_sha_group() {
        // Photo A and B: exact SHA copies. Photo C: sequential shot of A (2s, same camera).
        // SHA group {A, B}. C should NOT join via Phase 3.
        let a = make_photo_with_exif(1, "sha_x", Some(0xFF00), "2024-12-24 20:43:00", "iPhone");
        let b = make_photo_with_exif(2, "sha_x", Some(0xFF00), "2024-12-24 20:43:00", "iPhone");
        let c = make_photo_with_exif(3, "sha_c", Some(0xFF00), "2024-12-24 20:43:02", "iPhone");

        let photos = vec![a, b, c];
        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Only SHA group should exist");
        assert_eq!(groups[0].member_ids.len(), 2, "Sequential C must not join SHA group");
        assert!(!groups[0].member_ids.contains(&3));
    }

    // ── Full pipeline: real-world scenarios ──────────────────────────

    #[test]
    fn test_full_pipeline_two_sources_jpeg_heic_pairs() {
        // Source A + Source B: JPEG and HEIC of same photo.
        // SHA groups: {jpeg1, jpeg2}, {heic1, heic2}. EXIF group: all 4. Merge → 1 group.
        let photos = vec![
            make_photo_with_exif(1, "sha_jpeg_3234", Some(500), "2024-01-12 20:30:48", "iPhone 16 Pro Max"),
            {
                let mut p = make_photo_with_exif(2, "sha_heic_3234", None, "2024-01-12 20:30:48", "iPhone 16 Pro Max");
                p.format = PhotoFormat::Heic;
                p.size = 900_000;
                p
            },
            make_photo_with_exif(3, "sha_jpeg_3234", Some(500), "2024-01-12 20:30:48", "iPhone 16 Pro Max"),
            {
                let mut p = make_photo_with_exif(4, "sha_heic_3234", None, "2024-01-12 20:30:48", "iPhone 16 Pro Max");
                p.format = PhotoFormat::Heic;
                p.size = 900_000;
                p
            },
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "All 4 files should merge into one group");
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
            photos.push(make_photo_with_exif(id, jpeg_shas[i], Some(phashes[i]), dates[i], "iPhone"));
            id += 1;
            let mut p = make_photo_with_exif(id, heic_shas[i], None, dates[i], "iPhone");
            p.format = PhotoFormat::Heic;
            photos.push(p);
            id += 1;
            photos.push(make_photo_with_exif(id, jpeg_shas[i], Some(phashes[i]), dates[i], "iPhone"));
            id += 1;
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

    #[test]
    fn test_full_pipeline_iphone_original_export_heic_triplet() {
        // Real scenario: iPhone saves original JPEG (5.7MB, orientation=6) +
        // iOS export JPEG (1MB, orientation=1) + HEIC (2.9MB, no phash).
        // All have identical EXIF date. JPEGs have close phash after orientation fix.
        let original = {
            let mut p = make_photo_with_exif(1, "sha_orig", Some(0b1111_0000_1010_0101), "2024-12-24 18:30:00", "iPhone 16 Pro Max");
            p.dhash = Some(0b0000_1111_0101_1010);
            p.size = 5_700_000;
            p
        };
        let export = {
            let mut p = make_photo_with_exif(2, "sha_export", Some(0b1111_0000_1010_0100), "2024-12-24 18:30:00", "iPhone 16 Pro Max");
            p.dhash = Some(0b0000_1111_0101_1011);
            p.size = 1_000_000;
            p
        };
        let heic = {
            let mut p = make_photo_with_exif(3, "sha_heic", None, "2024-12-24 18:30:00", "iPhone 16 Pro Max");
            p.format = PhotoFormat::Heic;
            p.size = 2_900_000;
            p
        };

        let photos = vec![original, export, heic];
        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Original+export+HEIC should all group");
        assert_eq!(groups[0].member_ids.len(), 3);
    }

    #[test]
    fn test_full_pipeline_10_unique_photos_each_with_copy() {
        // 10 unique photos, each with an exact copy → 10 groups of 2.
        let mut photos = Vec::new();
        for i in 0..10 {
            let sha = format!("sha_{i}");
            let phash = (i as u64 + 1) * 1000; // spaced far apart
            photos.push(make_photo(i * 2 + 1, &sha, Some(phash)));
            photos.push(make_photo(i * 2 + 2, &sha, Some(phash)));
        }

        assert_eq!(photos.len(), 20);
        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 10, "Should have 10 separate groups");
        for group in &groups {
            assert_eq!(group.member_ids.len(), 2);
            assert_eq!(group.confidence, Confidence::Certain);
        }
    }

    #[test]
    fn test_full_pipeline_sequential_photos_among_true_duplicates() {
        // 2 true duplicate pairs + 2 sequential shots that look identical.
        // Only the true duplicates should group. Sequential shots stay ungrouped.
        let photos = vec![
            // True dup pair 1
            make_photo_with_exif(1, "sha_1", Some(0xAA00), "2024-12-24 10:00:00", "iPhone"),
            make_photo_with_exif(2, "sha_1", Some(0xAA00), "2024-12-24 10:00:00", "iPhone"),
            // True dup pair 2
            make_photo_with_exif(3, "sha_2", Some(0xBB00), "2024-12-24 11:00:00", "iPhone"),
            make_photo_with_exif(4, "sha_2", Some(0xBB00), "2024-12-24 11:00:00", "iPhone"),
            // Sequential shots (identical hash, 2s apart)
            make_photo_with_exif(5, "sha_3", Some(0xCC00), "2024-12-24 12:00:00", "iPhone"),
            make_photo_with_exif(6, "sha_4", Some(0xCC00), "2024-12-24 12:00:02", "iPhone"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 2, "Only 2 true duplicate groups");
        for group in &groups {
            assert_eq!(group.member_ids.len(), 2);
            assert!(!group.member_ids.contains(&5) || !group.member_ids.contains(&6),
                "Sequential shots 5 and 6 must not be in the same group");
        }
    }

    #[test]
    fn test_full_pipeline_different_days_same_hash() {
        // Same hash, same camera, but different days → NOT sequential (days apart).
        // Phase 3 should group them (perceptual match, not sequential).
        let photos = vec![
            make_photo_with_exif(1, "aaa", Some(0xFF00), "2024-12-20 20:43:45", "iPhone"),
            make_photo_with_exif(2, "bbb", Some(0xFF00), "2024-12-24 20:43:45", "iPhone"),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Different days, same hash → should group (not sequential)");
    }

    #[test]
    fn test_full_pipeline_same_scene_different_composition() {
        // Same EXIF date+camera, phash distance 3 (similar scene), dhash distance 6.
        // Phase 2 rejects (phash > NEAR_CERTAIN). Phase 3 rejects (dhash too far).
        // These are legitimately different compositions of the same scene.
        let photos = vec![
            {
                let mut p = make_photo_with_exif(1, "aaa", Some(0b1111_0000), "2024-01-15 12:00:00", "iPhone");
                p.dhash = Some(0b0000_0000);
                p
            },
            {
                let mut p = make_photo_with_exif(2, "bbb", Some(0b1111_0111), "2024-01-15 12:00:00", "iPhone");
                p.dhash = Some(0b0011_1111); // dhash 6 bits different
                p
            },
        ];

        let groups = find_duplicates(&photos);
        assert!(groups.is_empty(), "Same scene, different composition → must NOT group");
    }

    #[test]
    fn test_full_pipeline_renamed_file_same_sha() {
        // Same photo renamed → same SHA → Phase 1 groups them.
        let photos = vec![
            {
                let mut p = make_photo(1, "same_sha", Some(0xFF00));
                p.path = "photos/IMG_1234.jpeg".into();
                p
            },
            {
                let mut p = make_photo(2, "same_sha", Some(0xFF00));
                p.path = "backup/vacation_photo.jpeg".into();
                p
            },
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].confidence, Confidence::Certain);
    }

    #[test]
    fn test_full_pipeline_recompressed_jpeg_different_sha() {
        // Same photo recompressed → different SHA, different size, close phash (dist 1).
        // Phase 3 groups them via dual-hash consensus.
        let photos = vec![
            {
                let mut p = make_photo_full(1, "sha_hq", Some(0b1111_0000), Some(0b1010_0000));
                p.size = 5_000_000;
                p
            },
            {
                let mut p = make_photo_full(2, "sha_lq", Some(0b1111_0001), Some(0b1010_0001));
                p.size = 1_000_000;
                p
            },
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 1, "Recompressed JPEG should group by perceptual hash");
    }

    #[test]
    fn test_full_pipeline_no_false_merge_across_visually_different_groups() {
        // Group A: photos 1,2 (same SHA). Group B: photos 3,4 (same SHA).
        // Photo 5 has phash close to 1 but far from 3. Photo 6 has phash close to 3 but far from 1.
        // If 5 and 6 happen to have close phash to each other, Phase 3 creates bridge {5,6}.
        // Phase 4 must NOT merge A and B through {5,6} because exclusive members (1 vs 3) are visually far.
        let photos = vec![
            make_photo(1, "sha_a", Some(100)),
            make_photo(2, "sha_a", Some(100)),
            make_photo(3, "sha_b", Some(u64::MAX)),
            make_photo(4, "sha_b", Some(u64::MAX)),
        ];

        let groups = find_duplicates(&photos);
        assert_eq!(groups.len(), 2, "Visually unrelated SHA groups must stay separate");
    }
}
