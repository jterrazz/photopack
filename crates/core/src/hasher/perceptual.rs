use std::path::Path;

use fast_image_resize::{self as fir, images::Image as FirImage};

/// Compute average hash (aHash) and difference hash (dHash) for an image.
/// The aHash is stored in the `phash` field for historical reasons.
/// Returns (ahash, dhash) as u64 values, or None if the image cannot be processed.
/// Both hashes are 8x8 = 64-bit. Matching requires dual-hash consensus (both within threshold).
///
/// Uses a hybrid decode strategy:
/// - JPEG: `turbojpeg` for ~2-3x faster decode (feature-gated)
/// - Other formats: `image` crate decode
///
/// Both paths resize to 9x8 grayscale via SIMD-accelerated `fast_image_resize`,
/// then compute aHash + dHash from the same tiny buffer (no `img_hash` dependency).
pub fn compute_perceptual_hashes(path: &Path) -> Option<(u64, u64)> {
    let grayscale = load_grayscale(path)?;
    let (width, height) = (grayscale.width(), grayscale.height());

    // Resize to 9x8 grayscale using SIMD-accelerated resizer
    // 9 columns needed for dHash (gradient between adjacent pixels → 8 diffs)
    // 8 rows for both hashes (8x8 = 64 bits)
    let mut dst = FirImage::new(9, 8, fir::PixelType::U8);
    let mut resizer = fir::Resizer::new();
    let src = FirImage::from_vec_u8(width, height, grayscale.into_raw(), fir::PixelType::U8).ok()?;
    resizer.resize(&src, &mut dst, None).ok()?;

    let pixels = dst.buffer();
    let ahash = compute_ahash(pixels);
    let dhash = compute_dhash(pixels);
    Some((ahash, dhash))
}

/// Load image as grayscale buffer. Tries turbojpeg first for JPEG (faster decode),
/// then falls back to the `image` crate for all other formats.
fn load_grayscale(path: &Path) -> Option<image::GrayImage> {
    #[cfg(feature = "turbojpeg")]
    if is_jpeg(path) {
        if let Some(img) = load_jpeg_turbojpeg(path) {
            return Some(img);
        }
    }

    // Fallback: image crate (supports JPEG, PNG, TIFF, WebP)
    load_image_crate(path)
}

/// Check if a file is JPEG by extension.
#[cfg(feature = "turbojpeg")]
fn is_jpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
}

/// Decode JPEG at full resolution using turbojpeg (~2-3x faster than `image` crate).
/// Decodes to RGB, then converts to grayscale using the same ITU-R BT.601 formula
/// as the `image` crate (`to_luma8()`) for hash consistency across JPEG and PNG paths.
///
/// Note: We deliberately decode at full resolution (no DCT scaling) to preserve
/// spatial detail needed for distinguishing similar photos (e.g., sequential shots).
/// The performance gain comes from turbojpeg's optimized JPEG decoder (libjpeg-turbo)
/// plus SIMD-accelerated resize via `fast_image_resize`.
#[cfg(feature = "turbojpeg")]
fn load_jpeg_turbojpeg(path: &Path) -> Option<image::GrayImage> {
    let jpeg_data = std::fs::read(path).ok()?;
    let mut decompressor = turbojpeg::Decompressor::new().ok()?;
    let header = decompressor.read_header(&jpeg_data).ok()?;

    // Decompress to RGB at full resolution
    let w = header.width;
    let h = header.height;
    let mut buf = vec![0u8; w * h * 3];
    let output = turbojpeg::Image {
        pixels: buf.as_mut_slice(),
        width: w,
        pitch: w * 3,
        height: h,
        format: turbojpeg::PixelFormat::RGB,
    };
    decompressor.decompress(&jpeg_data, output).ok()?;

    let rgb = image::RgbImage::from_raw(w as u32, h as u32, buf)?;
    Some(image::DynamicImage::ImageRgb8(rgb).to_luma8())
}

/// Decode any supported format using the `image` crate, convert to grayscale.
fn load_image_crate(path: &Path) -> Option<image::GrayImage> {
    let img = image::open(path).ok()?;
    Some(img.to_luma8())
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
}
