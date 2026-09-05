# Agent brief — photopack

A Rust workspace that deduplicates a photo library across formats, elects
the best version of each photo, and writes it out twice: a lossless archive
and a compressed HEIC library. This file routes into `docs/`; it does not
restate it.

## Mental model

- Two crates. `photopack-core` holds every rule and is portable;
  `photopack-cli` is a `clap` wrapper that prints. Nothing decides in the
  CLI.
- One entry point. Everything public goes through the `Vault` struct in
  `crates/core/src/lib.rs`.
- Three signals decide identity — SHA-256, EXIF, perceptual hash — and five
  ordered phases combine them. A change to matching is a change to that
  pipeline, not to a heuristic somewhere.
- The catalog is the state. A single SQLite file, versioned and migrated;
  sources on disk are read and never written.
- `rusqlite::Connection` is not `Sync`. Keep database access out of `rayon`
  sections: gather in parallel, write on one thread.

## Where knowledge lives

The corpus is `docs/`, mapped by [docs/README.md](docs/README.md). Link into
it; never duplicate it here.

| Working on                                  | Read                            |
| ------------------------------------------- | ------------------------------- |
| The crates, the layout, the data flow       | `docs/01-architecture.md`       |
| Schema, indexes, migrations, batching       | `docs/02-catalog.md`            |
| Matching, confidence, the five phases       | `docs/03-deduplication.md`      |
| Which version of a photo wins               | `docs/04-source-of-truth.md`    |
| Which formats are scanned or hashed         | `docs/05-formats.md`            |
| `pack`, `export`, HEIC and `sips`           | `docs/06-pack-and-export.md`    |
| Command surface and its invariants          | `docs/07-cli.md`                |
| Dependencies and rejected alternatives      | `docs/08-crates.md`             |
| Test layers, the count, e2e fixtures        | `docs/09-testing.md`            |
| Why a choice was made                       | `docs/decisions/`               |

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
cargo fmt --all
```

## Standing rules

- A change to matching, ranking or the catalog schema ships with its test in
  the same commit. E2E cases go in `crates/core/tests/vault_e2e.rs`.
- A change to the perceptual hash algorithm bumps `PHASH_VERSION` in the
  same commit, or every cached hash in the wild becomes silently wrong.
- Never reintroduce DCT-scaled JPEG decode. It has been tried and reverted
  three times; the reason is `docs/decisions/002-full-resolution-decode-no-dct-scaling.md`.
- A schema change adds a migration — raise `SCHEMA_VERSION`, write
  `migrate_vN_to_vM()`, append it to `MIGRATIONS`.
- A change to behaviour updates the chapter that owns the subject in the
  same commit.
