pub mod catalog;
pub mod domain;
pub mod error;
pub mod exif;
pub mod export;
pub mod hasher;
pub mod matching;
pub mod ranking;
pub mod scanner;
pub mod vault_save;

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use catalog::Catalog;
use domain::*;
use error::{Error, Result};

/// Callback for reporting scan progress.
pub enum ScanProgress {
    /// Starting scan of a source directory.
    SourceStart { source: String, file_count: usize },
    /// A file has been processed.
    FileProcessed { path: PathBuf },
    /// Scan phase completed.
    PhaseComplete { phase: String },
}

/// The main entry point for the LosslessVault library.
pub struct Vault {
    catalog: Catalog,
}

impl Vault {
    /// Open or create a vault at the given catalog path.
    pub fn open(catalog_path: &Path) -> Result<Self> {
        let catalog = Catalog::open(catalog_path)?;
        Ok(Self { catalog })
    }

    /// Register a new source directory.
    pub fn add_source(&self, path: &Path) -> Result<Source> {
        if !path.exists() {
            return Err(Error::SourceNotFound(path.to_path_buf()));
        }
        if !path.is_dir() {
            return Err(Error::SourceNotDirectory(path.to_path_buf()));
        }
        self.catalog.add_source(path)
    }

    /// Remove a source and all its photos from the catalog.
    pub fn remove_source(&self, path: &Path) -> Result<(Source, usize)> {
        self.catalog.remove_source(path)
    }

    /// Scan all registered sources, hash files, find duplicates, and rank them.
    /// Calls `progress_cb` with progress updates if provided.
    pub fn scan(&mut self, mut progress_cb: Option<&mut dyn FnMut(ScanProgress)>) -> Result<()> {
        let sources = self.catalog.list_sources()?;
        let now = chrono::Utc::now().timestamp();

        for source in &sources {
            // Discover files
            let scanned_files = scanner::scan_directory(&source.path)?;

            if let Some(ref mut cb) = progress_cb {
                cb(ScanProgress::SourceStart {
                    source: source.path.to_string_lossy().to_string(),
                    file_count: scanned_files.len(),
                });
            }

            // Filter out files whose mtime hasn't changed (incremental scan)
            let files_to_process: Vec<&ScannedFile> = scanned_files
                .iter()
                .filter(|sf| {
                    !matches!(self.catalog.get_photo_mtime(&sf.path), Ok(Some(existing_mtime)) if existing_mtime == sf.mtime)
                })
                .collect();

            // Process files in parallel: hash + EXIF (no DB access here)
            let processed: Vec<PhotoFile> = files_to_process
                .par_iter()
                .filter_map(|sf| {
                    let sha256 = match hasher::compute_sha256(&sf.path) {
                        Ok(h) => h,
                        Err(_) => return None,
                    };

                    // Only attempt perceptual hashing for formats the image crate can decode
                    let (phash, dhash) = if sf.format.supports_perceptual_hash() {
                        match hasher::perceptual::compute_perceptual_hashes(&sf.path) {
                            Some((p, d)) => (Some(p), Some(d)),
                            None => (None, None),
                        }
                    } else {
                        (None, None)
                    };

                    let exif_data = exif::extract_exif(&sf.path);

                    Some(PhotoFile {
                        id: 0,
                        source_id: source.id,
                        path: sf.path.clone(),
                        size: sf.size,
                        format: sf.format,
                        sha256,
                        phash,
                        dhash,
                        exif: exif_data,
                        mtime: sf.mtime,
                    })
                })
                .collect();

            // Batch insert into catalog (single transaction)
            self.catalog.upsert_photos_batch(&processed)?;

            for photo in &processed {
                if let Some(ref mut cb) = progress_cb {
                    cb(ScanProgress::FileProcessed {
                        path: photo.path.clone(),
                    });
                }
            }

            self.catalog.update_source_scanned(source.id, now)?;
        }

        if let Some(ref mut cb) = progress_cb {
            cb(ScanProgress::PhaseComplete {
                phase: "indexing".to_string(),
            });
        }

        // Matching phase
        let all_photos = self.catalog.list_all_photos()?;
        let match_groups = matching::find_duplicates(&all_photos);

        // Build a lookup map for ranking
        let photo_map: std::collections::HashMap<i64, &PhotoFile> =
            all_photos.iter().map(|p| (p.id, p)).collect();

        // Prepare groups for batch insert
        let mut group_tuples: Vec<(i64, matching::MatchGroup)> = Vec::new();
        for group in &match_groups {
            let members: Vec<&PhotoFile> = group
                .member_ids
                .iter()
                .filter_map(|id| photo_map.get(id).copied())
                .collect();

            if members.len() < 2 {
                continue;
            }

            let sot = ranking::elect_source_of_truth(&members);
            group_tuples.push((sot.id, group.clone()));
        }

        let batch: Vec<(i64, domain::Confidence, Vec<i64>)> = group_tuples
            .into_iter()
            .map(|(sot_id, g)| (sot_id, g.confidence, g.member_ids))
            .collect();
        self.catalog.replace_groups_batch(&batch)?;

        if let Some(ref mut cb) = progress_cb {
            cb(ScanProgress::PhaseComplete {
                phase: "matching".to_string(),
            });
        }

        Ok(())
    }

