use std::io::BufReader;
use std::path::Path;

use fast_image_resize::{self as fir, images::Image as FirImage};

/// Compute average hash (aHash) and difference hash (dHash) for an image.
/// The aHash is stored in the `phash` field for historical reasons.
/// Returns (ahash, dhash) as u64 values, or None if the image cannot be processed.
/// Both hashes are 8x8 = 64-bit. Matching requires dual-hash consensus (both within threshold).
///
/// Uses a hybrid decode strategy:
/// - JPEG: `turbojpeg` full-resolution grayscale decode (feature-gated, skips chroma)
/// - Other formats: `image` crate decode, RGB resize to 9x8, then grayscale conversion
///
/// Both paths apply EXIF orientation before resizing, so photos with rotation tags
/// (common on iPhone originals) produce the same hash as physically-rotated exports.
///
/// Both paths produce a 9x8 grayscale buffer for manual aHash + dHash computation.
/// Full-resolution decode is critical — DCT scaling changes frequency-domain coefficients
/// differently for recompressed JPEGs, causing hash divergence beyond threshold.
pub fn compute_perceptual_hashes(path: &Path) -> Option<(u64, u64)> {
    let pixels = load_9x8_grayscale(path)?;
    let ahash = compute_ahash(&pixels);
    let dhash = compute_dhash(&pixels);
    Some((ahash, dhash))
}

/// Load image and produce a 9x8 grayscale pixel buffer ready for hashing.
fn load_9x8_grayscale(path: &Path) -> Option<[u8; 72]> {
    // JPEG: turbojpeg full-res grayscale → orientation → resize to 9x8
    #[cfg(feature = "turbojpeg")]
    if is_jpeg(path) {
        if let Some(buf) = load_jpeg_9x8(path) {
            return Some(buf);
        }
    }

    // Other formats: image crate → orientation → RGB resize to 9x8 → grayscale
    load_image_crate_9x8(path)
}

/// Check if a file is JPEG by extension.
#[cfg(feature = "turbojpeg")]
fn is_jpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
}

/// Read EXIF orientation tag (1-8). Returns 1 (normal) if missing or unreadable.
fn read_exif_orientation(path: &Path) -> u8 {
    let read = || -> Option<u8> {
        let file = std::fs::File::open(path).ok()?;
        let mut reader = BufReader::new(file);
        let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
        let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
        field.value.get_uint(0).map(|v| v as u8)
    };
    read().unwrap_or(1)
}

/// Apply EXIF orientation to a grayscale buffer, returning the corrected buffer
/// and new dimensions. Handles all 8 EXIF orientation values.
///
/// Orientations:
/// 1: Normal                    5: Mirror + rotate 90° CW
/// 2: Mirror horizontal         6: Rotate 90° CW
/// 3: Rotate 180°               7: Mirror + rotate 90° CCW
/// 4: Mirror vertical           8: Rotate 90° CCW
fn apply_orientation(buf: &[u8], w: usize, h: usize, orientation: u8) -> (Vec<u8>, usize, usize) {
    match orientation {
        1 => (buf.to_vec(), w, h),
        2 => {
            // Mirror horizontal
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = buf[y * w + (w - 1 - x)];
                }
            }
            (out, w, h)
        }
        3 => {
            // Rotate 180°
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = buf[(h - 1 - y) * w + (w - 1 - x)];
                }
            }
            (out, w, h)
        }
        4 => {
            // Mirror vertical
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = buf[(h - 1 - y) * w + x];
                }
            }
            (out, w, h)
        }
        5 => {
            // Transpose (mirror + rotate 90° CW)
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[x * h + y] = buf[y * w + x];
                }
            }
            (out, h, w)
        }
        6 => {
            // Rotate 90° CW
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[x * h + (h - 1 - y)] = buf[y * w + x];
                }
            }
            (out, h, w)
        }
        7 => {
            // Transverse (mirror + rotate 90° CCW)
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[(w - 1 - x) * h + (h - 1 - y)] = buf[y * w + x];
                }
            }
            (out, h, w)
        }
        8 => {
            // Rotate 90° CCW
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[(w - 1 - x) * h + y] = buf[y * w + x];
                }
            }
            (out, h, w)
        }
        _ => (buf.to_vec(), w, h),
    }
}

