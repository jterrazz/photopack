use std::collections::HashMap;

use anyhow::Result;
use comfy_table::{presets::UTF8_FULL, Cell, ContentArrangement, Table};
use photopack_core::Vault;

use super::status::{
    add_photo_row, compute_aggregates, sort_photos_for_display, source_display_name, StatusData,
};

pub fn run(vault: &Vault, dupes: bool, id: Option<i64>) -> Result<()> {
    if dupes {
        match id {
            Some(id) => show_group(vault, id),
            None => list_groups(vault),
        }
    } else {
        list_files(vault)
    }
}

fn list_files(vault: &Vault) -> Result<()> {
    let sources = vault.sources()?;
    let photos = vault.photos()?;
    let groups = vault.groups()?;

    let data = StatusData::build(&groups);
    let agg = compute_aggregates(&photos, &groups, &data);

    let source_name_map: HashMap<i64, String> = sources
        .iter()
        .map(|s| (s.id, source_display_name(s)))
        .collect();

    let mut files_table = Table::new();
    files_table.load_preset(UTF8_FULL);
    files_table.set_content_arrangement(ContentArrangement::Dynamic);
    files_table.set_header(vec![
        Cell::new("File"),
        Cell::new("Source"),
        Cell::new("Fmt"),
        Cell::new("Size"),
        Cell::new("Group"),
        Cell::new("Role"),
        Cell::new("Vault"),
    ]);

    let header_len = 7; // File, Source, Fmt, Size, Group, Role, Vault

    // Partition and sort
    let (grouped_photos, ungrouped_photos) = sort_photos_for_display(&photos, &data);

    // Add grouped photo rows
    let mut last_group_id: Option<i64> = None;

    for photo in &grouped_photos {
        let gid = *data.photo_group.get(&photo.id).unwrap();

        if last_group_id.is_some() && last_group_id != Some(gid) {
            let empty_row: Vec<Cell> = (0..header_len).map(|_| Cell::new("")).collect();
            files_table.add_row(empty_row);
        }
        last_group_id = Some(gid);

        add_photo_row(&mut files_table, photo, &source_name_map, &data);
    }

    // Separator between grouped and ungrouped
    if !grouped_photos.is_empty() && !ungrouped_photos.is_empty() {
        let empty_row: Vec<Cell> = (0..header_len).map(|_| Cell::new("")).collect();
        files_table.add_row(empty_row);
    }

    for photo in &ungrouped_photos {
        add_photo_row(&mut files_table, photo, &source_name_map, &data);
    }

    println!();
    println!("  Files");
    println!("  -----");
    println!("{files_table}");
    println!();
    println!(
        "  {} files ({} groups, {} duplicates)",
        agg.total_photos, agg.total_groups, agg.total_duplicates
    );
    println!();

    Ok(())
}

fn list_groups(vault: &Vault) -> Result<()> {
    let groups = vault.groups()?;

    if groups.is_empty() {
        println!("No duplicates found. Run `photopack scan` first.");
        return Ok(());
    }

    println!(
        "{:<6} {:<12} {:<8} Source of Truth",
        "ID", "Confidence", "Members"
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
