# Pack and export

Two outputs with opposite natures: an archive you must be able to trust
forever, and a product you regenerate at will. The split is
[ADR-003](decisions/003-pack-export-split-content-addressable.md).

## Pack — the lossless archive

`photopack pack <path>` copies every source-of-truth file, byte for byte,
into a content-addressable store. It is permanent: removing a source later
does not remove what the pack already holds.

- A file is named by its SHA-256 with a two-character shard,
  `{hash[..2]}/{hash}.{ext}`. Deduplication is structural — the same hash is
  the same file — and no collision handling is needed.
- An embedded SQLite manifest at `.photopack/manifest.sqlite` maps each hash
  back to its metadata: original filename, format, size, EXIF.
- For each duplicate group only the source of truth is copied; ungrouped
  photos are copied as they are.
- When a better format becomes the source of truth, its hash-named file is
  written alongside and the entries no longer in the desired hash set are
  cleaned out through the manifest.
- Re-running skips any file whose hash-named target already exists.
- `set_vault_path` stores the destination in the catalog and registers the
  pack directory as a scan source, idempotently. `photopack pack` with no
  argument re-syncs the stored path.

The copy is parallel, through `rayon`.

## Export — the compressed library

`photopack export <path>` converts the same selection — source of truth plus
ungrouped photos — into HEIC. It reads from the catalog, meaning the source
directories; a pack is never required, and export never touches one.

The destination is not persisted. It is given on every run, deliberately: an
export is disposable output.

1. Select the photos.
2. Compute a date for each, from EXIF capture date, falling back to mtime.
3. Build the target path, `<dir>/YYYY/MM/DD/<stem>.heic`. Date and stem keep
   photos apart without a collision scheme.
4. Skip the file when a `.heic` already exists there, which is what makes a
   re-export incremental.
5. Convert in parallel through `rayon`.
6. Emit progress events: start, converted, skipped, complete.

## HEIC, and why `sips`

HEIC is a HEIF container carrying HEVC (H.265) compression — the default
photo format on Apple devices since iOS 11. It is 2 to 3 times smaller than
JPEG at equivalent visual quality, and it preserves EXIF and ICC profiles.

Conversion shells out to the macOS `sips` command:

```rust
Command::new("sips")
    .arg("-s").arg("format").arg("heic")
    .arg("-s").arg("formatOptions").arg(quality.to_string())
    .arg(&source)
    .arg("--out").arg(&target)
    .output()?;
```

`sips` ships with every macOS install, so it costs no dependency. It drives
Apple's VideoToolbox hardware H.265 encoder — the same encoder iCloud Photos
uses, at original dimensions with no downscaling — and it converts anything
macOS can decode, RAW included.

The cost is the platform. On anything but macOS, `photopack export` returns
`SipsNotAvailable`. Code paths that call it are gated with
`#[cfg(target_os = "macos")]`.

## Quality

`--quality` takes 0 to 100 and defaults to 85.

| Quality | Size against JPEG | Visible difference             | Use                 |
| ------- | ----------------- | ------------------------------ | ------------------- |
| 95      | ~40% smaller      | None                           | Archival            |
| 85      | ~60% smaller      | Effectively none               | The default         |
| 75      | ~70% smaller      | Barely perceptible             | Maximum savings     |
| 50      | ~80% smaller      | Noticeable on close inspection | Not recommended     |
