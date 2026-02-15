use std::path::PathBuf;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use losslessvault_core::{vault_save::VaultSaveProgress, Vault};

pub fn set(vault: &Vault, path: PathBuf) -> Result<()> {
    vault.set_vault_path(&path)?;
    let resolved = vault.get_vault_path()?.unwrap();
    println!("Vault path set to: {}", resolved.display());
    println!("Vault registered as scan source.");
    Ok(())
}

pub fn sync(vault: &mut Vault) -> Result<()> {
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
