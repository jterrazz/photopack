use anyhow::Result;
use losslessvault_core::Vault;

pub fn run(vault: &Vault, id: Option<i64>) -> Result<()> {
    match id {
        Some(id) => show_group(vault, id),
        None => list_groups(vault),
    }
}

fn list_groups(vault: &Vault) -> Result<()> {
    let groups = vault.groups()?;

    if groups.is_empty() {
        println!("No duplicates found. Run `lsvault sources scan` first.");
        return Ok(());
    }

    println!(
        "{:<6} {:<12} {:<8} {}",
        "ID", "Confidence", "Members", "Source of Truth"
    );
    println!("{}", "-".repeat(80));

    for group in &groups {
        let sot = group
            .members
            .iter()
            .find(|m| m.id == group.source_of_truth_id)
            .map(|m| m.path.display().to_string())
            .unwrap_or_else(|| "?".to_string());

        println!(
            "{:<6} {:<12} {:<8} {}",
            group.id,
            group.confidence,
            group.members.len(),
            sot,
        );
    }

    Ok(())
}

fn show_group(vault: &Vault, id: i64) -> Result<()> {
    let group = vault.group(id)?;

    println!("Group #{} ({})", group.id, group.confidence);
    println!("{}", "-".repeat(60));

    for member in &group.members {
        let marker = if member.id == group.source_of_truth_id {
            " [SOURCE]"
        } else {
            ""
        };
        println!(
            "  {} ({}, {:.1} KB){}",
            member.path.display(),
            member.format,
            member.size as f64 / 1024.0,
            marker,
        );
    }

    Ok(())
}
