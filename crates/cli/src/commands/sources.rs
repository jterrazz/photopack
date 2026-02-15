use std::path::PathBuf;

use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use photopack_core::{ScanProgress, Vault};

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

fn active_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "  {bar:30.cyan/blue} {spinner:.green} {pos:>5}/{len:<5} {prefix:.dim} {msg}",
    )
    .unwrap()
    .progress_chars("━╸─")
}

fn done_style() -> ProgressStyle {
    ProgressStyle::with_template("  {bar:30.green} {prefix:.green} {msg:.dim}").unwrap()
}

fn source_display_name(source: &str) -> &str {
    source
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(source)
}

pub fn scan(vault: &mut Vault) -> Result<()> {
    let mp = MultiProgress::new();
    let mut active_pb: Option<ProgressBar> = None;
    let mut current_len: u64 = 0;

    vault.scan(Some(&mut |progress| match progress {
        ScanProgress::SourceStart {
            source,
            file_count,
        } => {
            // Finish any leftover active bar
            if let Some(pb) = active_pb.take() {
                pb.finish_and_clear();
                mp.remove(&pb);
            }

            mp.println(String::new()).ok();
            mp.println(format!(
                "  Scanning {} ({} files)",
                source_display_name(&source),
                file_count
            ))
            .ok();

            current_len = file_count as u64;
            let pb = mp.add(ProgressBar::new(current_len));
            pb.set_style(active_style());
            pb.set_prefix("Hashing");
            pb.set_message(String::new());
            pb.enable_steady_tick(std::time::Duration::from_millis(80));
            active_pb = Some(pb);
        }
        ScanProgress::FileHashed { path } => {
            if let Some(ref pb) = active_pb {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                pb.set_message(name);
                pb.inc(1);
            }
        }
        ScanProgress::FilesRemoved { count } => {
            mp.println(format!("  Cleaned {count} stale entries")).ok();
        }
        ScanProgress::AnalysisStart { count } => {
            // Finish hashing bar — stays visible with done style
            if let Some(pb) = active_pb.take() {
                pb.set_style(done_style());
                pb.set_prefix("done");
                pb.finish_with_message(format!("Hashed {} files", current_len));
            }

            // New bar for analysis phase
            current_len = count as u64;
            let pb = mp.add(ProgressBar::new(current_len));
            pb.set_style(active_style());
            pb.set_prefix("Analyzing");
            pb.set_message(String::new());
            pb.enable_steady_tick(std::time::Duration::from_millis(80));
            active_pb = Some(pb);
        }
        ScanProgress::AnalysisDone { path } => {
            if let Some(ref pb) = active_pb {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                pb.set_message(name);
                pb.inc(1);
            }
        }
        ScanProgress::PhaseComplete { phase } => {
            if let Some(pb) = active_pb.take() {
                if phase == "indexing" {
                    pb.set_style(done_style());
                    pb.set_prefix("done");
                    pb.finish_with_message(format!("Indexed {} files", current_len));
                } else {
                    pb.finish_and_clear();
                    mp.remove(&pb);
                }
            }
        }
    }))?;

    mp.println(String::new()).ok();
    mp.println("  Scan complete.").ok();
    mp.println(String::new()).ok();
    Ok(())
}
