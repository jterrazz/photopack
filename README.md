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
cargo run -p losslessvault-cli -- status
cargo run -p losslessvault-cli -- duplicates
cargo run -p losslessvault-cli -- duplicates 1

# Export deduplicated library to vault (preserves original formats)
cargo run -p losslessvault-cli -- vault set ~/Vault
cargo run -p losslessvault-cli -- vault save

# Export as HEIC (macOS — like iCloud Photo export)
cargo run -p losslessvault-cli -- vault export-set ~/Export
cargo run -p losslessvault-cli -- vault export
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `lsvault sources` | List registered source directories |
| `lsvault sources add <path>` | Register a directory as a photo source |
| `lsvault sources scan` | Scan all sources, hash files, and find duplicates |
| `lsvault status` | Show rich status dashboard (overview, sources, vault) |
| `lsvault status --files` | Include full files table with roles and vault eligibility |
| `lsvault duplicates` | List all duplicate groups |
| `lsvault duplicates <id>` | Show group detail with source-of-truth marker |
| `lsvault vault set <path>` | Set the vault export destination directory |
| `lsvault vault show` | Show the current vault path |
| `lsvault vault save` | Copy deduplicated best-quality photos to the vault |
| `lsvault vault export-set <path>` | Set the HEIC export destination directory |
| `lsvault vault export-show` | Show the current export path |
| `lsvault vault export [--quality 85]` | Convert deduplicated photos to HEIC (macOS) |

The catalog defaults to `~/.losslessvault/catalog.db`. Override with `--catalog <path>`.

## How It Works

### Duplicate Detection (4-phase pipeline)

1. **Exact match (Phase 1)** — SHA-256 hash identity groups byte-identical files across any directory. Confidence: **Certain**.

2. **EXIF triangulation (Phase 2)** — Groups photos with the same capture date and camera model. If perceptual hashes confirm similarity, confidence is **High**; otherwise **Near-Certain** (EXIF-only). This catches cross-format duplicates (e.g. a JPEG export of a RAW file) when both retain EXIF metadata.

3. **Perceptual similarity (Phase 3)** — Compares ungrouped photos against *all* photos (including already-grouped ones) using pHash/dHash Hamming distance. This bridges cross-format duplicates where one variant is already in a SHA-256 group. Confidence: **Probable** to **Near-Certain** depending on distance.

4. **Transitive merge (Phase 4)** — Overlapping groups are merged iteratively until no overlaps remain. The merged group takes the lowest (most conservative) confidence level.

### Confidence Levels

| Level | Meaning |
|-------|---------|
| Certain | Byte-identical SHA-256 |
| Near-Certain | Strong EXIF match or very close perceptual hash (distance <= 2) |
| High | EXIF match validated by perceptual hash (distance <= 5) |
| Probable | Perceptual hash match (distance <= 10) |
| Low | Weak signal (reserved for future heuristics) |

### Perceptual Hashing

Perceptual hashes (pHash and dHash) are computed for formats the `image` crate can decode: **JPEG, PNG, TIFF, WebP**. Formats like HEIC and RAW are indexed by SHA-256 and EXIF but skip perceptual hashing to avoid hangs from unsupported decoders.

The hasher uses `img_hash` v3 (internally `image` v0.23) with a fallback to `image` v0.25 for broader format coverage.

### Source-of-Truth Election

Each duplicate group elects a best copy using:

1. **Format quality tier** — RAW (CR2, CR3, NEF, ARW, ORF, RAF, RW2, DNG) > TIFF > PNG > JPEG > HEIC > WebP
2. **Largest file size** (tiebreaker)
3. **Oldest modification time** (final tiebreaker)

### Incremental Scanning

Rescanning skips files whose modification time (mtime) hasn't changed since the last scan. New or modified files are hashed and inserted; duplicate groups are rebuilt from scratch each scan.

### Status Dashboard

`lsvault status` displays a rich overview:

- **Overview** — Photo count, unique count, duplicate groups, disk usage, estimated savings, source count, vault path
- **Sources table** — Per-source photo count, total size, and last scanned timestamp
- **Files table** (`--files`) — Every file with its source name, format, size, group ID, role (Best Copy / Duplicate / Unique), and vault eligibility (checkmark)

Files are sorted by group (source-of-truth first within each group), then ungrouped files by path. Blank separator rows visually separate groups.

### Vault Export

`lsvault vault save` copies a clean, deduplicated photo library to the configured vault directory:

- **Deduplication** — For each duplicate group, only the source-of-truth is exported. Ungrouped photos are exported as-is.
- **Date-based organization** — Photos are organized into `YYYY/MM/DD/` folders based on EXIF capture date, with modification time as fallback.
- **Collision handling** — When multiple photos share the same date and filename, a suffix (`_1`, `_2`, ...) is appended.
- **Incremental** — Re-running `vault save` skips files that already exist in the vault with the same size.
- **Vault path persistence** — The destination is stored in the SQLite catalog and persists across sessions.

