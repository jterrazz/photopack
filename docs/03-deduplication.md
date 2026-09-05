# Deduplication

Three independent signals decide that two files are the same photo. Each has
a blind spot the other two cover.

## The three signals

| Signal                 | Detects                                   | Strength                                    | Limit                                    |
| ---------------------- | ----------------------------------------- | ------------------------------------------- | ---------------------------------------- |
| SHA-256                | The same file copied                      | Exact, and fast — hardware-accelerated on ARM | Blind to format conversion               |
| EXIF triangulation     | The same capture (date and camera model)  | Links RAW to JPEG to HEIC reliably          | Some tools strip EXIF; scans have none   |
| Perceptual hash        | The same visual content                   | Survives recompression, format change, small crops | Can fire on very similar photos     |

### SHA-256

A cryptographic hash of the whole file: two files that share it are
byte-identical. It is streamed through a 64 KB buffer, accelerated on ARM
through the `sha2` `asm` feature, and parallelised with `rayon`. A RAW and
its JPEG export share nothing here.

### EXIF triangulation

Capture date and time — with sub-second precision when the file carries it —
plus camera make and model. A match on both is strong evidence that a RAW, a
JPEG and a HEIC came from one shutter press.

### Perceptual hash

Two 64-bit hashes are computed per image and both are stored. aHash compares
each pixel to the mean brightness and resists contrast changes; it lives in
the catalog's `phash` column. dHash reads the gradient between adjacent
pixels and is better at composition.

Matching requires dual-hash consensus: both must fall within threshold. The
two algorithms are sensitive to different features, so demanding both cuts
false positives sharply. When only one hash exists — a cross-format pair
where the HEIC or RAW side has none — the single available hash must clear
the stricter High threshold instead.

Hamming distance is the measure: the count of differing bits between two
64-bit hashes, lower being closer.

### The decode pipeline

Hand-rolled, replacing `img_hash`
([ADR-001](decisions/001-custom-ahash-dhash-over-img-hash.md)):

- JPEG goes through `turbojpeg`, which decodes directly to grayscale
  (GRAY, one byte per pixel, chroma skipped entirely). Decoding is
  full-resolution and must stay so — DCT-scaled intermediates of an original
  and its recompression diverge past the threshold
  ([ADR-002](decisions/002-full-resolution-decode-no-dct-scaling.md)).
- PNG, TIFF and WebP go through the `image` crate to RGB, resize to 9×8,
  then a manual BT.601 conversion over the 72 remaining pixels.
- Both paths apply EXIF orientation before the resize. iPhone originals
  store landscape pixels with a rotation tag while exports rotate the pixels
  and clear it; without the correction the same photo hashes to a distance
  around 33 out of 64.
- Both paths reduce to 9×8 with `fast_image_resize`, which uses SSE4.1,
  AVX2 or NEON.

`turbojpeg` is a default feature and can be switched off with
`--no-default-features`, leaving a pure-Rust path for a WASM build.

## Confidence levels

| Level        | Meaning                                                    |
| ------------ | ---------------------------------------------------------- |
| Certain      | Byte-identical SHA-256                                     |
| Near-certain | Strong EXIF match, or a perceptual distance of 2 or less   |
| High         | EXIF match validated by a perceptual distance of 2 or less |
| Probable     | Perceptual match at distance 3                             |
| Low          | Reserved for future heuristics                             |

Thresholds are deliberately tight: true cross-format duplicates land at
distance 0 to 2, and distinct photos at 3 or more.

## The five phases

### Phase 1 — exact matches

Group by SHA-256. Byte-identical files, confidence Certain.

### Phase 2 — EXIF triangulation, visually filtered

Cluster all photos, grouped or not, by EXIF signature (date plus camera
model). Perceptual hashes then act as a strict filter at the near-certain
threshold of 2 bits: a member whose hash fails pairwise validation is
removed, which is how burst and sequential shots from one camera are thrown
out. Members with no hash at all — HEIC and RAW — are kept, because EXIF is
the best signal they have.

Confidence is High when visual validation passed, near-certain when the
group rests on EXIF alone. Overlap with phase 1 groups is left to phase 4.

### Phase 3 — perceptual similarity

Compare each ungrouped photo against all photos, including already-grouped
ones, so a cross-format variant can still join a group formed in phase 1.
The comparison is dual-hash consensus, or the stricter High threshold alone
when one hash is missing. A BK-tree does the nearest-neighbour search.

A sequential shot filter rejects a match when both photos share a camera
model and their EXIF dates are 1 to 60 seconds apart without being
identical. True duplicates always carry the identical capture instant; burst
frames differ by seconds and can still hash identically at 9×8.

Confidence ranges from Probable to near-certain with the distance.

### Phase 4 — transitive merge

Overlapping groups merge only after cross-group visual validation: at least
one pair of exclusive members, one from each side, must be perceptually
close. Without it a bridge photo chains unrelated groups into one cascading
false merge. The merged group takes the lowest confidence of the two.

### Phase 5 — EXIF orphan sweep

After the merge, an ungrouped photo with no perceptual hash is attached to a
group when it shares an EXIF key — date plus camera — with one of its
members.

This closes one specific hole. Phase 2 forms an EXIF group holding both
hashable and non-hashable members; its hashable members fail the strict
dual-hash validation and are removed; the HEIC or RAW member is left alone
and dropped as a singleton. Phase 3 then regroups the hashable members at
Probable confidence, but the non-hashable one is invisible to it. Phase 5
puts it back.

## Phash version tracking

`PHASH_VERSION` in `lib.rs` names the version of the hashing algorithm. When
it changes, the next scan clears every cached perceptual hash from the
catalog and resets every file mtime.

Resetting the mtimes is not optional: the incremental scan skips files whose
mtime is unchanged, so clearing hashes alone would leave those files
permanently unhashed. With both cleared, full recomputation happens on the
next scan.

## Two-phase hashing

Scanning avoids image decoding wherever it can.

1. SHA-256 and EXIF for every new file, in parallel. This is I/O-bound, on
   the order of 10 to 50 ms per file.
2. Group those results by SHA-256. Only one representative per exact-duplicate
   set needs a perceptual hash, and any hash already in the catalog is
   reused.
3. Perceptual hashing for that unique content only, in parallel.

Four copies of one photo therefore cost one decode. Re-scanning after a new
exact duplicate appears costs none at all.
