# Developing

How a change to photopack is made: the toolchain, which crate it belongs in,
and what it owes before it lands.

## Toolchain

A `cargo` workspace of two members, declared in `Cargo.toml`:
`crates/core` and `crates/cli`.

```bash
cargo build --workspace   # build both crates
cargo clippy --workspace  # lint
cargo fmt --all           # format
```

`cargo`'s own output is redirected under `.artifacts/cargo` instead of the
default `target/`, so every tool's build output sits under one gitignored
root — `.cargo/config.toml`. `cargo test` proves a change; it is
[03 — Testing](03-testing.md)'s.

## Gates

This repository carries no `Makefile` and no CI workflow (no `.github/`
directory): `cargo build`, `cargo clippy`, `cargo fmt --check` and
`cargo test` are run by hand, and a reviewer is the gate. The estate-wide
docs layout is checked the same way, from the repository root:

```bash
npx --yes @jterrazz/typescript docs-layout .
```

## Where a change goes

`photopack-core` (`crates/core/`) holds every rule: the catalog, matching,
ranking, pack sync, HEIC export. `photopack-cli` (`crates/cli/`) only
parses arguments and prints — it decides nothing. The source layout and
which file owns which subsystem are [01 — Architecture](01-architecture.md)'s.

## What a change owes

- **A test in the same commit.** A change to matching, ranking or the
  catalog schema ships with its proof — [03 — Testing](03-testing.md) says
  where a case belongs; end-to-end cases go in
  `crates/core/tests/vault_e2e.rs`.
- **A `PHASH_VERSION` bump.** A change to the perceptual hash algorithm
  raises the constant in the same commit, or every cached hash in the wild
  becomes silently wrong — the mechanism it drives is
  [06 — Deduplication](06-deduplication.md#phash-version-tracking)'s.
- **No DCT-scaled JPEG decode.** The optimization has been tried and
  reverted three times; the reason it stays banned is
  [ADR-002](decisions/002-full-resolution-decode-no-dct-scaling.md).
- **A migration for a schema change.** Raise `SCHEMA_VERSION`, write
  `migrate_vN_to_vM()`, append it to `MIGRATIONS` —
  [05 — Catalog](05-catalog.md#schema-versioning) carries the mechanism.
- **The chapter that owns the subject, updated in the same commit.** A
  chapter describing behaviour a change makes false is part of the change,
  not a follow-up.