/// Decode JPEG at full resolution directly to grayscale using turbojpeg,
/// apply EXIF orientation, then SIMD-resize to 9x8.
///
/// Pipeline: turbojpeg GRAY format (full res) → EXIF orientation → fast_image_resize 9x8
/// Skips chroma decode entirely (1 byte/pixel instead of 3).
/// Full-resolution decode is required — DCT scaling produces different
/// intermediate pixels for recompressed JPEGs, causing hash divergence.
#[cfg(feature = "turbojpeg")]
fn load_jpeg_9x8(path: &Path) -> Option<[u8; 72]> {
    let jpeg_data = std::fs::read(path).ok()?;
    let mut decompressor = turbojpeg::Decompressor::new().ok()?;
    let header = decompressor.read_header(&jpeg_data).ok()?;
    let w = header.width;
    let h = header.height;

    // Decode directly to grayscale at full resolution (skips chroma, 1 byte/pixel)
    let mut buf = vec![0u8; w * h];
    let output = turbojpeg::Image {
        pixels: buf.as_mut_slice(),
        width: w,
        pitch: w,
        height: h,
        format: turbojpeg::PixelFormat::GRAY,
    };
    decompressor.decompress(&jpeg_data, output).ok()?;

    // Apply EXIF orientation before resize
    let orientation = read_exif_orientation(path);
    let (buf, w, h) = apply_orientation(&buf, w, h, orientation);

    // SIMD resize grayscale to 9x8
    let src = FirImage::from_vec_u8(w as u32, h as u32, buf, fir::PixelType::U8).ok()?;
    let mut dst = FirImage::new(9, 8, fir::PixelType::U8);
    fir::Resizer::new().resize(&src, &mut dst, None).ok()?;

    let mut pixels = [0u8; 72];
    pixels.copy_from_slice(&dst.buffer()[..72]);
    Some(pixels)
}

/// Apply EXIF orientation to an RGB buffer, returning corrected buffer and new dimensions.
fn apply_orientation_rgb(buf: &[u8], w: usize, h: usize, orientation: u8) -> (Vec<u8>, usize, usize) {
    if orientation == 1 {
        return (buf.to_vec(), w, h);
    }

    let pixel_count = w * h;
    let mut out = vec![0u8; pixel_count * 3];
    let (new_w, new_h) = if orientation >= 5 { (h, w) } else { (w, h) };

    for y in 0..h {
        for x in 0..w {
            let src_idx = (y * w + x) * 3;
            let (dx, dy) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (h - 1 - y, x),
                7 => (h - 1 - y, w - 1 - x),
                8 => (y, w - 1 - x),
                _ => (x, y),
            };
            let dst_idx = (dy * new_w + dx) * 3;
            out[dst_idx..dst_idx + 3].copy_from_slice(&buf[src_idx..src_idx + 3]);
        }
    }
    (out, new_w, new_h)
}

/// Decode any supported format using the `image` crate, apply EXIF orientation,
/// resize RGB to 9x8, then convert only those 72 pixels to grayscale.
/// Avoids full-resolution grayscale conversion (e.g., 12MP × BT.601 per pixel).
fn load_image_crate_9x8(path: &Path) -> Option<[u8; 72]> {
    let img = image::open(path).ok()?;
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);

    // Apply EXIF orientation before resize
    let orientation = read_exif_orientation(path);
    let (rgb_data, w, h) = apply_orientation_rgb(rgb.as_raw(), w, h, orientation);

    // SIMD resize RGB to 9x8 (216 bytes output instead of millions)
    let src = FirImage::from_vec_u8(w as u32, h as u32, rgb_data, fir::PixelType::U8x3).ok()?;
    let mut dst = FirImage::new(9, 8, fir::PixelType::U8x3);
    fir::Resizer::new().resize(&src, &mut dst, None).ok()?;

    // Convert 72 RGB pixels to grayscale using BT.601
    let rgb_buf = dst.buffer();
    let mut gray = [0u8; 72];
    for i in 0..72 {
        let r = rgb_buf[i * 3] as f32;
        let g = rgb_buf[i * 3 + 1] as f32;
        let b = rgb_buf[i * 3 + 2] as f32;
        gray[i] = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
    }
    Some(gray)
}

/// Compute average hash (aHash) from 9x8 grayscale pixels.
/// Uses the left 8x8 block. Each bit = 1 if pixel >= mean, 0 otherwise.
fn compute_ahash(pixels: &[u8]) -> u64 {
    // Extract 8x8 block from 9-wide rows
    let mut block = [0u8; 64];
    for row in 0..8 {
        for col in 0..8 {
            block[row * 8 + col] = pixels[row * 9 + col];
        }
    }

    let mean: u64 = block.iter().map(|&p| p as u64).sum::<u64>() / 64;
    let mut hash: u64 = 0;
    for (i, &pixel) in block.iter().enumerate() {
        if pixel as u64 >= mean {
            hash |= 1 << i;
        }
    }
    hash
}

/// Compute difference hash (dHash) from 9x8 grayscale pixels.
/// For each row of 9 pixels, compare adjacent pairs → 8 bits per row × 8 rows = 64 bits.
fn compute_dhash(pixels: &[u8]) -> u64 {
    let mut hash: u64 = 0;
    let mut bit = 0;
    for row in 0..8 {
        for col in 0..8 {
            let left = pixels[row * 9 + col];
            let right = pixels[row * 9 + col + 1];
            if left > right {
                hash |= 1 << bit;
            }
            bit += 1;
        }
    }
    hash
}

