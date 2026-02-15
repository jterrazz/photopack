# Photopack

Pack your photo library tight. Rust-powered photo deduplication engine.

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
```

## Architecture

- `crates/core` — library: domain types, catalog (SQLite), scanner, hasher, EXIF, matching, ranking, manifest, export
- `crates/cli` — binary (`photopack`): CLI interface using clap

## Conventions

- Rust 2021 edition
- Use `thiserror` in core, `anyhow` in CLI
- SQLite via `rusqlite` with WAL mode
- All public API goes through the `Vault` struct in `lib.rs`
- `rusqlite::Connection` is not `Sync` — DB access must be separated from `rayon` parallel sections

## Key Design Decisions

- **Perceptual hashing gate**: Only JPEG, PNG, TIFF, WebP support perceptual hashing (`PhotoFormat::supports_perceptual_hash()`). HEIC and RAW formats skip it to avoid decoder hangs. These formats are still indexed by SHA-256 and EXIF.
- **Perceptual hash pipeline**: JPEG path: `turbojpeg` full-resolution GRAY format (skips chroma, 1 byte/pixel) → EXIF orientation → SIMD resize to 9x8. Non-JPEG path: `image` crate → EXIF orientation → RGB resize to 9x8 → manual BT.601 on 72 pixels. Both produce a 9x8 grayscale buffer for manual aHash + dHash. No `img_hash` dependency. `turbojpeg` is feature-gated (default on, disable with `--no-default-features` for WASM). EXIF orientation is applied before resize (iPhone originals store landscape pixels with rotation tag; exports physically rotate). Full-resolution decode is critical — DCT scaling causes hash divergence. Phash version tracking (`PHASH_VERSION` constant) auto-invalidates cached hashes when the algorithm changes.
- **Dual-hash consensus**: Matching requires both aHash (stored as `phash`) and dHash to be within threshold. When one hash is missing (cross-format), phash-only match requires stricter HIGH threshold. This dramatically reduces false positives.
- **Perceptual hash thresholds**: NearCertain ≤2, High ≤2, Probable ≤3 bits (out of 64). Super-safe: true cross-format duplicates have distance 0-2, different photos have distance 3+.
- **Phase 3 cross-format matching**: Ungrouped photos are compared against ALL photos (including already-grouped ones) via BK-tree. This enables cross-format duplicate detection when one variant is already in a SHA-256 group.
- **EXIF matching filters burst shots**: Phase 2 uses perceptual hash as a strict filter (NEAR_CERTAIN threshold ≤2). Sequential/burst shots (distance 3+) are rejected. Members without phash (HEIC/RAW) are kept.
- **Sequential shot filter (Phase 3)**: Photos from the same camera with EXIF dates 1-60 seconds apart (but not identical) are rejected as sequential/burst shots. True duplicates always have identical EXIF dates. This prevents false positives from visually similar but distinct consecutive photos that produce identical hashes at 9x8 resolution.
- **Merge safeguards**: Phase 4 requires cross-group visual validation before merging overlapping groups. At least one pair of exclusive members must be perceptually close. Prevents cascading false merges through bridge photos.
- **Content-addressable pack**: Pack files are named by SHA-256 hash with 2-char prefix sharding (`{hash[..2]}/{hash}.{ext}`). Deduplication is structural (same hash = same file). An embedded manifest at `.photopack/manifest.sqlite` maps hashes to metadata (original filename, format, size, EXIF). Cleanup removes entries not in the desired hash set. No collision handling needed — hash uniqueness is guaranteed.
- **Pack auto-registers as source**: `set_vault_path` automatically registers the pack directory as a scan source (idempotent).
- **Pack quality upgrade**: When a higher-quality format becomes SOT, the new format's hash-named file is written to the pack. Stale entries (hashes no longer in the desired set) are cleaned up via the manifest.
- **Two-phase hashing**: Scan computes SHA-256 + EXIF first (fast, I/O-bound), then perceptual hashes only for unique SHA-256 content. Exact duplicates skip image decoding entirely; existing catalog hashes are reused. Batch mtime check replaces per-file queries.
- **Incremental scan**: Files are skipped if their mtime hasn't changed. Files deleted from disk are removed from the catalog (`remove_photos_by_paths`). Groups are rebuilt from scratch each scan.
- **HEIC export via sips**: Uses macOS `sips` command for HEIC conversion (zero dependencies). Invoked via `photopack export`, independent from lossless vault sync. Reads from catalog (source directories), not the vault. Skip by file existence (not size, since conversion changes size). `#[cfg(target_os = "macos")]` gates for e2e tests.
- **Flat CLI structure**: All commands are top-level (`add`, `rm`, `scan`, `status`, `ls`, `pack`, `export`). `ls` = file listing (default) or duplicate groups (`--dupes`). `pack` = permanent lossless archive (best-quality originals, persists across source removal, path persisted in catalog). `export` = compressed HEIC output for space savings (path required each invocation).
- **Schema versioning**: Catalog DB tracks `schema_version` in the config table (current: 1). On open, `schema::migrate()` runs pending migrations in a transaction. If the DB version is higher than the code version, open fails with `SchemaTooNew`. To add a migration: increment `SCHEMA_VERSION`, write a `migrate_vN_to_vM()` function, append it to the `MIGRATIONS` array. Pre-versioning databases auto-upgrade to v1.

