use super::cache::CacheManager;
use super::config::get_config;
use super::indexer::SearchIndexer;
use super::lock::ExclusiveIndexAccess;
use super::path_utils::discover_jsonl_files;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::fs::{self};
use std::path::{Path, PathBuf};
use tracing::info;

pub fn get_cache_dir() -> Result<PathBuf> {
    get_config().get_cache_dir()
}

/// Get file modification time as DateTime<Utc>
pub fn file_mtime(path: &Path) -> Result<DateTime<Utc>> {
    let metadata = fs::metadata(path)?;
    let mtime = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    Ok(DateTime::from_timestamp(mtime, 0).unwrap_or_else(Utc::now))
}

/// Truncate string at UTF-8 character boundary, optionally collapsing whitespace
pub fn truncate_content(s: &str, max_chars: usize, collapse_whitespace: bool) -> String {
    let processed = if collapse_whitespace {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        s.to_string()
    };

    if processed
        .chars()
        .count()
        <= max_chars
    {
        processed
    } else {
        let truncated: String = processed
            .chars()
            .take(max_chars - 1)
            .collect();
        format!("{}…", truncated)
    }
}

pub fn auto_index(index_path: &Path) -> Result<()> {
    if !get_config()
        .index
        .auto_index_on_startup
    {
        return Ok(());
    }
    index_now_with_forced_files(index_path, None, true)
}

/// Run an explicit incremental indexing operation. Unlike [`auto_index`], an
/// explicit caller never honors the startup convenience switch.
pub fn index_now(index_path: &Path) -> Result<()> {
    index_now_with_forced_files(index_path, None, false)
}

/// Explicitly index selected artifacts even if their mtime and size appear
/// unchanged. Used after replacing managed import files.
pub fn index_now_forced(index_path: &Path, files: Vec<PathBuf>) -> Result<()> {
    index_now_with_forced_files(index_path, Some(files), false)
}

fn index_now_with_forced_files(
    index_path: &Path,
    forced_files: Option<Vec<PathBuf>>,
    skip_on_busy: bool,
) -> Result<()> {
    let _lock = match ExclusiveIndexAccess::acquire() {
        Ok(lock) => lock,
        Err(error) if skip_on_busy => {
            info!("Skipping auto-index: another process is currently indexing ({error})");
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    let mut indexer = if index_path
        .join("meta.json")
        .exists()
    {
        // Check if existing index has correct schema
        match SearchIndexer::validate_schema(index_path) {
            Ok(true) => {
                // Schema is valid, open existing index
                SearchIndexer::open(index_path)?
            }
            Ok(false) => anyhow::bail!(
                "Index schema mismatch; automatic indexing will not discard retained history. Run `agent-recall index rebuild --allow-history-loss` after reviewing the consequences."
            ),
            Err(error) => anyhow::bail!(
                "Index validation failed ({error}); automatic indexing will not discard retained history. Repair the index or run `agent-recall index rebuild --allow-history-loss` deliberately."
            ),
        }
    } else {
        info!("No index found, creating new one...");
        SearchIndexer::new(index_path)?
    };

    let mut cache_manager = CacheManager::new(index_path)?;
    let all_files = discover_jsonl_files()?;
    cache_manager.update_incremental(&mut indexer, all_files)?;
    if let Some(files) = forced_files {
        cache_manager.update_incremental_forced(&mut indexer, files)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn invalid_existing_index_is_preserved_for_explicit_recovery() {
        let temporary = TempDir::new().unwrap();
        let index = temporary
            .path()
            .join("index");
        let marker = index.join("meta.json");
        std::fs::create_dir_all(&index).unwrap();
        let mut builder = tantivy::schema::Schema::builder();
        builder.add_text_field("wrong", tantivy::schema::TEXT);
        let invalid = tantivy::Index::create_in_dir(&index, builder.build()).unwrap();
        invalid
            .writer::<tantivy::TantivyDocument>(15_000_000)
            .unwrap()
            .commit()
            .unwrap();
        drop(invalid);
        assert!(marker.exists());
        assert!(!SearchIndexer::validate_schema(&index).unwrap());

        assert!(index_now(&index).is_err());
        assert!(marker.exists());
    }
}