/// Compute the Hamming distance between two hash values.
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_jpeg(path: &Path, r: u8, g: u8, b: u8) {
        let img = image::RgbImage::from_fn(64, 64, |_, _| image::Rgb([r, g, b]));
        img.save(path).unwrap();
    }

    #[test]
    fn test_hamming_distance_identical() {
        assert_eq!(hamming_distance(0, 0), 0);
        assert_eq!(hamming_distance(u64::MAX, u64::MAX), 0);
    }

    #[test]
    fn test_hamming_distance_different() {
        assert_eq!(hamming_distance(0, 1), 1);
        assert_eq!(hamming_distance(0, 3), 2);
        assert_eq!(hamming_distance(0, u64::MAX), 64);
    }

    #[test]
    fn test_compute_perceptual_hashes_returns_values() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.jpg");
        create_test_jpeg(&path, 128, 128, 128);

        let result = compute_perceptual_hashes(&path);
        assert!(result.is_some());
    }

    #[test]
    fn test_identical_images_same_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let path_a = tmp.path().join("a.jpg");
        let path_b = tmp.path().join("b.jpg");
        create_test_jpeg(&path_a, 200, 100, 50);
        create_test_jpeg(&path_b, 200, 100, 50);

        let (phash_a, dhash_a) = compute_perceptual_hashes(&path_a).unwrap();
        let (phash_b, dhash_b) = compute_perceptual_hashes(&path_b).unwrap();
        assert_eq!(phash_a, phash_b);
        assert_eq!(dhash_a, dhash_b);
    }

    #[test]
    fn test_different_images_different_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let path_a = tmp.path().join("gradient.jpg");
        let path_b = tmp.path().join("checkerboard.jpg");

        // Horizontal gradient
        let img_a = image::RgbImage::from_fn(64, 64, |x, _| {
            let v = (x * 4) as u8;
            image::Rgb([v, 0, 0])
        });
        img_a.save(&path_a).unwrap();

        // Checkerboard pattern
        let img_b = image::RgbImage::from_fn(64, 64, |x, y| {
            if (x / 8 + y / 8) % 2 == 0 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 0, 0])
            }
        });
        img_b.save(&path_b).unwrap();

        let (phash_a, _) = compute_perceptual_hashes(&path_a).unwrap();
        let (phash_b, _) = compute_perceptual_hashes(&path_b).unwrap();
        assert_ne!(phash_a, phash_b);
    }

    #[test]
    fn test_nonexistent_file_returns_none() {
        let result = compute_perceptual_hashes(Path::new("/nonexistent/image.jpg"));
        assert!(result.is_none());
    }

    #[test]
    fn test_non_image_file_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("not_an_image.jpg");
        std::fs::write(&path, b"this is not a jpeg").unwrap();

        let result = compute_perceptual_hashes(&path);
        assert!(result.is_none());
    }

    #[test]
    fn test_ahash_dhash_manual() {
        // 9x8 = 72 pixels, all 100 except a bright spot
        let mut pixels = [100u8; 72];
        pixels[0] = 200; // one bright pixel

        let ahash = compute_ahash(&pixels);
        let dhash = compute_dhash(&pixels);

        // ahash: only pixel[0] > mean(~101), so bit 0 set
        assert_ne!(ahash, 0);
        // dhash: first pair 200 > 100, so bit 0 set
        assert_ne!(dhash, 0);
    }

    #[test]
    fn test_png_support() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.png");
        let img = image::RgbImage::from_fn(32, 32, |_, _| image::Rgb([100, 150, 200]));
        img.save(&path).unwrap();

        let result = compute_perceptual_hashes(&path);
        assert!(result.is_some());
    }

    #[test]
    fn test_apply_orientation_identity() {
        let buf: Vec<u8> = (0..12).collect(); // 4x3
        let (out, w, h) = apply_orientation(&buf, 4, 3, 1);
        assert_eq!((w, h), (4, 3));
        assert_eq!(out, buf);
    }

    #[test]
    fn test_apply_orientation_rotate_90_cw() {
        // 3x2 grid:
        // [1, 2, 3]
        // [4, 5, 6]
        let buf = vec![1, 2, 3, 4, 5, 6];
        let (out, w, h) = apply_orientation(&buf, 3, 2, 6);
        // Rotated 90° CW → 2x3:
        // [4, 1]
        // [5, 2]
        // [6, 3]
        assert_eq!((w, h), (2, 3));
        assert_eq!(out, vec![4, 1, 5, 2, 6, 3]);
    }

    #[test]
    fn test_apply_orientation_rotate_180() {
        let buf = vec![1, 2, 3, 4, 5, 6];
        let (out, w, h) = apply_orientation(&buf, 3, 2, 3);
        // Rotated 180° → 3x2:
        // [6, 5, 4]
        // [3, 2, 1]
        assert_eq!((w, h), (3, 2));
        assert_eq!(out, vec![6, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn test_apply_orientation_rotate_90_ccw() {
        let buf = vec![1, 2, 3, 4, 5, 6];
        let (out, w, h) = apply_orientation(&buf, 3, 2, 8);
        // Rotated 90° CCW → 2x3:
        // [3, 6]
        // [2, 5]
        // [1, 4]
        assert_eq!((w, h), (2, 3));
        assert_eq!(out, vec![3, 6, 2, 5, 1, 4]);
    }
}
