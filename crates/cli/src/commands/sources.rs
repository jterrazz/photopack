use std::path::PathBuf;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use losslessvault_core::{ScanProgress, Vault};

pub fn list(vault: &Vault) -> Result<()> {
    let sources = vault.sources()?;

    if sources.is_empty() {
        println!("No sources registered. Run `lsvault sources add <path>` to add one.");
        return Ok(());
    }

    println!("{:<4} {:<60} Last Scanned", "ID", "Path");
    println!("{}", "-".repeat(80));

    for source in &sources {
        let scanned = match source.last_scanned {
            Some(ts) => chrono::DateTime::from_timestamp(ts, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            None => "never".to_string(),
        };
        println!("{:<4} {:<60} {}", source.id, source.path.display(), scanned);
    }

    Ok(())
}

pub fn add(vault: &Vault, path: PathBuf) -> Result<()> {
    let source = vault.add_source(&path)?;
    println!("Added source: {}", source.path.display());
    Ok(())
}

pub fn rm(vault: &Vault, path: PathBuf) -> Result<()> {
    let (source, photo_count) = vault.remove_source(&path)?;
    println!(
        "Removed source: {} ({} photos removed from catalog)",
        source.path.display(),
        photo_count
    );
    Ok(())
}

pub fn scan(vault: &mut Vault) -> Result<()> {
    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    vault.scan(Some(&mut |progress| match progress {
        ScanProgress::SourceStart {
            source,
            file_count,
        } => {
            pb.set_length(file_count as u64);
            pb.set_position(0);
            pb.set_message(format!("Scanning {source}"));
        }
        ScanProgress::FileProcessed { .. } => {
            pb.inc(1);
        }
        ScanProgress::PhaseComplete { phase } => {
            pb.finish_with_message(format!("{phase} complete"));
        }
    }))?;

    println!("Scan complete.");
    Ok(())
}
