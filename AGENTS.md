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

## Where knowledge lives

The corpus is `docs/`, mapped by [docs/README.md](docs/README.md). Link into
it; never duplicate it here.

| Working on                                  | Read                            |
| ------------------------------------------- | ------------------------------- |
| The crates, the dependency stack, the layout | `docs/01-architecture.md`      |
| The toolchain, the gates, what a change owes | `docs/02-developing.md`        |
| Test layers, the count, e2e fixtures        | `docs/03-testing.md`            |
| Schema, indexes, migrations, batching       | `docs/05-catalog.md`            |
| Matching, confidence, the five phases       | `docs/06-deduplication.md`      |
| Which version of a photo wins               | `docs/07-source-of-truth.md`    |
| Which formats are scanned or hashed         | `docs/08-formats.md`            |
| `pack`, `export`, HEIC and `sips`           | `docs/09-pack-and-export.md`    |
| Command surface and its invariants          | `docs/10-cli.md`                |
| Why a choice was made                       | `docs/decisions/`               |

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
cargo fmt --all
```

## Standing rules

What a change owes — a test in the same commit, the `PHASH_VERSION` bump,
the ban on DCT-scaled decode, a migration for a schema change, the chapter
kept in sync — is `docs/02-developing.md` § "What a change owes".
