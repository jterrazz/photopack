# ADR-002: Full-Resolution Decode — No DCT Scaling

**Date:** 2026-02-15
**Status:** Accepted
**Recorded:** retroactively (2026-09-04), from this repository's history

## Context

Perceptual hashing only needs a 9×8 buffer, and turbojpeg offers DCT
scaling (decode at 1/8, 1/4, 1/2 resolution) — an apparently free,
large speedup for the hottest path in a scan. It was implemented the
same day the custom pipeline landed.

The trap: recompressed JPEGs have **different DCT coefficients** than
their originals. Scaled decode reconstructs pixels from those
coefficients directly, so the scaled intermediates of an original and
its recompression diverge — pushing hash distances past the ≤2
NEAR_CERTAIN threshold and silently breaking exactly the match the
perceptual hash exists for (original ↔ recompressed copy).

## Decision

Full-resolution decode is mandatory for perceptual hashing. The speedup
comes from what decoding skips, not from scaling:

- JPEG decodes directly to grayscale (GRAY format, 1 byte/pixel) —
  skipping chroma is safe and gives the ~2-3x gain
- `fast_image_resize` does the reduction to 9×8 with SIMD, after decode

The rule is pinned in doc comments in
`crates/core/src/hasher/perceptual.rs`, because it does not stay
learned: the optimization was attempted and reverted **three times**
over the project's life. `PHASH_VERSION` — at 4 — tracks the resulting
algorithm churn: each bump invalidates all cached hashes and resets file
mtimes so skipped files re-enter hashing.

## Consequences

### Easier

- Correct matching of recompressed JPEGs — the core promise holds.
- The revert discipline produced the version-tracking machinery
  ([deduplication — phash version tracking](../03-deduplication.md#phash-version-tracking)).

### Harder

- JPEG decode stays full-resolution — scans pay the real decode cost on
  every unique photo. Two-phase hashing (SHA-256 dedup first) exists to
  keep that cost to one decode per unique image.

## Evidence

- Commits `d2d8958` (2026-02-15) — "Accelerate perceptual
  hashing with DCT scaling…" — and its revert `2ab5a06` (same day) —
  "Remove DCT scaling to fix hash divergence on recompressed JPEGs"
- `PHASH_VERSION = "4"` in `crates/core/src/lib.rs`; the three attempts and
  their reverts are in the repository's history
