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
use std::io::Write;
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
    /// Primary-index records per source-qualified conversation. This lets a
    /// moved source artifact supersede only its matching conversations.
    #[serde(default)]
    pub conversation_entry_counts: HashMap<String, usize>,
    /// Stable source-qualified record IDs used to avoid counting overlapping
    /// rollout fragments twice. Older metadata falls back to stored counts.
    #[serde(default)]
    pub primary_record_ids: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub message_ids: HashMap<String, Vec<String>>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IndexingOutcome {
    pub candidates: usize,
    pub indexed_artifacts: usize,
    pub indexed_primary_records: usize,
    pub reference_only_artifacts: usize,
    pub empty_artifacts: usize,
    pub parse_failures: usize,
    pub unchanged_artifacts: usize,
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
        Self::load(cache_dir, false)
    }

    /// Open cache metadata for an explicitly destructive reset.
    ///
    /// Ordinary reads must preserve the strict failure mode so corrupt
    /// retention metadata can never be discarded implicitly. `cache clear`
    /// and history-loss-acknowledged rebuilds may use this escape hatch
    /// because the caller has already requested that retained history be
    /// destroyed.
    pub fn new_for_destructive_reset(cache_dir: &Path) -> Result<Self> {
        Self::load(cache_dir, true)
    }

    fn load(cache_dir: &Path, discard_invalid_metadata: bool) -> Result<Self> {
        let metadata_file = cache_dir.join("cache-metadata.json");

        let metadata = if metadata_file.exists() {
            let content = fs::read_to_string(&metadata_file)?;
            match serde_json::from_str(&content) {
                Ok(metadata) => metadata,
                Err(error) if discard_invalid_metadata => {
                    warn!(
                        "Discarding invalid cache metadata {} during an explicitly destructive reset: {}",
                        metadata_file.display(),
                        error
                    );
                    CacheMetadata::default()
                }
                Err(error) => {
                    anyhow::bail!(
                        "Could not parse cache metadata {}: {error}. Refusing to discard retention information; run `agent-recall cache clear` or `agent-recall index rebuild --allow-history-loss` to reset it deliberately.",
                        metadata_file.display()
                    );
                }
            }
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
    ) -> Result<IndexingOutcome> {
        self.update_incremental_with_mode(indexer, files, false)
    }

    /// Reparse supplied artifacts even when their size and coarse mtime match.
    /// Explicit import uses this to make same-second, same-size replacement
    /// deterministic.
    pub fn update_incremental_forced(
        &mut self,
        indexer: &mut SearchIndexer,
        files: Vec<PathBuf>,
    ) -> Result<IndexingOutcome> {
        self.update_incremental_with_mode(indexer, files, true)
    }

    fn update_incremental_with_mode(
        &mut self,
        indexer: &mut SearchIndexer,
        files: Vec<PathBuf>,
        force: bool,
    ) -> Result<IndexingOutcome> {
        let mut outcome = IndexingOutcome::default();
        // Phase 1 (serial): remove deleted files and collect files that need parsing.
        let mut to_parse: Vec<PathBuf> = Vec::new();
        for file_path in files {
            if !file_path.exists() {
                // A client may remove an old transcript between discovery and
                // parsing. Keep its committed document and metadata: automatic
                // indexing is additive and must not delete retained history.
                debug!(
                    "Skipping unavailable source artifact while retaining indexed history: {}",
                    file_path.display()
                );
                continue;
            }
            if !force && !self.needs_indexing(&file_path)? {
                debug!("Skipping unchanged file: {}", file_path.display());
                outcome.unchanged_artifacts += 1;
                continue;
            }
            to_parse.push(file_path);
        }

        outcome.candidates = to_parse.len();

        if to_parse.is_empty() {
            info!("No files needed indexing");
            return Ok(outcome);
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
            .map(|file_path| {
                info!("Processing: {}", file_path.display());
                let file_size = match fs::metadata(&file_path) {
                    Ok(metadata) => metadata.len(),
                    Err(error) => {
                        warn!("Cannot stat {}: {}", file_path.display(), error);
                        return None;
                    }
                };
                let file_modified = match file_mtime(&file_path) {
                    Ok(modified) => modified,
                    Err(error) => {
                        warn!("Cannot read mtime of {}: {}", file_path.display(), error);
                        return None;
                    }
                };
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
        outcome.parse_failures = parsed
            .iter()
            .filter(|file| file.is_none())
            .count();
        for parsed_file in parsed
            .into_iter()
            .flatten()
        {
            let source_adapter = super::source::conversation_source(parsed_file.source);
            // Apply source-owned storage policy at the last boundary before
            // Tantivy. Codex reference payloads must remain only in the source
            // rollout; indexing and reference retrieval intentionally share the
            // same parser so record identity and sequencing cannot drift.
            let reference_count = parsed_file
                .entries
                .iter()
                .filter(|entry| source_adapter.is_reference_record(entry))
                .count();
            let entries: Vec<_> = parsed_file
                .entries
                .into_iter()
                .filter(|entry| source_adapter.is_primary_index_record(entry))
                .collect();
            let entry_count = entries.len();
            let mut conversation_entry_counts = HashMap::new();
            let mut primary_record_ids: HashMap<String, Vec<String>> = HashMap::new();
            for entry in &entries {
                let key = entry
                    .source
                    .conversation_key(&entry.session_id);
                *conversation_entry_counts
                    .entry(key.clone())
                    .or_insert(0) += 1;
                primary_record_ids
                    .entry(key)
                    .or_default()
                    .push(
                        entry
                            .uuid
                            .clone(),
                    );
            }
            self.reconcile_moved_conversations(
                indexer,
                parsed_file.source,
                &parsed_file.path,
                &conversation_entry_counts,
            )?;
            indexer.delete_artifact(parsed_file.source, &parsed_file.path)?;

            let mut conversation_counts = HashMap::new();
            let mut message_ids: HashMap<String, Vec<String>> = HashMap::new();
            for entry in &entries {
                if matches!(
                    entry.message_type,
                    MessageType::User | MessageType::Assistant
                ) && !entry.is_tool_record()
                {
                    let key = entry
                        .source
                        .conversation_key(&entry.session_id);
                    *conversation_counts
                        .entry(key.clone())
                        .or_insert(0) += 1;
                    message_ids
                        .entry(key)
                        .or_default()
                        .push(
                            entry
                                .uuid
                                .clone(),
                        );
                }
            }

            indexer.index_conversations(entries)?;
            info!(
                "  Indexed {} entries; kept {} references source-backed",
                entry_count, reference_count
            );

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
                        conversation_entry_counts,
                        primary_record_ids,
                        message_ids,
                    },
                );
            outcome.indexed_artifacts += 1;
            outcome.indexed_primary_records += entry_count;
            if entry_count == 0 {
                if reference_count == 0 {
                    outcome.empty_artifacts += 1;
                } else {
                    outcome.reference_only_artifacts += 1;
                }
            }
        }

        if outcome.indexed_artifacts > 0 {
            indexer.commit()?;
        }

        self.refresh_derived_metadata();
        self.metadata
            .last_full_scan = Some(Utc::now());
        self.save_metadata()?;

        if outcome.indexed_artifacts > 0 {
            info!(
                "Incremental indexing complete: {} files processed, {} entries indexed",
                outcome.indexed_artifacts,
                self.metadata
                    .total_entries
            );
        } else {
            info!("No files needed indexing");
        }

        Ok(outcome)
    }

    fn reconcile_moved_conversations(
        &mut self,
        indexer: &mut SearchIndexer,
        source: Source,
        replacement_path: &Path,
        incoming: &HashMap<String, usize>,
    ) -> Result<()> {
        // A session can have several live artifacts (Claude subagents and
        // resumed Codex rollouts). Only a missing source path can be replaced.
        // Require all of its conversations in the replacement so a partial
        // import cannot discard unrelated retained history.
        let superseded: Vec<_> = self
            .metadata
            .indexed_files
            .iter()
            .filter(|(path, metadata)| {
                *path != replacement_path
                    && !path.exists()
                    && metadata.source == source
                    && !incoming.is_empty()
                    && {
                        let previous: HashSet<_> = metadata
                            .conversation_entry_counts
                            .keys()
                            .chain(
                                metadata
                                    .conversation_counts
                                    .keys(),
                            )
                            .collect();
                        !previous.is_empty()
                            && previous
                                .iter()
                                .all(|conversation| incoming.contains_key(*conversation))
                    }
            })
            .map(|(path, _)| path.clone())
            .collect();

        for path in superseded {
            indexer.delete_artifact(source, &path)?;
            self.metadata
                .indexed_files
                .remove(&path);
        }
        Ok(())
    }

    fn refresh_derived_metadata(&mut self) {
        let mut primary_ids: HashMap<String, HashSet<&str>> = HashMap::new();
        let mut message_ids: HashMap<String, HashSet<&str>> = HashMap::new();
        for file in self
            .metadata
            .indexed_files
            .values()
        {
            for (conversation, ids) in &file.primary_record_ids {
                primary_ids
                    .entry(conversation.clone())
                    .or_default()
                    .extend(
                        ids.iter()
                            .map(String::as_str),
                    );
            }
            for (conversation, ids) in &file.message_ids {
                message_ids
                    .entry(conversation.clone())
                    .or_default()
                    .extend(
                        ids.iter()
                            .map(String::as_str),
                    );
            }
        }
        self.metadata
            .total_entries = primary_ids
            .values()
            .map(|ids| ids.len() as u64)
            .sum();
        self.metadata
            .session_counts
            .clear();
        for file in self
            .metadata
            .indexed_files
            .values()
        {
            if file
                .primary_record_ids
                .is_empty()
            {
                self.metadata
                    .total_entries += file.entry_count as u64;
            }
            for (conversation_key, count) in &file.conversation_counts {
                if !file
                    .message_ids
                    .contains_key(conversation_key)
                {
                    *self
                        .metadata
                        .session_counts
                        .entry(conversation_key.clone())
                        .or_insert(0) += count;
                }
            }
        }
        for (conversation, ids) in message_ids {
            *self
                .metadata
                .session_counts
                .entry(conversation)
                .or_insert(0) += ids.len();
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

    /// Native client artifacts missing from disk that would be lost by a
    /// destructive cache rebuild. Managed Claude web imports have their own
    /// durable storage and are intentionally excluded.
    pub fn missing_native_artifact_count(&self) -> usize {
        self.metadata
            .indexed_files
            .iter()
            .filter(|(path, metadata)| metadata.source != Source::ClaudeWeb && !path.exists())
            .count()
    }

    /// Native artifacts that a rebuild cannot restore from the current
    /// discovery scope, whether absent on disk or simply no longer discoverable.
    pub fn at_risk_native_artifact_count(&self, discovered: &[PathBuf]) -> usize {
        self.metadata
            .indexed_files
            .iter()
            .filter(|(path, metadata)| {
                metadata.source != Source::ClaudeWeb
                    && (!path.exists() || !discovered.contains(path))
            })
            .count()
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
        let mut temporary = tempfile::NamedTempFile::new_in(&self.cache_dir)?;
        temporary.write_all(content.as_bytes())?;
        temporary
            .as_file()
            .sync_all()?;
        temporary
            .persist(&self.metadata_file)
            .map_err(|error| error.error)?;
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
        for path in all_jsonl_files {
            let Some(meta) = self
                .metadata
                .indexed_files
                .get(path)
            else {
                new_files += 1;
                continue;
            };
            if let Ok(current_mtime) = file_mtime(path) {
                let current_size = fs::metadata(path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                if current_size != meta.size
                    || current_mtime != meta.modified
                    || meta.parser_version != parser_version(meta.source)
                {
                    stale += 1;
                }
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
                if current_size != cached_meta.size
                    || current_mtime != cached_meta.modified
                    || cached_meta.parser_version != parser_version(cached_meta.source)
                {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::{
        ConversationEntry, MessageType, SearchEngine, SearchIndexer, SearchQuery, Source,
    };
    use chrono::Utc;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn write_web_artifact(path: &Path, messages: &[(&str, &str)]) {
        let messages: Vec<_> = messages
            .iter()
            .map(|(id, text)| {
                serde_json::json!({
                    "uuid": id,
                    "text": text,
                    "sender": "human",
                    "created_at": "2026-01-01T00:00:00Z"
                })
            })
            .collect();
        let document = serde_json::json!([{
            "uuid": "shared-session",
            "name": "Shared session",
            "chat_messages": messages
        }]);
        std::fs::write(path, document.to_string()).unwrap();
    }

    #[test]
    fn valid_empty_artifact_is_cached_and_second_pass_converges() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let artifact = temporary
            .path()
            .join("conversation-empty.claude-web.json");
        write_web_artifact(&artifact, &[]);
        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();

        let first = cache
            .update_incremental(&mut indexer, vec![artifact.clone()])
            .unwrap();
        assert_eq!(first.candidates, 1);
        assert_eq!(first.empty_artifacts, 1);
        assert_eq!(
            cache.quick_health_check(std::slice::from_ref(&artifact)),
            (0, 0)
        );
        assert_eq!(
            cache
                .metadata
                .indexed_files[&artifact]
                .entry_count,
            0
        );

        let second = cache
            .update_incremental(&mut indexer, vec![artifact.clone()])
            .unwrap();
        assert_eq!(second.candidates, 0);
        assert_eq!(second.unchanged_artifacts, 1);
    }

    #[test]
    fn retained_artifact_outside_discovery_does_not_report_stale() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let artifact = temporary
            .path()
            .join("conversation-retained.claude-web.json");
        write_web_artifact(&artifact, &[("message", "Original text")]);
        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();
        cache
            .update_incremental(&mut indexer, vec![artifact.clone()])
            .unwrap();

        write_web_artifact(&artifact, &[("message", "Changed text is longer")]);
        assert_eq!(cache.quick_health_check(&[]), (0, 0));
        assert_eq!(cache.quick_health_check(&[artifact]), (1, 0));
    }

    #[test]
    fn coexisting_artifacts_keep_unique_and_overlapping_records() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let first = temporary
            .path()
            .join("conversation-first.claude-web.json");
        let second = temporary
            .path()
            .join("conversation-second.claude-web.json");
        write_web_artifact(
            &first,
            &[("overlap", "Shared text"), ("first", "First text")],
        );
        write_web_artifact(
            &second,
            &[("overlap", "Shared text"), ("second", "Second text")],
        );
        let files = vec![first.clone(), second.clone()];
        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();

        let initial = cache
            .update_incremental(&mut indexer, files.clone())
            .unwrap();
        assert_eq!(initial.indexed_artifacts, 2);
        assert_eq!(cache.quick_health_check(&files), (0, 0));
        assert_eq!(
            cache
                .metadata
                .indexed_files
                .len(),
            2
        );
        assert_eq!(
            cache
                .metadata
                .total_entries,
            3
        );
        assert_eq!(
            cache.get_session_counts()[&Source::ClaudeWeb.conversation_key("shared-session")],
            3
        );
        let engine = SearchEngine::new(
            &cache_dir,
            cache
                .get_session_counts()
                .clone(),
        )
        .unwrap();
        let messages = engine
            .get_conversation_messages(Source::ClaudeWeb, "shared-session")
            .unwrap();
        assert_eq!(messages.len(), 3);
        assert!(
            messages
                .iter()
                .any(|message| message.uuid == "first")
        );
        assert!(
            messages
                .iter()
                .any(|message| message.uuid == "second")
        );
        let matches = engine
            .search(SearchQuery {
                text: "text".to_string(),
                limit: 3,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(matches.len(), 3);
        assert_eq!(
            engine
                .aggregate_conversation_stats(None)
                .unwrap()
                .total_messages,
            3
        );

        write_web_artifact(
            &first,
            &[("overlap", "Shared text"), ("first", "First updated")],
        );
        let changed = cache
            .update_incremental_forced(&mut indexer, vec![first.clone()])
            .unwrap();
        assert_eq!(changed.indexed_artifacts, 1);
        assert_eq!(
            cache
                .metadata
                .indexed_files
                .len(),
            2
        );
        let engine = SearchEngine::new(
            &cache_dir,
            cache
                .get_session_counts()
                .clone(),
        )
        .unwrap();
        let messages = engine
            .get_conversation_messages(Source::ClaudeWeb, "shared-session")
            .unwrap();
        assert_eq!(messages.len(), 3);
        assert!(
            messages
                .iter()
                .any(|message| message.uuid == "second")
        );
        assert!(
            messages
                .iter()
                .any(|message| message.content == "First updated")
        );
        assert_eq!(
            cache
                .update_incremental(&mut indexer, files.clone())
                .unwrap()
                .candidates,
            0
        );

        let rebuilt_dir = temporary
            .path()
            .join("rebuilt");
        let mut rebuilt_indexer = SearchIndexer::new(&rebuilt_dir).unwrap();
        let mut rebuilt_cache = CacheManager::new(&rebuilt_dir).unwrap();
        rebuilt_cache
            .update_incremental(&mut rebuilt_indexer, files)
            .unwrap();
        let rebuilt = SearchEngine::new(
            &rebuilt_dir,
            rebuilt_cache
                .get_session_counts()
                .clone(),
        )
        .unwrap();
        let mut incremental_records: Vec<_> = messages
            .iter()
            .map(|message| {
                (
                    message
                        .uuid
                        .as_str(),
                    message
                        .content
                        .as_str(),
                )
            })
            .collect();
        let rebuilt_messages = rebuilt
            .get_conversation_messages(Source::ClaudeWeb, "shared-session")
            .unwrap();
        let mut rebuilt_records: Vec<_> = rebuilt_messages
            .iter()
            .map(|message| {
                (
                    message
                        .uuid
                        .as_str(),
                    message
                        .content
                        .as_str(),
                )
            })
            .collect();
        incremental_records.sort();
        rebuilt_records.sort();
        assert_eq!(incremental_records, rebuilt_records);
    }

    #[test]
    fn parse_failures_remain_pending() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let artifact = temporary
            .path()
            .join("conversation-invalid.claude-web.json");
        std::fs::write(&artifact, "invalid JSON").unwrap();
        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();
        let outcome = cache
            .update_incremental(&mut indexer, vec![artifact.clone()])
            .unwrap();
        assert_eq!(outcome.parse_failures, 1);
        assert_eq!(outcome.indexed_artifacts, 0);
        assert_eq!(cache.quick_health_check(&[artifact]), (0, 1));
    }

    #[test]
    fn moving_one_fragment_preserves_a_coexisting_sibling() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let original = temporary
            .path()
            .join("conversation-original.claude-web.json");
        let sibling = temporary
            .path()
            .join("conversation-sibling.claude-web.json");
        let moved = temporary
            .path()
            .join("conversation-moved.claude-web.json");
        write_web_artifact(&original, &[("original", "Original record")]);
        write_web_artifact(&sibling, &[("sibling", "Sibling record")]);
        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();
        cache
            .update_incremental(&mut indexer, vec![original.clone(), sibling.clone()])
            .unwrap();

        std::fs::rename(&original, &moved).unwrap();
        cache
            .update_incremental(&mut indexer, vec![moved.clone(), sibling.clone()])
            .unwrap();
        assert!(
            !cache
                .metadata
                .indexed_files
                .contains_key(&original)
        );
        assert!(
            cache
                .metadata
                .indexed_files
                .contains_key(&moved)
        );
        assert!(
            cache
                .metadata
                .indexed_files
                .contains_key(&sibling)
        );
        let engine = SearchEngine::new(
            &cache_dir,
            cache
                .get_session_counts()
                .clone(),
        )
        .unwrap();
        let messages = engine
            .get_conversation_messages(Source::ClaudeWeb, "shared-session")
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert!(
            messages
                .iter()
                .any(|message| message.uuid == "original")
        );
        assert!(
            messages
                .iter()
                .any(|message| message.uuid == "sibling")
        );
    }

    #[test]
    fn automatic_indexing_retains_a_conversation_after_its_source_disappears() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let artifact = temporary
            .path()
            .join("conversation-retained.claude-web.json");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/claude_export/conversations.json"),
            &artifact,
        )
        .unwrap();

        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();
        cache
            .update_incremental(&mut indexer, vec![artifact.clone()])
            .unwrap();
        assert_eq!(
            cache
                .get_basic_stats()
                .0,
            1
        );

        std::fs::remove_file(&artifact).unwrap();
        // A source can disappear after discovery but before parsing. The
        // incremental boundary must retain its committed search document and
        // cache metadata in that race too.
        cache
            .update_incremental(&mut indexer, vec![artifact.clone()])
            .unwrap();

        let engine = SearchEngine::new(
            &cache_dir,
            cache
                .get_session_counts()
                .clone(),
        )
        .unwrap();
        assert_eq!(
            engine
                .get_conversation_messages(Source::ClaudeWeb, "conversation-a")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            cache
                .get_basic_stats()
                .0,
            1
        );
    }

    #[test]
    fn undiscovered_claude_and_codex_artifacts_are_at_risk_of_rebuild() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();

        for source in [Source::Claude, Source::Codex] {
            let session_id = format!("{source}-retained");
            let artifact = temporary
                .path()
                .join(format!("{source}-missing.jsonl"));
            std::fs::write(&artifact, "retained source outside discovery scope").unwrap();
            indexer
                .index_conversations(vec![ConversationEntry {
                    source,
                    uuid: format!("{source}-message"),
                    parent_uuid: None,
                    session_id: session_id.clone(),
                    source_artifact: artifact.clone(),
                    project_path: "retention-test".to_string(),
                    timestamp: Utc::now(),
                    message_type: MessageType::User,
                    record_kind: Default::default(),
                    content: format!("{source} durable searchable history"),
                    model: None,
                    cwd: None,
                    sequence_num: 0,
                    is_sidechain: false,
                    agent_id: None,
                    technologies: Vec::new(),
                    has_code: false,
                    code_languages: Vec::new(),
                    has_error: false,
                    tools_mentioned: Vec::new(),
                }])
                .unwrap();
            cache
                .metadata
                .indexed_files
                .insert(
                    artifact,
                    FileMetadata {
                        size_hex: "0".to_string(),
                        size: 0,
                        modified: Utc::now(),
                        indexed_at: Utc::now(),
                        entry_count: 1,
                        source,
                        parser_version: parser_version(source),
                        conversation_counts: HashMap::from([(
                            source.conversation_key(&session_id),
                            1,
                        )]),
                        conversation_entry_counts: HashMap::from([(
                            source.conversation_key(&session_id),
                            1,
                        )]),
                        primary_record_ids: HashMap::new(),
                        message_ids: HashMap::new(),
                    },
                );
        }
        indexer
            .commit()
            .unwrap();
        cache.refresh_derived_metadata();
        cache
            .save_metadata()
            .unwrap();

        assert_eq!(cache.missing_native_artifact_count(), 0);
        assert_eq!(cache.at_risk_native_artifact_count(&[]), 2);

        let engine = SearchEngine::new(
            &cache_dir,
            cache
                .get_session_counts()
                .clone(),
        )
        .unwrap();
        for source in [Source::Claude, Source::Codex] {
            assert_eq!(
                engine
                    .get_conversation_messages(source, &format!("{source}-retained"))
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[test]
    fn moved_artifact_supersedes_source_qualified_conversations() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let original = temporary
            .path()
            .join("conversation-original.claude-web.json");
        let moved = temporary
            .path()
            .join("conversation-moved.claude-web.json");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/claude_export/conversations.json"),
            &original,
        )
        .unwrap();
        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();
        cache
            .update_incremental(&mut indexer, vec![original.clone()])
            .unwrap();
        std::fs::rename(&original, &moved).unwrap();
        cache
            .update_incremental(&mut indexer, vec![moved])
            .unwrap();

        let engine = SearchEngine::new(
            &cache_dir,
            cache
                .get_session_counts()
                .clone(),
        )
        .unwrap();
        assert_eq!(
            engine
                .get_conversation_messages(Source::ClaudeWeb, "conversation-a")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            cache
                .get_session_counts()
                .get(&Source::ClaudeWeb.conversation_key("conversation-a")),
            Some(&2)
        );
    }

    #[test]
    fn legacy_metadata_is_removed_when_a_moved_artifact_supersedes_it() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let original = temporary
            .path()
            .join("conversation-legacy.claude-web.json");
        let moved = temporary
            .path()
            .join("conversation-legacy-moved.claude-web.json");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/claude_export/conversations.json"),
            &original,
        )
        .unwrap();
        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();
        cache
            .update_incremental(&mut indexer, vec![original.clone()])
            .unwrap();
        cache
            .metadata
            .indexed_files
            .get_mut(&original)
            .unwrap()
            .conversation_entry_counts
            .clear();
        std::fs::rename(&original, &moved).unwrap();
        cache
            .update_incremental(&mut indexer, vec![moved])
            .unwrap();

        assert!(
            !cache
                .metadata
                .indexed_files
                .contains_key(&original)
        );
        assert_eq!(
            cache
                .metadata
                .total_entries,
            3
        );
        assert_eq!(
            cache
                .get_session_counts()
                .get(&Source::ClaudeWeb.conversation_key("conversation-a")),
            Some(&2)
        );
        let engine = SearchEngine::new(
            &cache_dir,
            cache
                .get_session_counts()
                .clone(),
        )
        .unwrap();
        assert_eq!(
            engine
                .get_conversation_messages(Source::ClaudeWeb, "conversation-a")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn malformed_metadata_is_an_actionable_error() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("cache-metadata.json"), "not json").unwrap();
        let error = CacheManager::new(&cache_dir)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("Refusing to discard retention information"));
    }

    #[test]
    fn destructive_reset_recovers_from_malformed_metadata() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("cache-metadata.json"), "not json").unwrap();

        let mut cache = CacheManager::new_for_destructive_reset(&cache_dir).unwrap();
        assert_eq!(cache.get_basic_stats(), (0, 0, None));
        cache
            .clear_cache()
            .unwrap();

        let recovered = CacheManager::new(&cache_dir).unwrap();
        assert_eq!(recovered.get_basic_stats(), (0, 0, None));
    }

    #[test]
    fn forced_import_refreshes_same_size_same_second_artifact() {
        let temporary = TempDir::new().unwrap();
        let cache_dir = temporary
            .path()
            .join("cache");
        let artifact = temporary
            .path()
            .join("conversation-refresh.claude-web.json");
        let initial = r#"{"uuid":"same","name":"Same","chat_messages":[{"uuid":"m","text":"alpha","sender":"human","created_at":"2026-01-01T00:00:00Z"}]}"#;
        let replacement = initial.replace("alpha", "bravo");
        assert_eq!(initial.len(), replacement.len());
        std::fs::write(&artifact, initial).unwrap();
        let mut indexer = SearchIndexer::new(&cache_dir).unwrap();
        let mut cache = CacheManager::new(&cache_dir).unwrap();
        cache
            .update_incremental(&mut indexer, vec![artifact.clone()])
            .unwrap();
        std::fs::write(&artifact, replacement).unwrap();
        cache
            .update_incremental_forced(&mut indexer, vec![artifact])
            .unwrap();
        let engine = SearchEngine::new(
            &cache_dir,
            cache
                .get_session_counts()
                .clone(),
        )
        .unwrap();
        assert_eq!(
            engine
                .get_conversation_messages(Source::ClaudeWeb, "same")
                .unwrap()[0]
                .content,
            "bravo"
        );
    }
}
