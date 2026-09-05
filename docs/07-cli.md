# CLI

One binary, `photopack`, with a flat command set — no nested subcommands.
Flags and help text belong to `photopack --help`; this chapter carries the
families and the invariants help does not state.

## Command families

| Family     | Commands                          | Role                                            |
| ---------- | --------------------------------- | ----------------------------------------------- |
| Sources    | `add <path>`, `rm <path>`, `scan` | Register folders and index their photos         |
| Inspection | `status`, `ls`, `ls --dupes [id]` | Dashboard, files table, duplicate groups        |
| Pack       | `pack [<path>]`                   | Lossless content-addressable archive            |
| Export     | `export <path> [--quality N]`     | Compressed HEIC library, macOS, default quality 85 |

Every command accepts `--catalog <path>`; without it the catalog is
`~/.photopack/catalog.db`.

## Invariants

- Sources are never touched. `scan` reads, and `rm` removes only catalog
  metadata — the source's photos, plus any group left empty or without a
  source of truth. A file on disk is never modified or deleted.
- `add` registers, `scan` indexes. Adding a source does not scan it. `scan`
  walks every registered source, hashes, runs the
  [five-phase pipeline](03-deduplication.md), elects a source of truth per
  group, and shows an `indicatif` progress bar.
- Scan is incremental and self-healing. Only files whose mtime changed are
  processed, files that disappeared from disk are removed from the catalog,
  and duplicate groups are rebuilt from scratch each time for correctness.
- `ls` shows roles. Each file with its source, format, size, group ID, role
  (best copy, duplicate, unique) and pack eligibility, sorted by group with
  the source of truth first and blank rows between groups. `--dupes` lists
  the groups, `--dupes <id>` details one and marks its elected version.
- `pack` is the permanent archive, `export` is disposable output, and their
  asymmetry is deliberate — [06 — Pack and export](06-pack-and-export.md)
  carries both.

## Dashboard

`photopack status` prints three things: an overview (photo count, unique
count, duplicate groups, disk usage, estimated savings, source count, pack
path), a per-source table (photos, total size, last scanned), and the pack
destination.

`photopack ls` prints the files table itself. Tables are drawn with
`comfy-table`.

The logic behind the dashboard is extracted from the printing —
`StatusData`, `compute_aggregates`, `compute_source_stats`,
`sort_photos_for_display` and their neighbours are plain functions, which is
what lets them be unit-tested without capturing stdout.
