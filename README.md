# LosslessVault

A Rust-powered photo deduplication engine. Scan local folders, identify duplicates via SHA-256, perceptual hashing, and EXIF triangulation, then elect a source-of-truth per group. Export a clean, deduplicated photo library organized by date. Everything is persisted in a local SQLite catalog.

## Quick Start

```bash
# Build
cargo build --workspace

# Add photo directories
cargo run -p losslessvault-cli -- sources add ~/Photos
cargo run -p losslessvault-cli -- sources add ~/Backups/Photos

# Scan for duplicates
cargo run -p losslessvault-cli -- sources scan

# View results
cargo run -p losslessvault-cli -- catalog
cargo run -p losslessvault-cli -- catalog duplicates
cargo run -p losslessvault-cli -- catalog duplicates 1

# Sync deduplicated library to vault (preserves original formats)
cargo run -p losslessvault-cli -- vault set ~/Vault
cargo run -p losslessvault-cli -- vault sync

# Export as HEIC (macOS — like iCloud Photo export)
cargo run -p losslessvault-cli -- export set ~/Export
cargo run -p losslessvault-cli -- export
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `lsvault sources` | List registered source directories |
| `lsvault sources add <path>` | Register a directory as a photo source |
| `lsvault sources scan` | Scan all sources, hash files, and find duplicates |
| `lsvault sources rm <path>` | Unregister a source and remove its photos from the catalog |
| `lsvault catalog` | Show catalog dashboard (overview, sources, vault) |
| `lsvault catalog list` | Show full files table with roles and vault eligibility |
| `lsvault catalog duplicates` | List all duplicate groups |
| `lsvault catalog duplicates <id>` | Show group detail with source-of-truth marker |
| `lsvault vault set <path>` | Set the vault directory (auto-registers as scan source) |
| `lsvault vault sync` | Sync deduplicated best-quality photos to the vault |
| `lsvault export set <path>` | Set the HEIC export destination directory |
| `lsvault export show` | Show the current export path |
| `lsvault export [--quality 85]` | Convert deduplicated photos to HEIC (macOS) |

The catalog defaults to `~/.losslessvault/catalog.db`. Override with `--catalog <path>`.

## How It Works

### Duplicate Detection (4-phase pipeline)

1. **Exact match (Phase 1)** — SHA-256 hash identity groups byte-identical files across any directory. Confidence: **Certain**.

2. **EXIF triangulation (Phase 2)** — Groups photos with the same capture date and camera model. Perceptual hashes act as a **filter**: members with hashes that fail visual validation are removed (burst shots). Members without hashes (HEIC/RAW) are kept. Confidence: **High** if visually validated, **Near-Certain** otherwise.

3. **Perceptual similarity (Phase 3)** — Compares ungrouped photos against *all* photos (including already-grouped ones) using **dual-hash consensus**: both aHash and dHash must be within threshold. When one hash is missing (cross-format), only the stricter High threshold is accepted. Uses BK-tree for O(n log n) lookups. Confidence: **Probable** to **Near-Certain** depending on distance.

4. **Transitive merge (Phase 4)** — Overlapping groups are merged with **cross-group visual validation**: at least one pair of exclusive members must be perceptually close. Prevents cascading false merges through bridge photos.

### Confidence Levels

| Level | Meaning |
|-------|---------|
| Certain | Byte-identical SHA-256 |
| Near-Certain | Strong EXIF match or very close perceptual hash (distance <= 2) |
| High | EXIF match validated by perceptual hash (distance <= 2) |
| Probable | Perceptual hash match (distance <= 3) |
| Low | Weak signal (reserved for future heuristics) |

### Perceptual Hashing

Two 64-bit hashes are computed per image: **aHash** (average/mean, stored as `phash`) and **dHash** (gradient). Both must agree within threshold for a match (**dual-hash consensus**), dramatically reducing false positives. Supported formats: **JPEG, PNG, TIFF, WebP**. HEIC and RAW skip perceptual hashing (SHA-256 and EXIF only).

The hasher uses a hybrid decode pipeline: **`turbojpeg`** (libjpeg-turbo, ~2-3x faster JPEG decode) for JPEG, falling back to the `image` crate for other formats. Both paths resize to 9x8 grayscale via **`fast_image_resize`** (SIMD-accelerated: SSE4.1, AVX2, NEON), then compute aHash + dHash manually from the same buffer. The `turbojpeg` feature is optional (`--no-default-features` for pure-Rust/WASM builds).

### Source-of-Truth Election

Each duplicate group elects a best copy using:

1. **Format quality tier** — RAW (CR2, CR3, NEF, ARW, ORF, RAF, RW2, DNG) > TIFF > PNG > JPEG > HEIC > WebP
2. **Largest file size** (tiebreaker)
3. **Oldest modification time** (final tiebreaker)

### Incremental Scanning

Rescanning skips files whose modification time (mtime) hasn't changed since the last scan. New or modified files are hashed and inserted; files deleted from disk are automatically removed from the catalog. Duplicate groups are rebuilt from scratch each scan.

### Two-Phase Hashing (Performance)

Scanning uses a two-phase approach to minimize expensive image decoding:

1. **Phase 1 (fast)** — SHA-256 + EXIF extraction for all new files in parallel (I/O-bound, ~10-50ms/file)
2. **SHA-256 dedup** — Groups results by hash. For exact duplicates, only one representative needs perceptual hashing. Existing catalog hashes are reused.
3. **Phase 2 (optimized)** — Perceptual hashing only for unique content in parallel. JPEG uses `turbojpeg` (~2-3x faster decode); all formats use SIMD resize via `fast_image_resize`

If 4 copies of the same photo exist, only 1 image is decoded instead of 4. Re-scanning with a new exact duplicate reuses the catalog's perceptual hash (zero decodes).

### Catalog Dashboard

`lsvault catalog` displays a rich overview:

- **Overview** — Photo count, unique count, duplicate groups, disk usage, estimated savings, source count, vault path
- **Sources table** — Per-source photo count, total size, and last scanned timestamp
- **Files table** (`catalog list`) — Every file with its source name, format, size, group ID, role (Best Copy / Duplicate / Unique), and vault eligibility (checkmark)

Files are sorted by group (source-of-truth first within each group), then ungrouped files by path. Blank separator rows visually separate groups.

### Vault (Lossless Archive)

`lsvault vault sync` syncs a clean, deduplicated photo library to the configured vault directory. The vault is a permanent lossless archive — even if you remove sources later, the vault keeps your best originals:

- **Deduplication** — For each duplicate group, only the source-of-truth is synced. Ungrouped photos are synced as-is.
- **Quality upgrade** — When a higher-quality version is found in sources (e.g., RAW replaces JPEG as SOT), vault sync copies the better version and removes the superseded lower-quality file.
- **Date-based organization** — Photos are organized into `YYYY/MM/DD/` folders based on EXIF capture date, with modification time as fallback.
- **Collision handling** — When multiple photos share the same date and filename, a suffix (`_1`, `_2`, ...) is appended.
- **Incremental** — Re-running `vault sync` skips files that already exist in the vault with the same size.
- **Vault path persistence** — The destination is stored in the SQLite catalog and persists across sessions.

### HEIC Export (macOS)

`lsvault export` converts deduplicated photos to high-quality HEIC files, mimicking macOS iCloud Photo's export behavior. Export reads from the catalog (source directories), independent from the vault:

- **Full resolution** — Photos are converted at full width using macOS's native `sips` tool
- **Quality control** — Default quality 85 (0-100 range via `--quality` flag)
- **Same deduplication** — Only source-of-truth and ungrouped photos are exported
- **Date organization** — Same `YYYY/MM/DD/` folder structure as vault sync
- **Incremental** — Existing HEIC files are skipped on re-export
- **All formats supported** — Converts JPEG, PNG, TIFF, RAW (CR2, NEF, etc.) — anything macOS can decode
- **Separate destination** — Export path is independent from vault sync path

### Supported Formats

| Category | Formats |
|----------|---------|
| RAW | CR2, CR3, NEF, ARW, ORF, RAF, RW2, DNG |
| Lossless | TIFF, PNG |
| Lossy | JPEG, HEIC, WebP |

## Architecture

```
lossless-vault/
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── core/                   # Library crate (losslessvault-core)
│   │   ├── src/
│   │   │   ├── lib.rs          # Public Vault API
│   │   │   ├── domain.rs       # PhotoFile, PhotoFormat, DuplicateGroup, Confidence, ExifData
│   │   │   ├── error.rs        # Error types (thiserror)
│   │   │   ├── catalog/        # SQLite catalog (rusqlite, WAL mode)
│   │   │   │   ├── mod.rs      # CRUD operations
│   │   │   │   └── schema.rs   # Table definitions
│   │   │   ├── scanner/        # Recursive directory walk (walkdir)
│   │   │   │   ├── mod.rs      # scan_directory()
│   │   │   │   └── formats.rs  # Extension -> PhotoFormat mapping
│   │   │   ├── hasher/         # File hashing
│   │   │   │   ├── mod.rs      # SHA-256 (sha2)
│   │   │   │   └── perceptual.rs # aHash/dHash (turbojpeg + fast_image_resize)
│   │   │   ├── exif.rs         # EXIF extraction (kamadak-exif)
│   │   │   ├── matching/       # 4-phase duplicate matching pipeline
│   │   │   │   ├── mod.rs      # Pipeline orchestration + group merge
│   │   │   │   └── confidence.rs # Hamming distance thresholds
│   │   │   ├── ranking.rs      # Source-of-truth election
│   │   │   ├── vault_save.rs   # Vault sync logic (date org, dedup, parallel copy)
│   │   │   └── export.rs       # HEIC export via macOS sips
│   │   └── tests/
│   │       └── vault_e2e.rs    # 118 end-to-end integration tests
│   └── cli/                    # Binary crate (lsvault)
│       └── src/
│           ├── main.rs         # clap CLI definition
│           └── commands/       # Subcommand handlers
│               ├── sources.rs  # Add, rm, scan, list sources (progress bar via indicatif)
│               ├── status.rs   # Catalog dashboard with tables (comfy-table)
│               ├── duplicates.rs # List/detail duplicate groups
│               ├── vault.rs    # Vault set/sync commands
│               └── export.rs   # HEIC export set/show/run commands
└── tests/
    └── fixtures/               # Test photo fixtures
