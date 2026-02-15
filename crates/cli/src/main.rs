mod commands;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use photopack_core::Vault;

/// Photopack — pack your photo library tight
#[derive(Parser)]
#[command(name = "photopack", version, about)]
struct Cli {
    /// Path to the catalog database
    #[arg(long, default_value_t = default_catalog_path())]
    catalog: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Register a directory as a photo source
    Add {
        /// Path to the photo directory
        path: PathBuf,
    },
    /// Unregister a source and remove its photos from the catalog
    Rm {
        /// Path to the source directory
        path: PathBuf,
    },
    /// Scan all sources for photos and find duplicates
    Scan,
    /// Show catalog dashboard (overview, sources, vault info)
    Status {
        /// Show file table with roles and vault eligibility
        #[arg(long)]
        files: bool,
    },
    /// List duplicate groups, or show details of a specific group
    Dupes {
        /// Group ID (omit to list all)
        id: Option<i64>,
    },
    /// Pack best-quality originals into a permanent lossless archive
    Pack {
        /// Destination directory (saved for future runs)
        path: Option<PathBuf>,
    },
    /// Export compressed HEIC photos for space savings (macOS)
    Export {
        /// Destination directory
        path: PathBuf,
        /// HEIC quality 0-100
        #[arg(long, default_value_t = 85)]
        quality: u8,
    },
}

fn default_catalog_path() -> String {
    dirs_path().to_string_lossy().to_string()
}

fn dirs_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".photopack")
        .join("catalog.db")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let catalog_path = PathBuf::from(&cli.catalog);
    let mut vault = Vault::open(&catalog_path)?;

    match cli.command {
        Commands::Add { path } => commands::sources::add(&vault, path)?,
        Commands::Rm { path } => commands::sources::rm(&vault, path)?,
        Commands::Scan => commands::sources::scan(&mut vault)?,
        Commands::Status { files } => commands::status::run(&vault, files)?,
        Commands::Dupes { id } => commands::duplicates::run(&vault, id)?,
        Commands::Pack { path } => commands::pack::run(&mut vault, path)?,
        Commands::Export { path, quality } => commands::export::run(&mut vault, &path, quality)?,
    }

    Ok(())
}