    /// List all registered sources.
    pub fn sources(&self) -> Result<Vec<Source>> {
        self.catalog.list_sources()
    }

    /// List all photos in the catalog.
    pub fn photos(&self) -> Result<Vec<PhotoFile>> {
        self.catalog.list_all_photos()
    }

    /// Get catalog summary statistics (single query for photos/groups/duplicates).
    pub fn status(&self) -> Result<CatalogStats> {
        let (total_photos, total_groups, total_duplicates) = self.catalog.stats_summary()?;
        Ok(CatalogStats {
            total_sources: self.catalog.list_sources()?.len(),
            total_photos,
            total_groups,
            total_duplicates,
        })
    }

    /// List all duplicate groups.
    pub fn groups(&self) -> Result<Vec<DuplicateGroup>> {
        self.catalog.list_groups()
    }

    /// Get details of a specific duplicate group.
    pub fn group(&self, id: i64) -> Result<DuplicateGroup> {
        self.catalog.get_group(id)
    }

    /// Set the vault export destination path.
    pub fn set_vault_path(&self, path: &Path) -> Result<()> {
        let canonical = path
            .canonicalize()
            .map_err(|_| Error::VaultPathNotFound(path.to_path_buf()))?;
        if !canonical.is_dir() {
            return Err(Error::VaultPathNotFound(path.to_path_buf()));
        }
        self.catalog
            .set_config("vault_path", &canonical.to_string_lossy())
    }

    /// Get the current vault export destination path, if set.
    pub fn get_vault_path(&self) -> Result<Option<PathBuf>> {
        Ok(self.catalog.get_config("vault_path")?.map(PathBuf::from))
    }

    /// Copy deduplicated photos to the vault directory.
    /// For each duplicate group, only the source-of-truth is copied.
    /// Ungrouped photos are copied as-is.
    /// Photos are organized into YYYY/MM/DD folders based on EXIF date (mtime fallback).
    pub fn vault_save(
        &mut self,
        mut progress_cb: Option<&mut dyn FnMut(vault_save::VaultSaveProgress)>,
    ) -> Result<()> {
        let vault_path = self
            .catalog
            .get_config("vault_path")?
            .map(PathBuf::from)
            .ok_or(Error::VaultPathNotSet)?;

        if !vault_path.is_dir() {
            return Err(Error::VaultPathNotFound(vault_path));
        }

        let all_photos = self.catalog.list_all_photos()?;
        let groups = self.catalog.list_groups()?;
        let to_save = vault_save::select_photos_to_export(&all_photos, &groups);

        if let Some(ref mut cb) = progress_cb {
            cb(vault_save::VaultSaveProgress::Start {
                total: to_save.len(),
            });
        }

        // Pre-compute targets sequentially (needs filesystem checks for collisions)
        let targets: Vec<(&PhotoFile, PathBuf)> = to_save
            .iter()
            .map(|photo| {
                let date = vault_save::date_for_photo(photo);
                let target =
                    vault_save::build_target_path(&vault_path, date, &photo.path, photo.size);
                (*photo, target)
            })
            .collect();

        // Parallel file copy, collect results
        let results: Vec<(bool, PathBuf, PathBuf)> = targets
            .par_iter()
            .filter_map(|(photo, target)| {
                match vault_save::copy_photo_to_vault(&photo.path, target, photo.size) {
                    Ok(did_copy) => Some((did_copy, photo.path.clone(), target.clone())),
                    Err(_) => None,
                }
            })
            .collect();

        // Report progress sequentially (callback is not Send)
        let mut copied = 0usize;
        let mut skipped = 0usize;
        for (did_copy, source, target) in &results {
            if *did_copy {
                copied += 1;
                if let Some(ref mut cb) = progress_cb {
                    cb(vault_save::VaultSaveProgress::Copied {
                        source: source.clone(),
                        target: target.clone(),
                    });
                }
            } else {
                skipped += 1;
                if let Some(ref mut cb) = progress_cb {
                    cb(vault_save::VaultSaveProgress::Skipped {
                        path: source.clone(),
                    });
                }
            }
        }

        if let Some(ref mut cb) = progress_cb {
            cb(vault_save::VaultSaveProgress::Complete { copied, skipped });
        }

        Ok(())
    }

