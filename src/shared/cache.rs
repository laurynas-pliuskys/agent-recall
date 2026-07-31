use super::indexer::SearchIndexer;
use super::models::MessageType;
use super::source::Source;
use super::utils::file_mtime;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CacheMetadata {
    pub indexed_files: HashMap<PathBuf, FileMetadata>,
    pub last_full_scan: Option<DateTime<Utc>>,
    pub index_version: u32,
    pub total_entries: u64,
    /// Cached message counts per session (user + assistant messages only)
    #[serde(default)]
    pub session_counts: HashMap<String, usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileMetadata {
    #[serde(alias = "hash")]
    pub size_hex: String,
    pub size: u64,
    pub modified: DateTime<Utc>,
    pub indexed_at: DateTime<Utc>,
    pub entry_count: usize,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub parser_version: u32,
    #[serde(default)]
    pub conversation_counts: HashMap<String, usize>,
}

fn parser_version(source: Source) -> u32 {
    super::source::conversation_source(source).parser_version()
}

pub struct CacheManager {
    cache_dir: PathBuf,
    metadata_file: PathBuf,
    metadata: CacheMetadata,
}

impl CacheManager {
    pub fn new(cache_dir: &Path) -> Result<Self> {
        let metadata_file = cache_dir.join("cache-metadata.json");

        let metadata = if metadata_file.exists() {
            let content = fs::read_to_string(&metadata_file)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            CacheMetadata::default()
        };

        Ok(Self {
            cache_dir: cache_dir.to_path_buf(),
            metadata_file,
            metadata,
        })
    }

    pub fn needs_indexing(&self, file_path: &Path) -> Result<bool> {
        let file_size = fs::metadata(file_path)?.len();
        let file_modified = file_mtime(file_path)?;
        let source = super::path_utils::source_for_jsonl_path(file_path).ok_or_else(|| {
            anyhow::anyhow!("No conversation source owns {}", file_path.display())
        })?;

        match self
            .metadata
            .indexed_files
            .get(file_path)
        {
            Some(cached) => {
                // Check if file has changed using mtime and size
                Ok(cached.size != file_size
                    || cached.modified != file_modified
                    || cached.source != source
                    || cached.parser_version != parser_version(source))
            }
            None => Ok(true), // File not indexed yet
        }
    }

    pub fn update_incremental(
        &mut self,
        indexer: &mut SearchIndexer,
        files: Vec<PathBuf>,
    ) -> Result<()> {
        // Phase 1 (serial): remove deleted files and collect files that need parsing.
        let mut to_parse: Vec<PathBuf> = Vec::new();
        for file_path in files {
            if !file_path.exists() {
                if self
                    .metadata
                    .indexed_files
                    .remove(&file_path)
                    .is_some()
                {
                    debug!("Removed deleted file from cache: {}", file_path.display());
                }
                continue;
            }
            if !self.needs_indexing(&file_path)? {
                debug!("Skipping unchanged file: {}", file_path.display());
                continue;
            }
            to_parse.push(file_path);
        }

        if to_parse.is_empty() {
            info!("No files needed indexing");
            return Ok(());
        }

        // Phase 2 (parallel): parse all files concurrently.
        struct ParsedFile {
            path: PathBuf,
            source: Source,
            file_size: u64,
            file_modified: DateTime<Utc>,
            entries: Vec<super::models::ConversationEntry>,
        }

        let parsed: Vec<_> = to_parse
            .into_par_iter()
            .filter_map(|file_path| {
                info!("Processing: {}", file_path.display());
                let file_size = fs::metadata(&file_path)
                    .ok()?
                    .len();
                let file_modified = file_mtime(&file_path).ok()?;
                let source = match super::path_utils::source_for_jsonl_path(&file_path) {
                    Some(source) => source,
                    None => {
                        warn!("No source adapter owns {}", file_path.display());
                        return None;
                    }
                };
                let entries_res =
                    super::source::conversation_source(source).parse(&file_path, false);
                match entries_res {
                    Ok(entries) if entries.is_empty() => {
                        warn!(
                            "{} parsed successfully but yielded no recognized messages; leaving it uncached",
                            file_path.display()
                        );
                        None
                    }
                    Ok(entries) => Some(ParsedFile {
                        path: file_path,
                        source,
                        file_size,
                        file_modified,
                        entries,
                    }),
                    Err(e) => {
                        warn!("Failed to parse {}: {}", file_path.display(), e);
                        None
                    }
                }
            })
            .collect();

        // Phase 3 (serial): feed into IndexWriter and update cache metadata.
        let mut files_processed = 0;
        for parsed_file in parsed {
            let entry_count = parsed_file
                .entries
                .len();
            indexer.delete_artifact(parsed_file.source, &parsed_file.path)?;

            let mut conversation_counts = HashMap::new();
            for entry in &parsed_file.entries {
                if matches!(
                    entry.message_type,
                    MessageType::User | MessageType::Assistant
                ) && !entry.is_tool_record()
                {
                    *conversation_counts
                        .entry(
                            entry
                                .source
                                .conversation_key(&entry.session_id),
                        )
                        .or_insert(0) += 1;
                }
            }

            indexer.index_conversations(parsed_file.entries)?;
            info!("  Indexed {} entries", entry_count);

            self.metadata
                .indexed_files
                .insert(
                    parsed_file.path,
                    FileMetadata {
                        size_hex: format!("{:x}", parsed_file.file_size),
                        size: parsed_file.file_size,
                        modified: parsed_file.file_modified,
                        indexed_at: Utc::now(),
                        entry_count,
                        source: parsed_file.source,
                        parser_version: parser_version(parsed_file.source),
                        conversation_counts,
                    },
                );
            files_processed += 1;
        }

        if files_processed > 0 {
            indexer.commit()?;
        }

        self.refresh_derived_metadata();
        self.metadata
            .last_full_scan = Some(Utc::now());
        self.save_metadata()?;

        if files_processed > 0 {
            info!(
                "Incremental indexing complete: {} files processed, {} entries indexed",
                files_processed,
                self.metadata
                    .total_entries
            );
        } else {
            info!("No files needed indexing");
        }

        Ok(())
    }

    /// Remove indexed artifacts absent from a complete discovery result. This
    /// must not be used for targeted single-artifact refreshes.
    pub fn remove_missing_files(
        &mut self,
        indexer: &mut SearchIndexer,
        discovered_files: &[PathBuf],
    ) -> Result<usize> {
        let discovered: HashSet<_> = discovered_files
            .iter()
            .collect();
        let missing: Vec<_> = self
            .metadata
            .indexed_files
            .iter()
            .filter(|(path, _)| !path.exists() || !discovered.contains(path))
            .map(|(path, metadata)| (path.clone(), metadata.source))
            .collect();

        for (path, source) in &missing {
            indexer.delete_artifact(*source, path)?;
            self.metadata
                .indexed_files
                .remove(path);
            debug!("Removed missing source artifact: {}", path.display());
        }

        if !missing.is_empty() {
            indexer.commit()?;
            self.refresh_derived_metadata();
            self.save_metadata()?;
        }
        Ok(missing.len())
    }

    fn refresh_derived_metadata(&mut self) {
        self.metadata
            .total_entries = self
            .metadata
            .indexed_files
            .values()
            .map(|file| file.entry_count as u64)
            .sum();
        self.metadata
            .session_counts
            .clear();
        for file in self
            .metadata
            .indexed_files
            .values()
        {
            for (conversation_key, count) in &file.conversation_counts {
                *self
                    .metadata
                    .session_counts
                    .entry(conversation_key.clone())
                    .or_insert(0) += count;
            }
        }
    }

    pub fn clear_cache(&mut self) -> Result<()> {
        if self
            .cache_dir
            .exists()
        {
            fs::remove_dir_all(&self.cache_dir)?;
        }
        fs::create_dir_all(&self.cache_dir)?;

        self.metadata = CacheMetadata::default();
        self.save_metadata()?;

        info!("Cache cleared successfully");
        Ok(())
    }

    pub fn get_basic_stats(&self) -> (usize, u64, Option<DateTime<Utc>>) {
        (
            self.metadata
                .indexed_files
                .len(),
            self.metadata
                .total_entries,
            self.metadata
                .last_full_scan,
        )
    }

    /// Get cached session interaction counts
    pub fn get_session_counts(&self) -> &HashMap<String, usize> {
        &self
            .metadata
            .session_counts
    }

    pub fn get_stats(&self) -> CacheStats {
        CacheStats {
            total_files: self
                .metadata
                .indexed_files
                .len(),
            total_entries: self
                .metadata
                .total_entries,
            last_updated: self
                .metadata
                .last_full_scan,
            cache_size_mb: self.calculate_cache_size_mb(),
            projects: self.get_project_stats(),
        }
    }

    fn save_metadata(&self) -> Result<()> {
        fs::create_dir_all(&self.cache_dir)?;
        let content = serde_json::to_string_pretty(&self.metadata)?;
        fs::write(&self.metadata_file, content)?;
        Ok(())
    }

    fn calculate_cache_size_mb(&self) -> f64 {
        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            let total_bytes: u64 = entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| fs::metadata(entry.path()).ok())
                .map(|metadata| metadata.len())
                .sum();
            total_bytes as f64 / (1024.0 * 1024.0)
        } else {
            0.0
        }
    }

    fn get_project_stats(&self) -> Vec<ProjectStats> {
        let mut projects: HashMap<String, ProjectStats> = HashMap::new();

        for (file_path, file_meta) in &self
            .metadata
            .indexed_files
        {
            if let Some(parent) = file_path.parent()
                && let Some(project_name) = parent
                    .file_name()
                    .and_then(|n| n.to_str())
            {
                let stats = projects
                    .entry(project_name.to_string())
                    .or_insert_with(|| ProjectStats {
                        name: project_name.to_string(),
                        files: 0,
                        entries: 0,
                        last_updated: file_meta.indexed_at,
                    });

                stats.files += 1;
                stats.entries += file_meta.entry_count as u64;
                if file_meta.indexed_at > stats.last_updated {
                    stats.last_updated = file_meta.indexed_at;
                }
            }
        }

        let mut project_list: Vec<ProjectStats> = projects
            .into_values()
            .collect();
        project_list.sort_by(|a, b| {
            b.last_updated
                .cmp(&a.last_updated)
        });
        project_list
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_files: usize,
    pub total_entries: u64,
    pub last_updated: Option<DateTime<Utc>>,
    pub cache_size_mb: f64,
    pub projects: Vec<ProjectStats>,
}

