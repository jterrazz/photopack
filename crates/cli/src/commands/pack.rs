use std::path::PathBuf;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use photopack_core::{export::ExportProgress, vault_save::VaultSaveProgress, Vault};

pub fn run(vault: &mut Vault, path: Option<PathBuf>, heic: bool, quality: u8) -> Result<()> {
    if heic {
        run_heic(vault, path, quality)
    } else {
        run_lossless(vault, path)
    }
}

fn run_lossless(vault: &mut Vault, path: Option<PathBuf>) -> Result<()> {
    if let Some(path) = path {
        vault.set_vault_path(&path)?;
        let resolved = vault.get_vault_path()?.unwrap();
        println!("Vault path set to: {}", resolved.display());
        println!("Vault registered as scan source.");
    }

    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    vault.vault_save(Some(&mut |progress| match progress {
        VaultSaveProgress::Start { total } => {
            pb.set_length(total as u64);
            pb.set_position(0);
            pb.set_message("Syncing photos to vault...");
        }
        VaultSaveProgress::Copied { target, .. } => {
            pb.inc(1);
            pb.set_message(format!("-> {}", target.display()));
        }
        VaultSaveProgress::Skipped { .. } => {
            pb.inc(1);
        }
        VaultSaveProgress::Removed { path } => {
            pb.set_message(format!("removed superseded: {}", path.display()));
        }
        VaultSaveProgress::Complete {
            copied,
            skipped,
            removed,
        } => {
            let mut msg = format!("{copied} copied, {skipped} skipped");
            if removed > 0 {
                msg.push_str(&format!(", {removed} superseded removed"));
            }
            pb.finish_with_message(msg);
        }
    }))?;

    println!("Vault sync complete.");
    Ok(())
}

fn run_heic(vault: &mut Vault, path: Option<PathBuf>, quality: u8) -> Result<()> {
    if let Some(path) = path {
        vault.set_export_path(&path)?;
        let resolved = vault.get_export_path()?.unwrap();
        println!("Export path set to: {}", resolved.display());
    }

    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    vault.export(
        quality,
        Some(&mut |progress| match progress {
            ExportProgress::Start { total } => {
                pb.set_length(total as u64);
                pb.set_position(0);
                pb.set_message("Converting photos to HEIC...");
            }
            ExportProgress::Converted { target, .. } => {
                pb.inc(1);
                pb.set_message(format!("-> {}", target.display()));
            }
            ExportProgress::Skipped { .. } => {
                pb.inc(1);
            }
            ExportProgress::Complete {
                converted,
                skipped,
            } => {
                pb.finish_with_message(format!("{converted} converted, {skipped} skipped"));
            }
        }),
    )?;

    println!("Export complete.");
    Ok(())
}
