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
    /// Manage photo sources: add, scan, or list directories
    Sources {
        #[command(subcommand)]
        action: Option<SourcesAction>,
    },
    /// Show catalog status summary
    Status {
        /// Show the full files table
        #[arg(long)]
        files: bool,
    },
    /// List all duplicate groups, or show details of a specific group
    Duplicates {
        /// Group ID (omit to list all)
        id: Option<i64>,
    },
    /// Manage the vault export destination
    Vault {
        #[command(subcommand)]
        action: VaultAction,
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
}

#[derive(Subcommand)]
enum VaultAction {
    /// Set the vault destination path
    Set {
        /// Path to the vault directory
        path: PathBuf,
    },
    /// Show the current vault path
    Show,
    /// Copy deduplicated best-quality photos to the vault
    Save,
    /// Set the export destination path for HEIC conversion
    ExportSet {
        /// Path to the export directory
        path: PathBuf,
    },
    /// Show the current export path
    ExportShow,
    /// Export deduplicated photos as HEIC files (macOS only)
    Export {
        /// HEIC quality (0-100, default: 85)
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
        },
        Commands::Status { files } => commands::status::run(&vault, files)?,
        Commands::Duplicates { id } => commands::duplicates::run(&vault, id)?,
        Commands::Vault { action } => match action {
            VaultAction::Set { path } => commands::vault::set(&vault, path)?,
            VaultAction::Show => commands::vault::show(&vault)?,
            VaultAction::Save => commands::vault::save(&mut vault)?,
            VaultAction::ExportSet { path } => commands::vault::export_set(&vault, path)?,
            VaultAction::ExportShow => commands::vault::export_show(&vault)?,
            VaultAction::Export { quality } => commands::vault::export(&vault, quality)?,
        },
    }

    Ok(())
}
