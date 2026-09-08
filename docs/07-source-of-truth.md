# Source of truth

Every duplicate group elects one member as the version that represents the
photo — the highest-quality original among them.

## Format hierarchy

Format decides first, in descending order:

```
RAW (.cr2, .cr3, .nef, .arw, .orf, .raf, .rw2, .dng)
  > TIFF (.tiff, .tif)
    > PNG (.png)
      > JPEG (.jpg, .jpeg)
        > HEIC (.heic, .heif)
          > WebP (.webp)
```

A RAW always beats a JPEG of the same photo; a JPEG always beats a HEIC.

## Tiebreakers

Within one tier, the largest file wins. Size is the most reliable proxy for
"least compressed" between two files of the same format.

Still tied, the oldest modification time wins. The earliest file is the more
likely original, a later one the more likely re-export. This is a weak
signal and is only reached when size is inconclusive.

## The election

```
duplicate group
    │
    ▼
sort by format quality tier
    │
    ▼
same tier → largest file size
    │
    ▼
still tied → oldest mtime
    │
    ▼
winner = source of truth; the rest are duplicates
```

The winner is the file both `photopack pack` (copied unmodified) and
`photopack export` (converted to HEIC) act on. The others are excluded from
both.

## Worked examples

| Group | Files                                 | Elected    | Why                            |
| ----- | ------------------------------------- | ---------- | ------------------------------ |
| A     | CR2 (25 MB), JPEG (8 MB), HEIC (3 MB) | CR2        | RAW beats every other tier     |
| B     | JPEG (12 MB), JPEG (4 MB)             | JPEG 12 MB | Same tier, larger file         |
| C     | HEIC (5 MB), JPEG (8 MB)              | JPEG 8 MB  | JPEG tier beats HEIC tier      |
| D     | TIFF (45 MB), DNG (30 MB)             | DNG        | RAW tier beats TIFF tier       |
| E     | CR2 (25 MB), DNG (30 MB)              | DNG 30 MB  | Both RAW tier, larger file     |