### HEIC Export (macOS)

`lsvault vault export` converts deduplicated photos to high-quality HEIC files, mimicking macOS iCloud Photo's export behavior:

- **Full resolution** — Photos are converted at full width using macOS's native `sips` tool
- **Quality control** — Default quality 85 (0-100 range via `--quality` flag)
- **Same deduplication** — Only source-of-truth and ungrouped photos are exported
- **Date organization** — Same `YYYY/MM/DD/` folder structure as vault save
- **Incremental** — Existing HEIC files are skipped on re-export
- **All formats supported** — Converts JPEG, PNG, TIFF, RAW (CR2, NEF, etc.) — anything macOS can decode
- **Separate destination** — Export path is independent from vault save path

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
│   │   │   │   └── perceptual.rs # pHash/dHash (img_hash + image fallback)
│   │   │   ├── exif.rs         # EXIF extraction (kamadak-exif)
│   │   │   ├── matching/       # 4-phase duplicate matching pipeline
│   │   │   │   ├── mod.rs      # Pipeline orchestration + group merge
│   │   │   │   └── confidence.rs # Hamming distance thresholds
│   │   │   ├── ranking.rs      # Source-of-truth election
│   │   │   ├── vault_save.rs   # Vault export logic (date org, dedup, copy)
│   │   │   └── export.rs       # HEIC export via macOS sips
│   │   └── tests/
│   │       └── vault_e2e.rs    # 95 end-to-end integration tests
│   └── cli/                    # Binary crate (lsvault)
│       └── src/
│           ├── main.rs         # clap CLI definition
│           └── commands/       # Subcommand handlers
│               ├── sources.rs  # Add, scan, list sources (progress bar via indicatif)
│               ├── status.rs   # Rich dashboard with tables (comfy-table)
│               ├── duplicates.rs # List/detail duplicate groups
│               └── vault.rs    # Vault set/show/save + export commands
└── tests/
    └── fixtures/               # Test photo fixtures
```

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `rusqlite` (bundled) | SQLite catalog with WAL mode |
| `sha2` | SHA-256 file hashing |
| `img_hash` 3 | Perceptual hashing (pHash, dHash) |
| `image` 0.25 | Image decoding fallback for perceptual hashing |
| `kamadak-exif` | EXIF metadata extraction |
| `sha2-asm` | Hardware-accelerated SHA-256 (ARM Crypto Extensions) |
| `rayon` | Parallel file hashing, copying, and HEIC conversion |
| `walkdir` | Recursive directory traversal |
| `clap` (derive) | CLI argument parsing |
| `indicatif` | Progress bars during scan |
| `comfy-table` | UTF-8 box-drawing tables for status dashboard |
| `chrono` | Date handling for vault export organization |
| `thiserror` / `anyhow` | Error handling (core / CLI) |

## Development

```bash
# Run all tests (156 unit + 95 e2e)
cargo test --workspace

# Lint
cargo clippy --workspace
```

### Test Coverage

The test suite covers:

- **Catalog** (18 tests) — CRUD operations, format/confidence roundtrip, mtime tracking, config persistence
- **Matching** (25 tests) — All 4 phases, cross-format grouping, transitive merge, full pipeline
- **Status dashboard** (28 tests) — format_size, source_display_name, StatusData, is_duplicate, vault_eligible, compute_aggregates, compute_source_stats, sort_photos_for_display
- **Vault save** (23 tests) — Date parsing, EXIF/mtime fallback, photo selection, collision handling, incremental copy
- **Export** (21 tests) — build_export_path (all format extensions, collision, skip, no-extension), export_photo_to_heic (skip/convert), convert_to_heic (parent dirs, invalid source, output validation, quality effect), sips availability
- **Domain** (5 tests) — Quality tiers, format support, confidence ordering
- **Perceptual hash** (8 tests) — Hamming distance, real JPEG/PNG hashing
- **EXIF** (5 tests) — Edge cases, missing data
- **SHA-256** (4 tests) — Consistency, empty files, error handling
- **Scanner** (11 tests) — Directory walk, format filtering, nested directories (deep nesting, multiple levels, siblings, symlinks)
- **Ranking** (3 tests) — Format preference, size tiebreak, mtime tiebreak
- **E2E** (95 tests) — Full vault lifecycle, cross-directory and cross-format duplicates, incremental scan, source-of-truth election, photos API, quality preservation (all format tier combinations, RAW > HEIC > JPEG, vault as source), nested directories (multi-level, cross-source, incremental), vault export (deduplication, date structure, incremental skip, progress events, error cases, file integrity), HEIC export (JPEG/PNG conversion, multi-source, nested dirs, dedup, cross-source dedup, incremental skip+rescan, independent from vault save, progress events, config persistence, error handling, file validity)