#[derive(Debug, Clone)]
pub struct ProjectStats {
    pub name: String,
    pub files: usize,
    pub entries: u64,
    pub last_updated: DateTime<Utc>,
}

/// Result of checking index health
#[derive(Debug, Clone)]
pub struct IndexHealth {
    pub total_indexed_files: usize,
    pub total_entries: u64,
    pub last_indexed: Option<DateTime<Utc>>,
    pub stale_files: Vec<PathBuf>,
    pub missing_files: Vec<PathBuf>,
    pub new_files: Vec<PathBuf>,
    pub status: IndexHealthStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexHealthStatus {
    Healthy,
    NeedsUpdate,
    NeedsRebuild,
}

impl CacheManager {
    /// Quick health check - just counts stale/new files without full scan
    /// Returns (stale_count, new_count) for passive reporting
    pub fn quick_health_check(&self, all_jsonl_files: &[PathBuf]) -> (usize, usize) {
        let mut stale = 0;
        let mut new_files = 0;
        for (path, meta) in &self
            .metadata
            .indexed_files
        {
            if let Ok(current_mtime) = file_mtime(path) {
                let current_size = fs::metadata(path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                if current_size != meta.size || current_mtime != meta.modified {
                    stale += 1;
                }
            }
        }
        for path in all_jsonl_files {
            if !self
                .metadata
                .indexed_files
                .contains_key(path)
            {
                new_files += 1;
            }
        }
        (stale, new_files)
    }

    /// Check index health by comparing cached metadata with actual files
    pub fn check_index_health(&self, all_jsonl_files: &[PathBuf]) -> Result<IndexHealth> {
        let mut stale_files = Vec::new();
        let mut missing_files = Vec::new();
        let mut new_files = Vec::new();

        // Check for stale and missing files
        for (cached_path, cached_meta) in &self
            .metadata
            .indexed_files
        {
            if !cached_path.exists() {
                missing_files.push(cached_path.clone());
            } else if let Ok(current_mtime) = file_mtime(cached_path) {
                let current_size = fs::metadata(cached_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                if current_size != cached_meta.size || current_mtime != cached_meta.modified {
                    stale_files.push(cached_path.clone());
                }
            }
        }

        // Check for new files not in cache
        for file_path in all_jsonl_files {
            if !self
                .metadata
                .indexed_files
                .contains_key(file_path)
            {
                new_files.push(file_path.clone());
            }
        }

        // Determine overall status
        let status = if missing_files.len()
            > self
                .metadata
                .indexed_files
                .len()
                / 2
        {
            IndexHealthStatus::NeedsRebuild
        } else if !stale_files.is_empty() || !new_files.is_empty() || !missing_files.is_empty() {
            IndexHealthStatus::NeedsUpdate
        } else {
            IndexHealthStatus::Healthy
        };

        Ok(IndexHealth {
            total_indexed_files: self
                .metadata
                .indexed_files
                .len(),
            total_entries: self
                .metadata
                .total_entries,
            last_indexed: self
                .metadata
                .last_full_scan,
            stale_files,
            missing_files,
            new_files,
            status,
        })
    }
}

impl std::fmt::Display for IndexHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Index Health Report")?;
        writeln!(f, "===================")?;
        writeln!(
            f,
            "Total indexed: {} files, {} entries",
            self.total_indexed_files, self.total_entries
        )?;
        if let Some(last) = self.last_indexed {
            writeln!(f, "Last indexed: {}", last.format("%Y-%m-%d %H:%M:%S UTC"))?;
        }
        writeln!(
            f,
            "Stale files: {} (modified since indexed)",
            self.stale_files
                .len()
        )?;
        writeln!(
            f,
            "Missing files: {} (deleted from disk)",
            self.missing_files
                .len()
        )?;
        writeln!(
            f,
            "New files: {} (not yet indexed)",
            self.new_files
                .len()
        )?;
        writeln!(f, "Status: {:?}", self.status)?;
        Ok(())
    }
}
