use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("image processing error: {0}")]
    Image(#[from] image::ImageError),

    #[error("EXIF parsing error: {0}")]
    Exif(#[from] exif::Error),

    #[error("walkdir error: {0}")]
    WalkDir(#[from] walkdir::Error),

    #[error("source path does not exist: {}", .0.display())]
    SourceNotFound(PathBuf),

    #[error("source path is not a directory: {}", .0.display())]
    SourceNotDirectory(PathBuf),

    #[error("source already registered: {}", .0.display())]
    SourceAlreadyExists(PathBuf),

    #[error("source not registered: {}", .0.display())]
    SourceNotRegistered(PathBuf),

    #[error("group not found: {0}")]
    GroupNotFound(i64),

    #[error("unsupported file format: {}", .0.display())]
    UnsupportedFormat(PathBuf),

    #[error("vault path not configured — run `photopack pack <path>` first")]
    VaultPathNotSet,

    #[error("vault path does not exist: {}", .0.display())]
    VaultPathNotFound(PathBuf),

    #[error("export path not configured — run `photopack pack <path> --heic` first")]
    ExportPathNotSet,

    #[error("export path does not exist: {}", .0.display())]
    ExportPathNotFound(PathBuf),

    #[error("failed to convert {}: {message}", .path.display())]
    ConversionFailed { path: PathBuf, message: String },

    #[error("sips command not available — this feature requires macOS")]
    SipsNotAvailable,
}

pub type Result<T> = std::result::Result<T, Error>;