## Testing

- 384 tests total (28 CLI + 234 core + 122 e2e) — counts may vary after refactoring
- E2E tests in `crates/core/tests/vault_e2e.rs` use real JPEG/PNG generation via the `image` crate
- Cross-format testing: use `create_file_with_jpeg_bytes()` to write JPEG bytes to `.cr2`/`.heic`/`.dng` etc. — scanner assigns format from extension, hashes work on raw bytes
- Use structurally different patterns (gradient vs checkerboard vs stripes) in tests to ensure distinct perceptual hashes — color-only differences are not enough
- `tempfile` crate for isolated test directories
- CLI status tests: extracted testable logic (StatusData, compute_aggregates, etc.) for unit testing without stdout capture
- Pack sync tests cover: date parsing, content-addressable paths, incremental skip, cross-format dedup, progress events, error cases, file content preservation, manifest integration, cleanup
- Quality preservation tests: all format tier combinations (CR2>JPEG, DNG>JPEG, CR2>HEIC, TIFF>JPEG, PNG>HEIC, JPEG>HEIC), vault as source preserves RAW, vault replaces lower-quality with higher-quality on resync
- HEIC export tests: macOS-only tests gated with `#[cfg(target_os = "macos")]`, cross-platform config/error tests run everywhere

### Test Coverage

Tests across 28 CLI + core library + end-to-end:

- **Matching** (104 tests) — All 4 phases individually and combined. Sequential shot filter (burst detection, boundary cases, cross-format interaction, mixed with true duplicates). Dual-hash consensus (accept/reject matrix). EXIF filtering (camera model, date presence, phash validation). Cross-format grouping (HEIC/RAW without phash, HIGH threshold). BK-tree distance thresholds. Merge safeguards (cross-group visual validation, pure subsets, bridge photos). Transitive merge chains. Full pipeline scenarios (iPhone original+export+HEIC triplets, recompressed JPEGs, renamed files, 10-photo batch, sequential shots among true duplicates).
- **CLI dashboard** (28 tests) — format_size, source_display_name, StatusData, is_duplicate, vault_eligible, compute_aggregates, compute_source_stats, sort_photos_for_display
- **Catalog** (39 tests) — CRUD operations, format/confidence roundtrip, mtime tracking, config persistence, source removal, perceptual hash invalidation, schema versioning, schema structure pinning, data integrity (FK enforcement, reopen persistence)
- **Vault sync** (20 tests) — Date parsing, EXIF/mtime fallback, photo selection, content-addressable paths, incremental copy, manifest tests
- **Manifest** (9 tests) — Open/create, version, insert/contains, remove, list, idempotent insert, schema structure pinning (tables, columns), data reopen persistence
- **Export** (21 tests) — build_export_path (all format extensions, collision, skip, no-extension), export_photo_to_heic (skip/convert), convert_to_heic (parent dirs, invalid source, output validation, quality effect), sips availability
- **Perceptual hash** (17 tests) — Hamming distance, manual aHash/dHash computation, real JPEG/PNG hashing, EXIF orientation (identity, 90 CW, 180, 90 CCW)
- **Scanner** (11 tests) — Directory walk, format filtering, nested directories (deep nesting, multiple levels, siblings, symlinks)
- **Domain** (6 tests) — Quality tiers, format extension, format support, confidence ordering
- **EXIF** (5 tests) — Edge cases, missing data, non-image files
- **SHA-256** (4 tests) — Consistency, empty files, error handling
- **Ranking** (3 tests) — Format preference, size tiebreak, mtime tiebreak
- **Confidence** (2 tests) — Hamming distance to confidence mapping, confidence combination
- **E2E** (122 tests) — Full vault lifecycle, cross-directory and cross-format duplicates, incremental scan, source-of-truth election, source removal (with group cleanup), pack auto-registration as source, photos API, quality preservation (all format tier combinations: CR2>JPEG, DNG>JPEG, CR2>HEIC, TIFF>JPEG, PNG>HEIC, JPEG>HEIC, pack as source preserves RAW, pack replaces lower-quality on resync), nested directories (multi-level, cross-source, incremental), stale file cleanup (deleted files, all deleted, cross-source, group member removal), phash version tracking (cache invalidation, mtime reset, recomputation), pack sync (content-addressable structure, SHA-256 integrity, manifest metadata, hash dedup, cleanup of stale entries, deduplication, incremental skip, progress events, error cases, file integrity), HEIC export (JPEG/PNG conversion, multi-source, nested dirs, dedup, cross-source dedup, incremental skip+rescan, independent from pack sync, progress events, error handling, file validity)
