# ADR-003: Pack / Export Split, Content-Addressable Pack

**Date:** 2026-02-16
**Status:** Accepted
**Recorded:** retroactively (2026-09-04), from this repository's history

## Context

The engine originally had a single "vault" concept that mixed two jobs:
the lossless keep-forever copy of the best originals, and the optimized
HEIC output. Both paths persisted their destination in the catalog. The
two jobs have opposite natures — one is an archive you must be able to
trust permanently, the other is a product you regenerate at will.

## Decision

Split the surface in two, and make the archive content-addressable:

- **`pack`** is the permanent lossless archive. Files are copied
  byte-for-byte into content-addressable storage — named by SHA-256
  (`{hash[..2]}/{hash}.{ext}`) — with an embedded SQLite manifest at
  `.photopack/manifest.sqlite`. The pack path persists in the catalog
  and the pack directory auto-registers as a scan source.
- **`export`** is the compressed HEIC library, reading from the catalog
  independently of the pack — a pack is never required. Its destination
  is deliberately NOT persisted: the path is given on every run
  (persistence was shipped, then removed).

The asymmetric persistence is the point: the archive is the one fixed
place; an export is disposable output you may aim anywhere.

## Consequences

### Easier

- Content addressing gives structural deduplication and integrity
  verification for free, and removes collision handling entirely.
- The vault survives source removal — it is the safety net, not a cache.

### Harder

- The pack's `YYYY/MM/DD` human layout was given up for hash-named
  files; the manifest is what maps hashes back to metadata.

## Evidence

- Commits `928b85c` (2026-02-15) — "Split pack into pack
  (lossless archive) and export (compressed HEIC)"; `8cd1d81`
  (2026-02-16) — "Switch pack to content-addressable store with embedded
  manifest"; `f2603af` (2026-02-16) — "Remove export path persistence —
  require path on each invocation"
