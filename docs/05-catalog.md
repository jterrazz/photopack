# Catalog

One SQLite file holds everything the engine knows: the indexed photos, the
duplicate groups it found, and the configuration it must remember.

## Why SQLite

A single file is portable across every platform, backs up with a copy, and
opens in any SQLite tool for inspection. There is no server process to
manage. WAL mode allows concurrent reads while a scan writes, which matters
because scanning is parallel. `rusqlite` is bundled, so the binary carries
no system dependency.

The catalog defaults to `~/.photopack/catalog.db`; every command takes
`--catalog <path>` to point elsewhere.

One catalog is one library. Multiple sources feed the same file, which is
what makes cross-source deduplication possible — a photo in one folder can
be matched against a photo in another.

## Tables

| Table              | One row per                                                                      |
| ------------------ | -------------------------------------------------------------------------------- |
| `photos`           | Indexed file: path, size, mtime, format, sha256, phash, dhash, EXIF fields, source |
| `sources`          | Registered scan directory, with its last-scanned timestamp                       |
| `duplicate_groups` | Group, with its source-of-truth photo and its confidence level                    |
| `group_members`    | Membership link between a group and a photo                                       |
| `config`           | Setting: `vault_path`, `phash_version`, `schema_version`                          |

## Indexes

| Index                      | Serves                                            |
| -------------------------- | ------------------------------------------------- |
| `idx_photos_sha256`        | Exact-duplicate lookup in phase 1                 |
| `idx_photos_source`        | Per-source queries                                |
| `idx_photos_path`          | Path lookup during an incremental scan            |
| `idx_photos_source_mtime`  | The batch mtime check that drives incremental scan |
| `idx_group_members_photo`  | Photo-to-group joins                              |

## Schema versioning

The `config` table carries `schema_version`, currently 1. On open,
`schema::migrate()` runs every pending migration inside one transaction. A
database written by a newer build fails to open with `SchemaTooNew` rather
than being read wrongly. A pre-versioning database upgrades to v1 on first
open.

To add a migration: raise `SCHEMA_VERSION`, write `migrate_vN_to_vM()`, and
append it to the `MIGRATIONS` array. Nothing else is needed; the runner
walks the array.

## Batching

Bulk work goes through two entry points rather than a loop of statements.
`upsert_photos_batch()` writes every photo of a scan in one transaction, and
`replace_groups_batch()` clears and rebuilds all duplicate groups in
another. Both turn O(n) round-trips into one.

Two read paths are consolidated the same way. `list_groups()` is a single
join across `duplicate_groups`, `group_members` and `photos` instead of a
query per group, and `stats_summary()` returns the photo count, the group
count and the duplicate-member count from one query with three subqueries.

## What persists, and what does not

The pack path lives in `config` and survives across sessions. The HEIC
export destination is deliberately not stored — `photopack export` takes its
path on every run. The asymmetry is the point of
[ADR-003](decisions/003-pack-export-split-content-addressable.md): the
archive is the one fixed place, an export is disposable output.

## Designed, not shipped

Albums and tags derived from source-of-truth metadata. Neither the scope nor
the surface is decided.
