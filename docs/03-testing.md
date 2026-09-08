# Testing

Three layers: unit tests beside the code, integration tests over a
subsystem, and end-to-end tests that drive the whole pipeline through the
public `Vault` API.

## The count

388 tests across the workspace, measured on 2026-09-02 with
`cargo test --workspace` on macOS.

| Where                   | Count | Kind                                    |
| ----------------------- | ----- | --------------------------------------- |
| `photopack-core`, unit  | 238   | Colocated in the source modules         |
| `photopack-cli`, unit   | 28    | Dashboard logic                         |
| `photopack-core`, e2e   | 122   | `crates/core/tests/vault_e2e.rs`         |

macOS-only HEIC tests are gated with `#[cfg(target_os = "macos")]`, so a
Linux run reports fewer and that is expected, not a regression.

## Where they live

Unit tests sit in a `#[cfg(test)]` module at the bottom of the file they
cover, under `crates/core/src/**` and `crates/cli/src/**`. Every end-to-end
test lives in the single file `crates/core/tests/vault_e2e.rs` and exercises
the full `Vault` surface: scan, match, rank, pack sync, HEIC export.

The per-module breakdown is the test tree itself. `cargo test` prints it; no
document mirrors it, because a mirrored listing goes stale within a week.

## What the suite pins

- Schema safety. Version tracking on a fresh database, on reopen, on a
  pre-versioning upgrade, on rejection of a future version, and on a
  repeated migrate. Structure pinning of tables, indexes, columns and the
  SQL snapshot. Foreign-key enforcement. Data surviving close and reopen.
  All of it for the catalog and for the pack manifest alike.
- No false positives. Structurally different photos — gradient against
  checkerboard against stripes, varying resolutions, colour-shifted variants
  — are never grouped. This is the regression class the e2e suite is built
  around.
- The matching safeguards, each as a named case rather than incidental
  behaviour: dual-hash consensus, the near-certain EXIF threshold of 2 bits,
  and the sequential-shot filter.
- Quality preservation across every format tier combination, and the pack
  replacing a lower-quality file when a better one becomes the source of
  truth.
- Platform gating around `sips`.

## The e2e fixture technique

No binary fixture ships with the suite. Images are generated with the
`image` crate into `tempfile` directories, using structurally distinct
patterns so their perceptual hashes stay apart — colour-only differences are
not enough, and a test that relies on them will pass for the wrong reason.

Cross-format cases use `create_file_with_jpeg_bytes()`, which writes JPEG
bytes into a file named `.cr2`, `.heic` or `.dng`. The scanner assigns
format from the extension and hashing works on raw bytes, so the RAW and
HEIC paths are exercised without a RAW or HEIC encoder anywhere in the
build.

## Running them

```bash
cargo test --workspace        # everything
cargo test -p photopack-core  # core unit and e2e
```

The lint and format gates are not testing — they are
[02 — Developing](02-developing.md)'s.
