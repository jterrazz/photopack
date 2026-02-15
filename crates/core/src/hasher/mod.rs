pub mod perceptual;

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Compute the SHA-256 hash of a file's contents using streaming I/O.
/// Reads in 64KB chunks to avoid loading large files entirely into memory.
pub fn compute_sha256(path: &Path) -> std::io::Result<String> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let result = hasher.finalize();
    Ok(format!("{:x}", result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_sha256_consistency() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.bin");
        fs::write(&path, b"hello world").unwrap();

        let hash1 = compute_sha256(&path).unwrap();
        let hash2 = compute_sha256(&path).unwrap();
        assert_eq!(hash1, hash2);
        // Known SHA-256 of "hello world"
        assert_eq!(
            hash1,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_sha256_different_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path_a = tmp.path().join("a.bin");
        let path_b = tmp.path().join("b.bin");
        fs::write(&path_a, b"content A").unwrap();
        fs::write(&path_b, b"content B").unwrap();

        let hash_a = compute_sha256(&path_a).unwrap();
        let hash_b = compute_sha256(&path_b).unwrap();
        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn test_sha256_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.bin");
        fs::write(&path, b"").unwrap();

        let hash = compute_sha256(&path).unwrap();
        // Known SHA-256 of empty string
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sha256_nonexistent_file() {
        let result = compute_sha256(Path::new("/nonexistent/file.bin"));
        assert!(result.is_err());
    }
}
