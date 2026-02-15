mod commands;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use losslessvault_core::Vault;

/// LosslessVault — photo deduplication engine
#[derive(Parser)]
#[command(name = "lsvault", version, about)]
struct Cli {
    /// Path to the catalog database
    #[arg(long, default_value_t = default_catalog_path())]
    catalog: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage photo sources: add, rm, scan, or list directories
    Sources {
        #[command(subcommand)]
        action: Option<SourcesAction>,
    },
    /// Query the catalog: status overview, file list, or duplicate groups
    Catalog {
        #[command(subcommand)]
        action: Option<CatalogAction>,
    },
    /// Manage the vault: a permanent lossless archive of your best photos
    Vault {
        #[command(subcommand)]
        action: VaultAction,
    },
    /// Export optimized HEIC photos from your catalog (macOS)
    Export {
        #[command(subcommand)]
        action: Option<ExportAction>,

        /// HEIC quality (0-100, default: 85)
        #[arg(long, default_value_t = 85)]
        quality: u8,
    },
}

#[derive(Subcommand)]
enum SourcesAction {
    /// Register a directory as a photo source
    Add {
        /// Path to the photo directory
        path: PathBuf,
    },
    /// Scan all sources for photos and find duplicates
    Scan,
    /// Unregister a source and remove its photos from the catalog
    Rm {
        /// Path to the source directory
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum CatalogAction {
    /// Show the full files table with roles and vault eligibility
    List,
    /// List all duplicate groups, or show details of a specific group
    Duplicates {
        /// Group ID (omit to list all)
        id: Option<i64>,
    },
}

#[derive(Subcommand)]
enum VaultAction {
    /// Set the vault directory for archiving deduplicated originals
    Set {
        /// Path to the vault directory
        path: PathBuf,
    },
    /// Sync deduplicated best-quality photos to the vault (byte-for-byte copies)
    Sync,
}

#[derive(Subcommand)]
enum ExportAction {
    /// Set the export destination directory
    Set {
        /// Path to the export directory
        path: PathBuf,
    },
    /// Show the current export destination
    Show,
}

fn default_catalog_path() -> String {
    dirs_path().to_string_lossy().to_string()
}

fn dirs_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".losslessvault")
        .join("catalog.db")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let catalog_path = PathBuf::from(&cli.catalog);
    let mut vault = Vault::open(&catalog_path)?;

    match cli.command {
        Commands::Sources { action } => match action {
            None => commands::sources::list(&vault)?,
            Some(SourcesAction::Add { path }) => commands::sources::add(&vault, path)?,
            Some(SourcesAction::Scan) => commands::sources::scan(&mut vault)?,
            Some(SourcesAction::Rm { path }) => commands::sources::rm(&vault, path)?,
        },
        Commands::Catalog { action } => match action {
            None => commands::status::run(&vault, false)?,
            Some(CatalogAction::List) => commands::status::run(&vault, true)?,
            Some(CatalogAction::Duplicates { id }) => commands::duplicates::run(&vault, id)?,
        },
        Commands::Vault { action } => match action {
            VaultAction::Set { path } => commands::vault::set(&vault, path)?,
            VaultAction::Sync => commands::vault::sync(&mut vault)?,
        },
        Commands::Export { action, quality } => match action {
            Some(ExportAction::Set { path }) => commands::export::set(&vault, path)?,
            Some(ExportAction::Show) => commands::export::show(&vault)?,
            None => commands::export::run(&vault, quality)?,
        },
    }

    Ok(())
}
