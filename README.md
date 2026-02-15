# photopack

Pack your photo library tight. Deduplicate, organize, compress.

Your photos are a mess. Same shot saved as iPhone HEIC, Lightroom JPEG, and a RAW backup. Copies in iCloud, on a USB drive, and in ~/old-photos. After a few years, 30-50% of your library is redundant — and you're paying for iCloud storage you don't need.

**photopack** scans all your sources, finds every duplicate across formats, keeps the highest-quality version, and packs everything into a clean date-organized archive. Export to HEIC and it's 3x smaller. Zero quality loss.

### Use cases

- **All your photos in one place** — Point photopack at iCloud, Lightroom exports, camera imports, old backups. It merges everything into a single deduplicated archive organized by date.
- **Cut your iCloud bill** — Your 200GB library is full of duplicates you can't see — same photo as RAW + JPEG + HEIC across folders. photopack finds them all and exports a clean HEIC library that's 3x smaller.
- **Smart cross-format dedup** — Not just byte-matching. SHA-256, perceptual hashing, and EXIF metadata catch duplicates across RAW, JPEG, HEIC, PNG, TIFF, and WebP — even when file sizes and formats are completely different.

## Quick Start

```bash
# Install
cargo install photopack

# Point it at your photo sources
photopack add ~/Photos
photopack add ~/iCloud
photopack add /Volumes/Backup/Photos

# Scan — finds all duplicates across formats
photopack scan

# See what it found
photopack status
photopack dupes

# Pack into a permanent lossless archive (best-quality originals, date-organized)
photopack pack ~/PhotoArchive

# Or export as compressed HEIC (3x smaller, macOS)
photopack export ~/PhotosPacked --quality 85
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `photopack add <path>` | Register a directory as a photo source |
| `photopack rm <path>` | Unregister a source and remove its photos from the catalog |
| `photopack scan` | Scan all sources, hash files, and find duplicates |
| `photopack status` | Show catalog dashboard (overview, sources, vault) |
| `photopack status --files` | Show full files table with roles and vault eligibility |
| `photopack dupes` | List all duplicate groups |
| `photopack dupes <id>` | Show group detail with source-of-truth marker |
| `photopack pack <path>` | Set vault path and sync best-quality originals (lossless) |
| `photopack pack` | Re-sync using saved vault path |
| `photopack export <path>` | Set export path and convert to compressed HEIC (macOS) |
| `photopack export [--quality 85]` | Re-export using saved path with quality control |

The catalog defaults to `~/.photopack/catalog.db`. Override with `--catalog <path>`.

## How It Works

### Duplicate Detection (4-phase pipeline)

1. **Exact match (Phase 1)** — SHA-256 hash identity groups byte-identical files across any directory. Confidence: **Certain**.

2. **EXIF triangulation (Phase 2)** — Groups photos with the same capture date and camera model. Perceptual hashes act as a **filter**: members with hashes that fail visual validation (NEAR_CERTAIN threshold, distance > 2) are removed. This rejects burst/sequential shots that share EXIF metadata but differ visually. Members without hashes (HEIC/RAW) are kept on EXIF evidence alone. Confidence: **High** if visually validated, **Near-Certain** otherwise.

3. **Perceptual similarity (Phase 3)** — Compares ungrouped photos against *all* photos (including already-grouped ones) using **dual-hash consensus**: both aHash and dHash must be within threshold. When one hash is missing (cross-format), only the stricter High threshold (distance <= 2) is accepted. A **sequential shot filter** rejects matches where both photos have the same camera model and EXIF dates 1-60 seconds apart (but not identical) — true duplicates always have identical EXIF dates, while burst/sequential shots differ by seconds. Uses BK-tree for O(n log n) lookups. Confidence: **Probable** to **Near-Certain** depending on distance.

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

The hasher uses a hybrid decode pipeline:

- **JPEG path** — `turbojpeg` (libjpeg-turbo) decodes directly to grayscale (`GRAY` pixel format, 1 byte/pixel, skips chroma entirely). Full-resolution decode is critical — DCT scaling causes hash divergence between differently-compressed versions of the same photo.
- **Non-JPEG path** — `image` crate decodes to RGB, resizes to 9x8 via `fast_image_resize`, then applies manual BT.601 grayscale conversion on 72 pixels.
- **EXIF orientation** — Applied before resize on both paths. iPhone originals store landscape pixels with a rotation tag (e.g., orientation=6); iOS exports physically rotate pixels and clear the tag (orientation=1). Without orientation correction, the same photo produces completely different hashes (distance ~33/64).
- **SIMD resize** — Both paths use `fast_image_resize` for hardware-accelerated resize (SSE4.1, AVX2, NEON) to the 9x8 target.

The `turbojpeg` feature is optional (`--no-default-features` for pure-Rust/WASM builds).

**Phash version tracking** — A `PHASH_VERSION` constant auto-invalidates all cached perceptual hashes when the algorithm changes. On version mismatch, the scan clears all stored hashes and resets mtimes, forcing full recomputation.

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

`photopack status` displays a rich overview:

- **Overview** — Photo count, unique count, duplicate groups, disk usage, estimated savings, source count, vault path
- **Sources table** — Per-source photo count, total size, and last scanned timestamp
- **Files table** (`status --files`) — Every file with its source name, format, size, group ID, role (Best Copy / Duplicate / Unique), and vault eligibility (checkmark)

Files are sorted by group (source-of-truth first within each group), then ungrouped files by path. Blank separator rows visually separate groups.

### Pack (Lossless Archive)

`photopack pack` syncs a clean, deduplicated photo library to the configured pack directory using **content-addressable storage**. The pack is a permanent lossless archive — even if you remove sources later, the pack keeps your best originals:

- **Content-addressable** — Files are named by their SHA-256 hash (`{hash[..2]}/{hash}.{ext}`), providing structural deduplication and integrity verification. No collision handling needed.
- **Embedded manifest** — A SQLite database at `.photopack/manifest.sqlite` maps hashes to metadata (original filename, format, size, EXIF data).
- **Deduplication** — For each duplicate group, only the source-of-truth is synced. Ungrouped photos are synced as-is. Identical files produce the same hash → one pack file.
- **Quality upgrade** — When a higher-quality format becomes SOT (e.g., RAW replaces JPEG), the new format is packed alongside. Stale entries are cleaned up via the manifest.
- **Incremental** — Re-running `pack` skips files whose hash-named file already exists on disk.
- **Pack path persistence** — The destination is stored in the SQLite catalog and persists across sessions.

### HEIC Export (macOS)

`photopack export` converts deduplicated photos to compressed HEIC files, mimicking macOS iCloud Photo's export behavior. Export reads from the catalog (source directories), independent from the vault:

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
photopack/
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── core/                   # Library crate (photopack-core)
│   │   ├── src/
│   │   │   ├── lib.rs          # Public Vault API + PHASH_VERSION tracking
│   │   │   ├── domain.rs       # PhotoFile, PhotoFormat, DuplicateGroup, Confidence, ExifData
│   │   │   ├── error.rs        # Error types (thiserror)
│   │   │   ├── catalog/        # SQLite catalog (rusqlite, WAL mode)
│   │   │   │   ├── mod.rs      # CRUD operations, phash invalidation, mtime reset
│   │   │   │   └── schema.rs   # Table definitions
│   │   │   ├── scanner/        # Recursive directory walk (walkdir)
│   │   │   │   ├── mod.rs      # scan_directory()
│   │   │   │   └── formats.rs  # Extension -> PhotoFormat mapping
│   │   │   ├── hasher/         # File hashing
│   │   │   │   ├── mod.rs      # SHA-256 (sha2)
│   │   │   │   └── perceptual.rs # aHash/dHash (turbojpeg + EXIF orientation + fast_image_resize)
│   │   │   ├── exif.rs         # EXIF extraction (kamadak-exif)
│   │   │   ├── matching/       # 4-phase duplicate matching pipeline
│   │   │   │   ├── mod.rs      # Pipeline orchestration, BK-tree, sequential shot filter, merge
│   │   │   │   └── confidence.rs # Hamming distance thresholds
│   │   │   ├── ranking.rs      # Source-of-truth election
│   │   │   ├── vault_save.rs   # Pack sync logic (content-addressable, parallel copy)
│   │   │   ├── manifest.rs     # Embedded manifest (SQLite, hash→metadata)
│   │   │   └── export.rs       # HEIC export via macOS sips
│   │   └── tests/
│   │       └── vault_e2e.rs    # 129 end-to-end integration tests
│   └── cli/                    # Binary crate (photopack)
│       └── src/
│           ├── main.rs         # clap CLI definition
│           └── commands/       # Subcommand handlers
│               ├── sources.rs  # Add, rm, scan sources (progress bar via indicatif)
│               ├── status.rs   # Catalog dashboard with tables (comfy-table)
│               ├── duplicates.rs # List/detail duplicate groups
│               ├── pack.rs     # Lossless vault archive
│               └── export.rs   # Compressed HEIC export
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
# Run all tests (375 total)
cargo test --workspace

# Lint
cargo clippy --workspace
```