    /// Set the export destination path.
    pub fn set_export_path(&self, path: &Path) -> Result<()> {
        let canonical = path
            .canonicalize()
            .map_err(|_| Error::ExportPathNotFound(path.to_path_buf()))?;
        if !canonical.is_dir() {
            return Err(Error::ExportPathNotFound(path.to_path_buf()));
        }
        self.catalog
            .set_config("export_path", &canonical.to_string_lossy())
    }

    /// Get the current export destination path, if set.
    pub fn get_export_path(&self) -> Result<Option<PathBuf>> {
        Ok(self.catalog.get_config("export_path")?.map(PathBuf::from))
    }

    /// Export deduplicated photos as HEIC files.
    /// For each duplicate group, only the source-of-truth is exported.
    /// Ungrouped photos are exported as-is.
    /// Photos are organized into YYYY/MM/DD folders and converted to HEIC.
    pub fn export(
        &self,
        quality: u8,
        mut progress_cb: Option<&mut dyn FnMut(export::ExportProgress)>,
    ) -> Result<()> {
        export::check_sips_available()?;

        let export_path = self
            .catalog
            .get_config("export_path")?
            .map(PathBuf::from)
            .ok_or(Error::ExportPathNotSet)?;

        if !export_path.is_dir() {
            return Err(Error::ExportPathNotFound(export_path));
        }

        let all_photos = self.catalog.list_all_photos()?;
        let groups = self.catalog.list_groups()?;
        let to_export = vault_save::select_photos_to_export(&all_photos, &groups);

        if let Some(ref mut cb) = progress_cb {
            cb(export::ExportProgress::Start {
                total: to_export.len(),
            });
        }

        // Pre-compute targets sequentially (needs filesystem checks)
        let targets: Vec<(&PhotoFile, PathBuf)> = to_export
            .iter()
            .map(|photo| {
                let date = vault_save::date_for_photo(photo);
                let target = export::build_export_path(&export_path, date, &photo.path);
                (*photo, target)
            })
            .collect();

        // Parallel HEIC conversion, collect results
        let results: Vec<(bool, PathBuf, PathBuf)> = targets
            .par_iter()
            .filter_map(|(photo, target)| {
                match export::export_photo_to_heic(&photo.path, target, quality) {
                    Ok(did_convert) => Some((did_convert, photo.path.clone(), target.clone())),
                    Err(_) => None,
                }
            })
            .collect();

        // Report progress sequentially (callback is not Send)
        let mut converted = 0usize;
        let mut skipped = 0usize;
        for (did_convert, source, target) in &results {
            if *did_convert {
                converted += 1;
                if let Some(ref mut cb) = progress_cb {
                    cb(export::ExportProgress::Converted {
                        source: source.clone(),
                        target: target.clone(),
                    });
                }
            } else {
                skipped += 1;
                if let Some(ref mut cb) = progress_cb {
                    cb(export::ExportProgress::Skipped {
                        path: source.clone(),
                    });
                }
            }
        }

        if let Some(ref mut cb) = progress_cb {
            cb(export::ExportProgress::Complete {
                converted,
                skipped,
            });
        }

        Ok(())
    }
}
