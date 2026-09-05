# photopack

Pack your photo library tight. A Rust engine that finds duplicates across
formats, keeps the best version of each photo, and packs them into a clean
archive.

Your photos are a mess. The same shot is saved as an iPhone HEIC, a
Lightroom JPEG and a RAW backup, with copies in iCloud, on a USB drive and
in `~/old-photos`. After a few years, 30 to 50% of the library is redundant
— and you are paying to store it.

photopack scans every source you point it at, finds the duplicates that
byte-matching misses, elects the highest-quality version, and packs those
originals losslessly. Export to HEIC and the library is around three times
smaller, with no visible quality loss.

## Install

```bash
cargo install photopack
```

## Quick start

```bash
photopack add ~/Photos          # register a source, as many as you like
photopack add ~/iCloud
photopack scan                  # index and find duplicates across formats
photopack status                # what it found
photopack ls --dupes            # the duplicate groups
photopack pack ~/PhotoArchive   # lossless archive of the best originals
photopack export ~/Packed --quality 85   # compressed HEIC library (macOS)
```

The catalog lives at `~/.photopack/catalog.db`; `--catalog <path>` overrides
it. Formats scanned: CR2, CR3, NEF, ARW, ORF, RAF, RW2, DNG, TIFF, PNG,
JPEG, HEIC, WebP.

## Documentation

The manual is [docs/README.md](docs/README.md) — architecture, the matching
pipeline, the catalog, the command surface, the crate stack, the testing
doctrine, and the decisions behind them.

Agents start at [AGENTS.md](AGENTS.md).

## License

MIT / Apache 2.0.