```

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `rusqlite` (bundled) | SQLite catalog with WAL mode |
| `sha2` | SHA-256 file hashing |
| `turbojpeg` 1.4 | Fast JPEG decoding via libjpeg-turbo (optional, default feature) |
| `fast_image_resize` 6 | SIMD-accelerated image resize (SSE4.1, AVX2, NEON) |
| `image` 0.25 | Image decoding for PNG, TIFF, WebP (and JPEG fallback) |
| `kamadak-exif` | EXIF metadata extraction |
| `sha2-asm` | Hardware-accelerated SHA-256 (ARM Crypto Extensions) |
| `rayon` | Parallel file hashing, copying, and HEIC conversion |
| `walkdir` | Recursive directory traversal |
| `clap` (derive) | CLI argument parsing |
| `indicatif` | Progress bars during scan |
| `comfy-table` | UTF-8 box-drawing tables for catalog dashboard |
| `chrono` | Date handling for vault sync and HEIC export (`YYYY/MM/DD/`) |
| `thiserror` / `anyhow` | Error handling (core / CLI) |

## Development

```bash
# Run all tests (167 unit + 118 e2e)
cargo test --workspace

# Lint
cargo clippy --workspace
```

### Test Coverage

The test suite covers:

- **Catalog** (21 tests) — CRUD operations, format/confidence roundtrip, mtime tracking, config persistence, source removal
- **Matching** (33 tests) — All 4 phases, dual-hash consensus, EXIF filtering (strict NEAR_CERTAIN threshold), merge safeguards, cross-format grouping, transitive merge, sequential shot rejection, full pipeline
- **Catalog dashboard** (28 tests) — format_size, source_display_name, StatusData, is_duplicate, vault_eligible, compute_aggregates, compute_source_stats, sort_photos_for_display
- **Vault sync** (23 tests) — Date parsing, EXIF/mtime fallback, photo selection, collision handling, incremental copy, quality upgrade cleanup
- **Export** (21 tests) — build_export_path (all format extensions, collision, skip, no-extension), export_photo_to_heic (skip/convert), convert_to_heic (parent dirs, invalid source, output validation, quality effect), sips availability
- **Domain** (5 tests) — Quality tiers, format support, confidence ordering
- **Perceptual hash** (9 tests) — Hamming distance, manual aHash/dHash, real JPEG/PNG hashing
- **EXIF** (5 tests) — Edge cases, missing data
- **SHA-256** (4 tests) — Consistency, empty files, error handling
- **Scanner** (11 tests) — Directory walk, format filtering, nested directories (deep nesting, multiple levels, siblings, symlinks)
- **Ranking** (3 tests) — Format preference, size tiebreak, mtime tiebreak
- **E2E** (118 tests) — Full vault lifecycle, cross-directory and cross-format duplicates, incremental scan, source-of-truth election, source removal (with group cleanup), vault auto-registration as source, photos API, quality preservation (all format tier combinations, RAW > HEIC > JPEG, vault as source), nested directories (multi-level, cross-source, incremental), vault sync (deduplication, date structure, incremental skip, quality upgrade with superseded file cleanup, progress events, error cases, file integrity), HEIC export (JPEG/PNG conversion, multi-source, nested dirs, dedup, cross-source dedup, incremental skip+rescan, independent from vault sync, progress events, config persistence, error handling, file validity)
