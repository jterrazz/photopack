# LosslessVault

Rust-powered photo deduplication engine.

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
```

## Architecture

- `crates/core` — library: domain types, catalog (SQLite), scanner, hasher, EXIF, matching, ranking, export
- `crates/cli` — binary (`lsvault`): CLI interface using clap

## Conventions

- Rust 2021 edition
- Use `thiserror` in core, `anyhow` in CLI
- SQLite via `rusqlite` with WAL mode
- All public API goes through the `Vault` struct in `lib.rs`
- `rusqlite::Connection` is not `Sync` — DB access must be separated from `rayon` parallel sections

## Key Design Decisions

- **Perceptual hashing gate**: Only JPEG, PNG, TIFF, WebP support perceptual hashing (`PhotoFormat::supports_perceptual_hash()`). HEIC and RAW formats skip it to avoid decoder hangs. These formats are still indexed by SHA-256 and EXIF.
- **Perceptual hash pipeline**: Hybrid decode (`turbojpeg` for JPEG at full resolution, `image` crate for others) → SIMD resize via `fast_image_resize` to 9x8 grayscale → manual aHash + dHash from same buffer. No `img_hash` dependency. `turbojpeg` is feature-gated (default on, disable with `--no-default-features` for WASM). Full-resolution decode is critical — DCT scaling loses spatial detail needed to distinguish sequential photos.
- **Dual-hash consensus**: Matching requires both aHash (stored as `phash`) and dHash to be within threshold. When one hash is missing (cross-format), phash-only match requires stricter HIGH threshold. This dramatically reduces false positives.
- **Perceptual hash thresholds**: NearCertain ≤2, High ≤2, Probable ≤3 bits (out of 64). Super-safe: true cross-format duplicates have distance 0-2, different photos have distance 3+.
- **Phase 3 cross-format matching**: Ungrouped photos are compared against ALL photos (including already-grouped ones) via BK-tree. This enables cross-format duplicate detection when one variant is already in a SHA-256 group.
- **EXIF matching filters burst shots**: Phase 2 uses perceptual hash as a strict filter (NEAR_CERTAIN threshold ≤2). Sequential/burst shots (distance 3+) are rejected. Members without phash (HEIC/RAW) are kept.
- **Merge safeguards**: Phase 4 requires cross-group visual validation before merging overlapping groups. At least one pair of exclusive members must be perceptually close. Prevents cascading false merges through bridge photos.
- **Vault auto-registers as source**: `set_vault_path` automatically registers the vault directory as a scan source (idempotent).
- **Vault quality upgrade**: During vault sync, superseded vault files (group members in the vault that are NOT the source-of-truth) are automatically removed. This ensures the vault always contains only the highest-quality version.
- **Two-phase hashing**: Scan computes SHA-256 + EXIF first (fast, I/O-bound), then perceptual hashes only for unique SHA-256 content. Exact duplicates skip image decoding entirely; existing catalog hashes are reused. Batch mtime check replaces per-file queries.
- **Incremental scan**: Files are skipped if their mtime hasn't changed. Files deleted from disk are removed from the catalog (`remove_photos_by_paths`). Groups are rebuilt from scratch each scan.
- **HEIC export via sips**: Uses macOS `sips` command for HEIC conversion (zero dependencies). Export is a top-level CLI command (`lsvault export`), independent from vault. Reads from catalog (source directories), not the vault. Skip by file existence (not size, since conversion changes size). `#[cfg(target_os = "macos")]` gates for e2e tests.

## Testing

- 285 tests total (28 CLI + 139 core + 118 e2e)
- E2E tests in `crates/core/tests/vault_e2e.rs` use real JPEG/PNG generation via the `image` crate
- Cross-format testing: use `create_file_with_jpeg_bytes()` to write JPEG bytes to `.cr2`/`.heic`/`.dng` etc. — scanner assigns format from extension, hashes work on raw bytes
- Use structurally different patterns (gradient vs checkerboard vs stripes) in tests to ensure distinct perceptual hashes — color-only differences are not enough
- `tempfile` crate for isolated test directories
- CLI status tests: extracted testable logic (StatusData, compute_aggregates, etc.) for unit testing without stdout capture
- Vault sync tests cover: date parsing, collision handling, incremental skip, cross-format dedup, progress events, error cases, file content preservation, quality upgrade with superseded file cleanup
- Quality preservation tests: all format tier combinations (CR2>JPEG, DNG>JPEG, CR2>HEIC, TIFF>JPEG, PNG>HEIC, JPEG>HEIC), vault as source preserves RAW, vault replaces lower-quality with higher-quality on resync
- HEIC export tests: macOS-only tests gated with `#[cfg(target_os = "macos")]`, cross-platform config/error tests run everywhere
