use crate::shared::{
    CacheManager, ExclusiveIndexAccess, SearchIndexer, SharedIndexAccess, discover_jsonl_files,
};
use anyhow::Result;
use std::path::Path;
use tracing::info;

pub fn show_status(index_path: &Path) -> Result<()> {
    println!("Index Status");
    println!("============");

    // Check lock status
    if ExclusiveIndexAccess::is_available() {
        println!("Lock Status: Available");
    } else if SharedIndexAccess::is_available() {
        println!("Lock Status: Read-only access available");
    } else {
        println!("Lock Status: Locked by another process");
    }

    if !index_path.exists() {
        println!("Index: Not found (will be created on next search)");
        return Ok(());
    }

    // Try to acquire shared lock to read stats
    let _lock = match SharedIndexAccess::acquire() {
        Ok(lock) => lock,
        Err(e) => {
            println!("Index: Unable to read ({})", e);
            return Ok(());
        }
    };

    let cache_manager = CacheManager::new(index_path)?;
    let (total_files, total_entries, last_updated) = cache_manager.get_basic_stats();

    println!("Index Path: {}", index_path.display());
    println!("Total Files: {}", total_files);
    println!("Total Entries: {}", total_entries);

    if let Some(last_updated) = last_updated {
        println!(
            "Last Updated: {}",
            last_updated.format("%Y-%m-%d %H:%M:%S UTC")
        );
    } else {
        println!("Last Updated: Never");
    }

    // Show disk usage
    let cache_size_mb = if let Ok(entries) = std::fs::read_dir(index_path) {
        let total_bytes: u64 = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| std::fs::metadata(entry.path()).ok())
            .map(|metadata| metadata.len())
            .sum();
        total_bytes as f64 / (1024.0 * 1024.0)
    } else {
        0.0
    };

    println!("Index Size: {:.2} MB", cache_size_mb);

    Ok(())
}

pub fn rebuild(index_path: &Path) -> Result<()> {
    info!("Starting index rebuild...");

    // Acquire exclusive lock
    let _lock = ExclusiveIndexAccess::acquire()?;

    let mut cache_manager = CacheManager::new(index_path)?;
    cache_manager.clear_cache()?;

    let mut indexer = SearchIndexer::new(index_path)?;
    let all_files = discover_jsonl_files()?;

    info!("Found {} files to process", all_files.len());
    cache_manager.update_incremental(&mut indexer, all_files)?;

    println!("Index rebuild completed successfully.");
    Ok(())
}

pub fn vacuum(index_path: &Path) -> Result<()> {
    info!("Starting index vacuum operation...");

    // Acquire exclusive lock
    let _lock = ExclusiveIndexAccess::acquire()?;

    if !index_path.exists() {
        println!("No index found to vacuum.");
        return Ok(());
    }

    // For now, vacuum is essentially a rebuild since Tantivy doesn't have
    // built-in vacuum. In the future, we could implement a more sophisticated
    // approach that only removes deleted entries.
    println!("Vacuuming index by rebuilding...");
    rebuild(index_path)?;

    println!("Index vacuum completed.");
    Ok(())
}
