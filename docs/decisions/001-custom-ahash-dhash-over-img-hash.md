# ADR-001: Custom aHash + dHash Pipeline over `img_hash`

**Date:** 2026-02-15
**Status:** Accepted
**Recorded:** retroactively (2026-09-04), from this repository's history

## Context

Deduplication needs perceptual hashes that survive format changes and
recompression. The off-the-shelf choice, the `img_hash` crate, was the
original implementation — and it hides exactly the two things that turned
out to matter:

1. It pins `image` 0.23 internally, so the decode pipeline cannot be
   controlled (no way to forbid DCT scaling — see
   [ADR-002](./002-full-resolution-decode-no-dct-scaling.md)).
2. It offers no hook to apply EXIF orientation before hashing, and iPhone
   originals store landscape pixels with a rotation tag while exports
   physically rotate and clear it — the same photo hashes to distance
   ~33/64 (random) without correction.

## Decision

Compute aHash and dHash manually on a 9×8 grayscale buffer, over a custom
hybrid decode pipeline:

- **JPEG**: `turbojpeg` decodes full-resolution directly to grayscale
  (GRAY pixel format — skips chroma entirely)
- **PNG/TIFF/WebP**: `image` crate decodes to RGB, manual BT.601
  grayscale conversion on 72 pixels
- Both paths apply EXIF orientation before the SIMD resize
  (`fast_image_resize`) to 9×8
- aHash is stored in the catalog's `phash` column; dHash beside it —
  matching uses dual-hash consensus

## Consequences

### Easier

- Full control of the decode path enabled the two correctness fixes that
  followed: no DCT scaling (ADR-002) and orientation-before-hash.
- The `turbojpeg` dependency is an optional default feature —
  `--no-default-features` keeps a pure-Rust path for WASM builds.

### Harder

- Photopack owns the algorithm, so it also owns cache invalidation: the
  `PHASH_VERSION` constant exists because the hand-rolled pipeline
  changes — bumping it clears cached hashes and resets mtimes.

## Evidence

- Commit `ade7366` (2026-02-15) — "Replace img_hash with
  turbojpeg + fast_image_resize"; `eb219e3` (2026-02-15) — "Apply EXIF
  orientation before perceptual hashing"
- The rejected-alternatives row in [crates](../08-crates.md); the pipeline
  itself in `crates/core/src/hasher/perceptual.rs`
