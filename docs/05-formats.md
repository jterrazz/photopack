# Formats

Which files the scanner picks up, and which of them get a perceptual hash.

## Scan and analysis

| Category | Formats                                | Perceptual hash                |
| -------- | -------------------------------------- | ------------------------------ |
| RAW      | CR2, CR3, NEF, ARW, ORF, RAF, RW2, DNG | No — SHA-256 and EXIF only     |
| Lossless | TIFF, PNG                              | Yes                            |
| Lossy    | JPEG, HEIC, WebP                       | JPEG and WebP yes, HEIC no     |

`PhotoFormat::supports_perceptual_hash()` is the gate. HEIC and RAW are
excluded from it because their decoders hang on some files; those formats
are still fully indexed by SHA-256 and EXIF, and phases 2 and 5 of the
matching pipeline exist so they still land in the right group.

The decode pipeline behind the supported formats, and why it is hand-rolled,
are [03 — Deduplication](03-deduplication.md)'s.

## Export

| Output              | Command            | What it produces                                                       |
| ------------------- | ------------------ | ---------------------------------------------------------------------- |
| Original, unmodified | `photopack pack`   | Byte-for-byte copy in content-addressable layout `{hash[..2]}/{hash}.{ext}` |
| HEIC (macOS)        | `photopack export` | `sips` conversion into `YYYY/MM/DD/`, 2-3x smaller than JPEG           |

## Sources

Local folders are the only scan source. The Google Photos API and NAS or SMB
network storage are designed and not started.

## Quality tiers

Which of these formats wins when a group elects its best version is
[04 — Source of truth](04-source-of-truth.md)'s, and lives nowhere else.
