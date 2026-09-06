# Architecture

A Rust workspace of two crates: a portable library that holds every rule,
and a thin binary that exposes it.

## The two crates

`photopack-core` is the library, and it carries all the business logic: the
SQLite catalog, the five-phase duplicate detection, the BK-tree used for
perceptual matching, source-of-truth election, pack sync, and HEIC export.
It is portable — the one platform-specific path, HEIC conversion through
macOS `sips`, sits behind a runtime check.

`photopack-cli` is the binary named `photopack`. It parses arguments with
`clap` and calls the library; it holds no rule of its own beyond how a
result is printed.

Both are open source (MIT / Apache 2.0).

## Standing rules

- Every public entry point goes through the `Vault` struct in
  `crates/core/src/lib.rs`. Callers — the CLI, the tests, a future FFI —
  see that surface and nothing else.
- Errors are `thiserror` in the core and `anyhow` in the CLI.
- Rust 2021 edition.
- `rusqlite::Connection` is not `Sync`. Database access must be kept out of
  `rayon` parallel sections: collect in parallel, then write on one thread.
- Scanning is read-only. Nothing in the pipeline modifies or deletes a
  source file; the catalog is the only thing written.
- Scan, pack and export are all incremental — each skips what has not
  changed.

## Source layout

```
photopack/
├── Cargo.toml                      # workspace root
├── crates/
│   ├── core/                       # library crate (photopack-core)
│   │   ├── src/
│   │   │   ├── lib.rs              # the Vault API, PHASH_VERSION
│   │   │   ├── domain.rs           # PhotoFile, PhotoFormat, DuplicateGroup, Confidence, ExifData
│   │   │   ├── error.rs            # error types (thiserror)
│   │   │   ├── catalog/            # SQLite catalog (rusqlite, WAL)
│   │   │   │   ├── mod.rs          # CRUD, phash invalidation, mtime reset
│   │   │   │   └── schema.rs       # tables, indexes, migrations
│   │   │   ├── scanner/            # recursive walk (walkdir)
│   │   │   │   ├── mod.rs          # scan_directory()
│   │   │   │   └── formats.rs      # extension to PhotoFormat
│   │   │   ├── hasher/
│   │   │   │   ├── mod.rs          # streaming SHA-256 (sha2 + asm)
│   │   │   │   └── perceptual.rs   # aHash and dHash
│   │   │   ├── exif.rs             # EXIF extraction (kamadak-exif)
│   │   │   ├── matching/
│   │   │   │   ├── mod.rs          # the five phases, BK-tree, merge
│   │   │   │   └── confidence.rs   # Hamming distance thresholds
│   │   │   ├── ranking.rs          # source-of-truth election
│   │   │   ├── vault_save.rs       # pack sync (content-addressable, parallel)
│   │   │   ├── manifest.rs         # the pack's embedded manifest
│   │   │   └── export.rs           # HEIC export through sips
│   │   └── tests/
│   │       └── vault_e2e.rs        # end-to-end suite
│   └── cli/                        # binary crate (photopack)
│       └── src/
│           ├── main.rs             # the clap definition
│           └── commands/           # sources, status, ls, pack, export
└── tests/
    └── fixtures/
```

## What the engine does

| Step   | What happens                                                              |
| ------ | ------------------------------------------------------------------------- |
| Scan   | Walk every registered source folder and index each photo found            |
| Hash   | SHA-256 for exact identity, aHash and dHash for visual identity           |
| Match  | Combine hash, EXIF and perceptual signals into duplicate groups           |
| Rank   | Elect one source of truth per group                                       |
| Pack   | Copy the elected originals, unmodified, into a content-addressable store  |
| Export | Encode the deduplicated library as HEIC (macOS)                           |

`verify` and `prune` are designed and not shipped.

## Data flow

```
sources (local folders)
    │
    ▼
  scan     two phases: SHA-256 + EXIF first, perceptual hash for unique content only
    │
    ▼
  match    exact → EXIF filter → perceptual (dual-hash, BK-tree) → merge → EXIF orphan sweep
    │
    ▼
  rank     format tier, then size, then mtime
    │
    ▼
  catalog  persisted to SQLite in batch transactions
    │
    ├──▶ pack sync    content-addressable copy, parallel, incremental, manifest
    └──▶ HEIC export  sips conversion, parallel, incremental, macOS only
```

The chapters behind each step: [03 — Deduplication](03-deduplication.md),
[04 — Source of truth](04-source-of-truth.md), [02 — Catalog](02-catalog.md),
[06 — Pack and export](06-pack-and-export.md).

## Why it is fast

- Two-phase hashing. SHA-256 and EXIF run first because they are I/O-bound;
  perceptual hashing runs only for content whose SHA-256 is unique. Four
  copies of one photo cost one image decode, not four.
- The perceptual path decodes JPEG straight to grayscale and reduces with a
  SIMD resize, so a hash never pays for a full colour decode.
- Bulk photo upserts and group replacement each run in one SQLite
  transaction.
- Perceptual matching walks a BK-tree — O(n log n) instead of the O(n²) of
  a brute-force sweep.
- SHA-256 streams through a 64 KB buffer, so memory is O(64 KB) rather than
  O(file size).
- `rayon` parallelises file hashing, the pack copy and HEIC conversion.
- The catalog dashboard is one query with subqueries, not one query per
  group.
- `idx_photos_path` and `idx_photos_source_mtime` make the incremental scan
  a lookup rather than a table walk.

## What is not in this repository

The native macOS app links a `photopack-ffi` staticlib that the workspace
does not declare — it holds `crates/core` and `crates/cli` only. Landing
that crate here is the open gap, and until it does the app cannot be linked.

Platform adapter crates (PhotoKit on Apple, Windows.Storage and WIC, a
Lightroom catalog reader) and a WASM build of the core are designed and not
started.
