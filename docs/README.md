# The photopack corpus

How the engine works and how it is built, one subject per chapter. The
product knowledge above it — why Photopack exists, who it is for, how it is
positioned — is not here.

## Chapters

| Chapter                                        | Holds                                                                    |
| ---------------------------------------------- | ------------------------------------------------------------------------ |
| [01 — Architecture](01-architecture.md)        | The two crates, the source layout, the data flow, the standing rules     |
| [02 — Catalog](02-catalog.md)                  | The SQLite catalog: schema, indexes, migrations, batching                |
| [03 — Deduplication](03-deduplication.md)      | The three signals, the confidence levels, the five-phase pipeline        |
| [04 — Source of truth](04-source-of-truth.md)  | How a duplicate group elects its best version                            |
| [05 — Formats](05-formats.md)                  | What can be scanned, which formats get a perceptual hash                 |
| [06 — Pack and export](06-pack-and-export.md)  | The lossless archive, and the compressed HEIC library                    |
| [07 — CLI](07-cli.md)                          | The command families and the invariants `--help` does not state          |
| [08 — Crates](08-crates.md)                    | The dependency stack, the hardware acceleration, the rejected candidates |
| [09 — Testing](09-testing.md)                  | Where tests live, what they pin, how e2e fixtures are made               |

Decisions are in [decisions/](decisions/), numbered and chronological.

## Open questions

Carried here rather than in the chapter that raised them, so they are one
list and not a hunt.

- Should the core decode RAW files for perceptual hashing, or keep relying
  on SHA-256 and EXIF for them? Full decoding is more accurate and much
  slower.
- At what confidence level should a match be auto-grouped rather than
  flagged for review?
- Should `photopack` ever delete source files? The design is read-only
  today.
- Should pack and export mirror the source folder structure, flatten, or
  keep the date folders?
- Which library extracts a JPEG quality factor from the quantization
  tables? `kamadak-exif` does not expose it.
- How much of the core can realistically compile to WASM? File I/O and
  SQLite are the obstacles; `turbojpeg` is already gated behind a default
  feature for this.
- How many photos should a quality preview show before a full export — a
  random sample, or the worst-case SSIM?
