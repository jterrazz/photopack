# The photopack corpus

How the engine works and how it is built, one subject per chapter. The
product knowledge above it — why Photopack exists, who it is for, how it is
positioned — is not here.

## Chapters

| Chapter                                        | Holds                                                                     |
| ----------------------------------------------- | -------------------------------------------------------------------------- |
| [01 — Architecture](01-architecture.md)        | The two crates, the dependency stack, the source layout, the standing rules |
| [03 — Testing](03-testing.md)                  | Where tests live, what they pin, how e2e fixtures are made               |
| [05 — Catalog](05-catalog.md)                  | The SQLite catalog: schema, indexes, migrations, batching                |
| [06 — Deduplication](06-deduplication.md)      | The three signals, the confidence levels, the five-phase pipeline        |
| [07 — Source of truth](07-source-of-truth.md)  | How a duplicate group elects its best version                            |
| [08 — Formats](08-formats.md)                  | What can be scanned, which formats get a perceptual hash                 |
| [09 — Pack and export](09-pack-and-export.md)  | The lossless archive, and the compressed HEIC library                    |
| [10 — CLI](10-cli.md)                          | The command families and the invariants `--help` does not state          |

There is no `04-operating.md`: photopack ships nothing that runs — no image,
no publishable package, no provisioned platform — it is `cargo install`ed
from source.

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
