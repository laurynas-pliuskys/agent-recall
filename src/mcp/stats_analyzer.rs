use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use tracing::debug;

use super::server::{CallToolResponse, ToolResult};
use crate::shared::{CacheManager, SearchEngine, SearchQuery, SortOrder};

pub async fn handle_get_stats(
    search_engine: Option<&SearchEngine>,
    cache_manager: Option<&CacheManager>,
    args: Option<Value>,
) -> Result<Value> {
    let args = args.unwrap_or_default();
    let project_filter = args
        .get("project")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    debug!("Getting stats for project filter: {:?}", project_filter);

    // Get cache info
    let cache_stats = if let Some(cache_mgr) = cache_manager {
        cache_mgr.get_stats()
    } else {
        return Ok(serde_json::to_value(CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: "Cache manager not initialized".to_string(),
            }],
            is_error: Some(true),
        })?);
    };

    // Get search results for analysis
    let query = SearchQuery {
        text: "*".to_string(),
        source_filter: None,
        project_filter: project_filter.clone(),
        session_filter: None,
        limit: 1000, // Get a large sample for stats
        sort_by: SortOrder::default(),
        after: None,
        before: None,
    };

    let search_engine =
        search_engine.ok_or_else(|| anyhow::anyhow!("Search engine not initialized"))?;
    let results = search_engine.search(query)?;

    if results.is_empty() {
        let msg = if let Some(ref proj) = project_filter {
            format!("No conversations found for project: {}", proj)
        } else {
            "No conversations found in index".to_string()
        };

        return Ok(serde_json::to_value(CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: msg,
            }],
            is_error: Some(true),
        })?);
    }

    // Analyze conversation data
    let mut session_count = std::collections::HashSet::new();
    let mut project_counts = HashMap::new();
    let mut tech_counts = HashMap::new();
    let mut lang_counts = HashMap::new();
    let mut monthly_counts = HashMap::new();
    let mut has_code_count = 0;
    let mut has_error_count = 0;
    let mut total_chars = 0;

    for result in &results {
        session_count.insert(&result.session_id);
        *project_counts
            .entry(&result.project)
            .or_insert(0) += 1;
        total_chars += result
            .content
            .len();

        if result.has_code {
            has_code_count += 1;
        }
        if result.has_error {
            has_error_count += 1;
        }

        // Count technologies and languages
        for tech in &result.technologies {
            *tech_counts
                .entry(tech)
                .or_insert(0) += 1;
        }
        for lang in &result.code_languages {
            *lang_counts
                .entry(lang)
                .or_insert(0) += 1;
        }

        // Count by month
        let month_key = result
            .timestamp
            .format("%Y-%m")
            .to_string();
        *monthly_counts
            .entry(month_key)
            .or_insert(0) += 1;
    }

    // Sort projects by count
    let mut project_stats: Vec<_> = project_counts
        .into_iter()
        .collect();
    project_stats.sort_by(|a, b| {
        b.1.cmp(&a.1)
    });

    // Sort tech by count
    let mut tech_stats: Vec<_> = tech_counts
        .into_iter()
        .collect();
    tech_stats.sort_by(|a, b| {
        b.1.cmp(&a.1)
    });
    tech_stats.truncate(15); // Top 15

    // Sort languages by count
    let mut lang_stats: Vec<_> = lang_counts
        .into_iter()
        .collect();
    lang_stats.sort_by(|a, b| {
        b.1.cmp(&a.1)
    });
    lang_stats.truncate(10); // Top 10

    // Sort months chronologically
    let mut monthly_stats: Vec<_> = monthly_counts
        .into_iter()
        .collect();
    monthly_stats.sort_by(|a, b| {
        a.0.cmp(&b.0)
    });

    let mut output = String::new();

    // Header
    let title = if let Some(ref proj) = project_filter {
        format!("Conversation Statistics - Project: {}", proj)
    } else {
        "Conversation Statistics - All Projects".to_string()
    };
    output.push_str(&format!("# {}\n\n", title));

    // Overall stats
    output.push_str("## Overview\n");
    output.push_str(&format!("**Total Messages**: {}\n", results.len()));
    output.push_str(&format!("**Unique Sessions**: {}\n", session_count.len()));
    output.push_str(&format!("**Projects**: {}\n", project_stats.len()));
    output.push_str(&format!(
        "**Messages with Code**: {} ({:.1}%)\n",
        has_code_count,
        (has_code_count as f32 / results.len() as f32) * 100.0
    ));
    output.push_str(&format!(
        "**Messages with Errors**: {} ({:.1}%)\n",
        has_error_count,
        (has_error_count as f32 / results.len() as f32) * 100.0
    ));
    output.push_str(&format!(
        "**Total Content**: {:.1} MB\n\n",
        total_chars as f32 / 1_048_576.0
    ));

    // Cache stats
    output.push_str("## Index Status\n");
    output.push_str(&format!(
        "**Index Size**: {:.1} MB\n",
        cache_stats.cache_size_mb
    ));
    if let Some(last_updated) = cache_stats.last_updated {
        output.push_str(&format!(
            "**Last Updated**: {}\n",
            last_updated.format("%Y-%m-%d %H:%M")
        ));
    }
    output.push_str(&format!("**Total Files**: {}\n", cache_stats.total_files));
    output.push_str(&format!(
        "**Total Entries**: {}\n\n",
        cache_stats.total_entries
    ));

    // Project breakdown (if showing all projects)
    if project_filter.is_none() && project_stats.len() > 1 {
        output.push_str("## Projects\n");
        for (project, count) in project_stats
            .iter()
            .take(10)
        {
            let percentage = (*count as f32 / results.len() as f32) * 100.0;
            output.push_str(&format!(
                "**{}**: {} messages ({:.1}%)\n",
                project, count, percentage
            ));
        }
        if project_stats.len() > 10 {
            output.push_str(&format!(
                "... and {} more projects\n",
                project_stats.len() - 10
            ));
        }
        output.push('\n');
    }

    // Technology usage
    if !tech_stats.is_empty() {
        output.push_str("## Top Technologies\n");
        for (tech, count) in &tech_stats {
            let percentage = (*count as f32 / results.len() as f32) * 100.0;
            output.push_str(&format!(
                "**{}**: {} mentions ({:.1}%)\n",
                tech, count, percentage
            ));
        }
        output.push('\n');
    }

    // Language usage
    if !lang_stats.is_empty() {
        output.push_str("## Programming Languages\n");
        for (lang, count) in &lang_stats {
            let percentage = (*count as f32 / results.len() as f32) * 100.0;
            output.push_str(&format!(
                "**{}**: {} mentions ({:.1}%)\n",
                lang, count, percentage
            ));
        }
        output.push('\n');
    }

    // Monthly activity
    if monthly_stats.len() > 1 {
        output.push_str("## Activity by Month\n");
        for (month, count) in &monthly_stats {
            output.push_str(&format!("**{}**: {} messages\n", month, count));
        }
        output.push('\n');
    }

    Ok(serde_json::to_value(CallToolResponse {
        content: vec![ToolResult {
            result_type: "text".to_string(),
            text: output,
        }],
        is_error: None,
    })?)
}
