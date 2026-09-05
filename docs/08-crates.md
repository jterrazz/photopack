# Crates

What the workspace depends on, what each dependency buys, and what was
considered instead.

## Why Rust

Scanning thousands of photos means SHA-256 and perceptual hashing in
parallel, which `rayon` makes ordinary. Memory safety without a garbage
collector matters when large image buffers move through the pipeline. The
CLI ecosystem — `clap`, `rusqlite`, `image` — covers everything needed, and
the core compiles on Linux, macOS and Windows without change.

## The stack

| Need               | Crate                     | Note                                                          |
| ------------------ | ------------------------- | ------------------------------------------------------------- |
| CLI parsing        | `clap` (derive)           | Derive-based arguments and subcommands                        |
| Progress bars      | `indicatif`               | Scan and export progress                                      |
| Terminal tables    | `comfy-table`             | The dashboard's box-drawing tables                            |
| Catalog database   | `rusqlite` (bundled)      | SQLite in WAL mode, bundled so there is no system dependency  |
| Image decoding     | `image` 0.25              | PNG, TIFF, WebP, and the JPEG fallback                        |
| JPEG decoding      | `turbojpeg` 1.4           | libjpeg-turbo, optional default feature                       |
| Image resize       | `fast_image_resize` 6     | SIMD resize for the 9×8 perceptual target                     |
| Perceptual hashing | none — hand-rolled        | aHash and dHash computed in-repo, [ADR-001](decisions/001-custom-ahash-dhash-over-img-hash.md) |
| SHA-256            | `sha2` with `sha2-asm`    | Hardware-accelerated on ARM                                   |
| EXIF               | `kamadak-exif`            | Date, camera, GPS, dimensions                                 |
| Parallelism        | `rayon`                   | Scanning, hashing, copying, conversion                        |
| Dates              | `chrono`                  | The `YYYY/MM/DD` export layout                                |
| Errors             | `thiserror` / `anyhow`    | Core / CLI                                                    |
| HEIC encoding      | macOS `sips`, a system tool | Invoked through `std::process::Command`, no dependency      |
| Directory walk     | `walkdir`                 | Recursive source traversal                                    |

## Hardware acceleration

- SHA-256 uses ARM Crypto Extensions through `sha2-asm` — roughly 3 to 5
  times faster on Apple Silicon.
- The perceptual resize uses SSE4.1 or AVX2 on x86 and NEON on ARM, through
  `fast_image_resize`.
- JPEG decode through libjpeg-turbo straight to grayscale is roughly 2 to 3
  times faster than the `image` crate's path.
- HEIC encoding runs on Apple's VideoToolbox hardware H.265 encoder, because
  that is what `sips` drives.
- Hamming distance compiles to the ARM `cnt` popcount instruction.
- `rayon` fills every available core without configuration.

## Rejected alternatives

| Component       | Chosen          | Rejected               | Why                                                                     |
| --------------- | --------------- | ---------------------- | ----------------------------------------------------------------------- |
| Language        | Rust            | Go                     | No native FFI to Apple frameworks, and GC pauses under image processing |
| Language        | Rust            | C++                    | Less safe, and harder dependency management                             |
| Database        | SQLite          | PostgreSQL             | Needs a server; not portable for a desktop tool                         |
| Database        | SQLite          | Flat JSON files        | Too slow to query thousands of entries                                  |
| Image library   | `image`         | ImageMagick bindings   | Heavy, and awkward to compile across platforms                          |
| Perceptual hash | hand-rolled     | `img_hash`             | Pins `image` 0.23, so neither the decode path nor EXIF orientation can be controlled |
| EXIF            | `kamadak-exif`  | `rexif`                | Narrower format support                                                 |
| HEIC encoding   | `sips`          | `libheif-rs`           | `sips` costs no dependency and reaches the hardware encoder             |

The perceptual-hash row is two recorded decisions: replacing `img_hash` is
[ADR-001](decisions/001-custom-ahash-dhash-over-img-hash.md), and the
full-resolution rule that control made possible is
[ADR-002](decisions/002-full-resolution-decode-no-dct-scaling.md).
