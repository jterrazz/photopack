use std::path::PathBuf;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use losslessvault_core::{export::ExportProgress, vault_save::VaultSaveProgress, Vault};

pub fn set(vault: &Vault, path: PathBuf) -> Result<()> {
    vault.set_vault_path(&path)?;
    let resolved = vault.get_vault_path()?.unwrap();
    println!("Vault path set to: {}", resolved.display());
    Ok(())
}

pub fn show(vault: &Vault) -> Result<()> {
    match vault.get_vault_path()? {
        Some(path) => println!("Vault path: {}", path.display()),
        None => println!("No vault path configured. Use `lsvault vault set <path>` to set one."),
    }
    Ok(())
}

pub fn save(vault: &mut Vault) -> Result<()> {
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
            pb.set_message("Saving photos to vault...");
        }
        VaultSaveProgress::Copied { target, .. } => {
            pb.inc(1);
            pb.set_message(format!("-> {}", target.display()));
        }
        VaultSaveProgress::Skipped { .. } => {
            pb.inc(1);
        }
        VaultSaveProgress::Complete { copied, skipped } => {
            pb.finish_with_message(format!("{copied} copied, {skipped} skipped"));
        }
    }))?;

    println!("Vault save complete.");
    Ok(())
}

pub fn export_set(vault: &Vault, path: PathBuf) -> Result<()> {
    vault.set_export_path(&path)?;
    let resolved = vault.get_export_path()?.unwrap();
    println!("Export path set to: {}", resolved.display());
    Ok(())
}

pub fn export_show(vault: &Vault) -> Result<()> {
    match vault.get_export_path()? {
        Some(path) => println!("Export path: {}", path.display()),
        None => println!(
            "No export path configured. Use `lsvault vault export-set <path>` to set one."
        ),
    }
    Ok(())
}

pub fn export(vault: &Vault, quality: u8) -> Result<()> {
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
