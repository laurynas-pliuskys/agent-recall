use crate::cli::index;
use crate::shared::{self, CacheManager, DisplayOptions, SearchEngine, SearchQuery, SortOrder};
use anyhow::Result;
use chrono::{NaiveDate, TimeZone, Utc};
use clap::{Subcommand, ValueEnum};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

#[derive(Subcommand)]
pub enum CliCommands {
    /// Index management
    Index {
        #[command(subcommand)]
        action: Option<IndexAction>,
    },
    /// Search conversations (auto-indexes if needed)
    Search {
        /// Search query
        query: String,
        /// Filter by source client (claude or codex)
        #[arg(long)]
        source: Option<String>,
        /// Filter by project
        #[arg(long)]
        project: Option<String>,
        /// Filter by session ID (prefix match)
        #[arg(long)]
        session: Option<String>,
        /// Results limit
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Context lines before and after match (like grep -C)
        #[arg(short = 'C', default_value = "2")]
        context: usize,
        /// Context lines before match (like grep -B)
        #[arg(short = 'B')]
        ctx_before: Option<usize>,
        /// Context lines after match (like grep -A)
        #[arg(short = 'A')]
        ctx_after: Option<usize>,
        /// Exclude projects by name
        #[arg(long)]
        exclude_project: Vec<String>,
        /// Exclude results matching regex patterns
        #[arg(long)]
        exclude_pattern: Vec<String>,
        /// Sort order
        #[arg(long, value_enum, default_value = "relevance")]
        sort: SortArg,
        /// Results after date (YYYY-MM-DD or ISO 8601)
        #[arg(long)]
        after: Option<String>,
        /// Results before date (YYYY-MM-DD or ISO 8601)
        #[arg(long)]
        before: Option<String>,
        /// Include extra content types
        #[arg(long, value_enum)]
        include: Vec<IncludeArg>,
        /// Characters shown per message (0 = full content)
        #[arg(long, default_value = "300")]
        truncate: usize,
    },
    /// Show technology topics and their usage across conversations
    Topics {
        /// Filter by project
        #[arg(long)]
        project: Option<String>,
        /// Results limit
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Show detailed cache and conversation statistics
    Stats {
        /// Filter by project
        #[arg(long)]
        project: Option<String>,
    },
    /// View specific session conversations
    Session {
        /// Session ID to view
        session_id: String,
        /// Source for an otherwise ambiguous session ID
        #[arg(long)]
        source: Option<String>,
        /// Show full content (not just snippets)
        #[arg(long)]
        full: bool,
        /// Center on a message UUID (prefix match)
        #[arg(long)]
        center: Option<String>,
        /// Context messages before and after center (like grep -C)
        #[arg(short = 'C', default_value = "5")]
        context: usize,
        /// Context messages before center (like grep -B)
        #[arg(short = 'B')]
        before: Option<usize>,
        /// Context messages after center (like grep -A)
        #[arg(short = 'A')]
        after: Option<usize>,
        /// Characters shown per message (0 = full content)
        #[arg(long, default_value = "200")]
        truncate: usize,
    },
    /// Summarize a session using Claude (runs in jailed empty dir)
    Summary {
        /// Session ID to summarize
        session_id: String,
    },
    /// Cache management
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
    /// Run as MCP server
    Mcp,
    /// Register with Claude MCP
    Install {
        /// Use project scope instead of user scope
        #[arg(long)]
        project: bool,
    },
}

#[derive(Subcommand)]
pub enum CacheAction {
    /// Show cache statistics
    Info,
    /// Clear cache and rebuild
    Clear,
}

#[derive(ValueEnum, Clone, Default)]
pub enum SortArg {
    #[default]
    Relevance,
    DateDesc,
    DateAsc,
}

#[derive(ValueEnum, Clone, PartialEq)]
pub enum IncludeArg {
    Thinking,
    Tools,
}

impl From<SortArg> for SortOrder {
    fn from(s: SortArg) -> Self {
        match s {
            SortArg::Relevance => SortOrder::Relevance,
            SortArg::DateDesc => SortOrder::DateDesc,
            SortArg::DateAsc => SortOrder::DateAsc,
        }
    }
}

#[derive(Subcommand, Default)]
pub enum IndexAction {
    /// Show index status and statistics (default)
    #[default]
    Status,
    /// Force full rebuild of the index
    Rebuild,
    /// Clean up deleted entries from index
    Vacuum,
}

pub fn setup_logging(verbose: u8) {
    let level = match verbose {
        0 => Level::ERROR,
        1 => Level::WARN,
        2 => Level::INFO,
        _ => Level::DEBUG,
    };

    FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .init();
}

pub fn run_cli(verbose: u8, command: CliCommands) -> Result<()> {
    setup_logging(verbose);

    match command {
        CliCommands::Index { action } => {
            let config = shared::get_config();
            let index_path = config.get_cache_dir()?;
            match action.unwrap_or_default() {
                IndexAction::Status => index::show_status(&index_path)?,
                IndexAction::Rebuild => index::rebuild(&index_path)?,
                IndexAction::Vacuum => index::vacuum(&index_path)?,
            }
        }
        CliCommands::Completions { .. } => unreachable!("Completions handled in main"),
        CliCommands::Mcp => unreachable!("MCP handled in main"),
        CliCommands::Search {
            query,
            source,
            project,
            session,
            limit,
            context,
            ctx_before,
            ctx_after,
            exclude_project,
            exclude_pattern,
            sort,
            after,
            before,
            include,
            truncate,
        } => {
            let config = shared::get_config();
            let index_path = config.get_cache_dir()?;
            shared::auto_index(&index_path)?;
            let cb = ctx_before.unwrap_or(context);
            let ca = ctx_after.unwrap_or(context);
            let source_filter = source
                .as_deref()
                .map(|s| s.parse::<shared::Source>())
                .transpose()
                .map_err(|e| anyhow::anyhow!(e))?;
            let opts = SearchOpts {
                query,
                source: source_filter,
                project,
                session,
                limit,
                context_before: cb,
                context_after: ca,
                exclude_projects: exclude_project,
                exclude_patterns: exclude_pattern,
                sort: sort.into(),
                after: after
                    .as_deref()
                    .map(parse_date)
                    .transpose()?,
                before: before
                    .as_deref()
                    .map(parse_date)
                    .transpose()?,
                display: DisplayOptions {
                    include_thinking: include.contains(&IncludeArg::Thinking),
                    include_tools: include.contains(&IncludeArg::Tools),
                    truncate_length: truncate,
                },
            };
            search_conversations(&index_path, opts)?;
        }
        CliCommands::Topics { project, limit } => {
            let config = shared::get_config();
            let index_path = config.get_cache_dir()?;
            shared::auto_index(&index_path)?;
            show_topics(&index_path, project, limit)?;
        }
        CliCommands::Stats { project } => {
            let config = shared::get_config();
            let index_path = config.get_cache_dir()?;
            shared::auto_index(&index_path)?;
            show_stats(&index_path, project)?;
        }
        CliCommands::Session {
            session_id,
            source,
            full,
            center,
            context,
            before,
            after,
            truncate,
        } => {
            let config = shared::get_config();
            let index_path = config.get_cache_dir()?;
            shared::auto_index(&index_path)?;
            let ctx_before = before.unwrap_or(context);
            let ctx_after = after.unwrap_or(context);
            let max_content = if full { 0 } else { truncate };
            view_session(
                &index_path,
                session_id,
                source
                    .as_deref()
                    .map(str::parse::<shared::Source>)
                    .transpose()
                    .map_err(|error| anyhow::anyhow!(error))?,
                max_content,
                center,
                ctx_before,
                ctx_after,
            )?;
        }
        CliCommands::Summary { session_id } => {
            let config = shared::get_config();
            let index_path = config.get_cache_dir()?;
            shared::auto_index(&index_path)?;
            summarize_session(&index_path, session_id)?;
        }
        CliCommands::Cache { action } => {
            let config = shared::get_config();
            let index_path = config.get_cache_dir()?;
            match action {
                CacheAction::Info => show_cache_info(&index_path)?,
                CacheAction::Clear => clear_cache(&index_path)?,
            }
        }
        CliCommands::Install { project } => install(project)?,
    }

    Ok(())
}

fn install(project_scope: bool) -> Result<()> {
    use std::process::Command;

    let exe = std::env::current_exe()?;
    let exe_path = exe
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid exe path"))?;
    let scope = if project_scope { "project" } else { "user" };

    let _ = Command::new("claude")
        .args(["mcp", "remove", "-s", scope, "agent-recall"])
        .status();

    let status = Command::new("claude")
        .args(["mcp", "add", "-s", scope, "agent-recall", exe_path])
        .status()?;

    if !status.success() {
        anyhow::bail!("claude mcp add failed");
    }

    println!("{}", exe_path);
    Ok(())
}

fn show_cache_info(index_path: &Path) -> Result<()> {
    let cache_manager = CacheManager::new(index_path)?;
    let stats = cache_manager.get_stats();

    println!("Cache Statistics:");
    println!("  Total files indexed: {}", stats.total_files);
    println!("  Total entries: {}", stats.total_entries);
    println!("  Cache size: {:.2} MB", stats.cache_size_mb);

    if let Some(last_updated) = stats.last_updated {
        println!(
            "  Last updated: {}",
            last_updated.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }

    if !stats
        .projects
        .is_empty()
    {
        println!("\nProject breakdown:");
        for project in stats
            .projects
            .iter()
            .take(10)
        {
            println!(
                "  {} - {} files, {} entries (updated: {})",
                project.name,
                project.files,
                project.entries,
                project
                    .last_updated
                    .format("%Y-%m-%d")
            );
        }
        if stats
            .projects
            .len()
            > 10
        {
            println!(
                "  ... and {} more projects",
                stats
                    .projects
                    .len()
                    - 10
            );
        }
    }

    Ok(())
}

fn clear_cache(index_path: &Path) -> Result<()> {
    let mut cache_manager = CacheManager::new(index_path)?;
    cache_manager.clear_cache()?;
    println!("Cache cleared successfully. Run 'agent-recall index' to rebuild.");
    Ok(())
}

struct SearchOpts {
    query: String,
    source: Option<shared::Source>,
    project: Option<String>,
    session: Option<String>,
    limit: usize,
    context_before: usize,
    context_after: usize,
    exclude_projects: Vec<String>,
    exclude_patterns: Vec<String>,
    sort: SortOrder,
    after: Option<chrono::DateTime<Utc>>,
    before: Option<chrono::DateTime<Utc>>,
    display: DisplayOptions,
}

fn parse_date(s: &str) -> Result<chrono::DateTime<Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(Utc.from_utc_datetime(
            &date
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        ));
    }
    anyhow::bail!("Invalid date '{}': use YYYY-MM-DD or ISO 8601", s)
}

fn search_conversations(index_path: &Path, opts: SearchOpts) -> Result<()> {
    if !index_path.exists() {
        println!("Index not found. Please run 'agent-recall index' first.");
        return Ok(());
    }

    let config = shared::get_config();
    let mut all_exclude_patterns = config
        .search
        .exclude_patterns
        .clone();
    all_exclude_patterns.extend(opts.exclude_patterns);

    let exclude_regexes: Vec<Regex> = all_exclude_patterns
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect();

    let cache = CacheManager::new(index_path)?;
    let search_engine = SearchEngine::new(
        index_path,
        cache
            .get_session_counts()
            .clone(),
    )?;

    let query = SearchQuery {
        text: opts.query,
        source_filter: opts.source,
        project_filter: opts.project,
        session_filter: opts.session,
        limit: opts.limit * 3,
        sort_by: opts.sort,
        after: opts.after,
        before: opts.before,
    };

    let results = search_engine.search_with_context_options(
        query,
        opts.context_before,
        opts.context_after,
        opts.display
            .include_tools,
    )?;

    let mut session_seen = std::collections::HashSet::new();
    let filtered: Vec<_> = results
        .into_iter()
        .filter(|r| {
            let proj = &r
                .matched_message
                .project;
            let path = &r
                .matched_message
                .project_path;

            if opts
                .exclude_projects
                .contains(proj)
            {
                return false;
            }
            for regex in &exclude_regexes {
                if regex.is_match(proj) || regex.is_match(path) {
                    return false;
                }
            }
            session_seen.insert(
                r.matched_message
                    .source
                    .conversation_key(
                        &r.matched_message
                            .session_id,
                    ),
            )
        })
        .take(opts.limit)
        .collect();

    if filtered.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    let ctx_display = if opts.context_before == opts.context_after {
        format!("-C {}", opts.context_before)
    } else {
        format!("-B {} -A {}", opts.context_before, opts.context_after)
    };
    println!("Found {} results ({}):\n", filtered.len(), ctx_display);

    for (i, result) in filtered
        .iter()
        .enumerate()
    {
        print!("{}", result.format_compact_with_options(i, &opts.display));
        if i < filtered.len() - 1 {
            println!();
        }
    }

    Ok(())
}

fn show_topics(index_path: &Path, project_filter: Option<String>, limit: usize) -> Result<()> {
    if !index_path.exists() {
        println!("Index not found. Please run 'agent-recall index' first.");
        return Ok(());
    }

    let cache = CacheManager::new(index_path)?;
    let search_engine = SearchEngine::new(
        index_path,
        cache
            .get_session_counts()
            .clone(),
    )?;

    // Get all conversations to analyze topics
    let query = SearchQuery {
        text: "*".to_string(), // Match everything
        source_filter: None,
        project_filter: project_filter.clone(),
        session_filter: None,
        limit: 100_000,
        sort_by: SortOrder::default(),
        after: None,
        before: None,
    };

    let results = search_engine.search(query)?;

    // Count technology mentions
    let mut tech_counts = HashMap::new();
    let mut lang_counts = HashMap::new();
    let mut tool_counts = HashMap::new();
    let mut project_counts = HashMap::new();

    for result in &results {
        project_counts
            .entry(
                result
                    .project
                    .clone(),
            )
            .and_modify(|count| *count += 1)
            .or_insert(1);

        for tech in &result.technologies {
            tech_counts
                .entry(tech.clone())
                .and_modify(|count| *count += 1)
                .or_insert(1);
        }

        for lang in &result.code_languages {
            lang_counts
                .entry(lang.clone())
                .and_modify(|count| *count += 1)
                .or_insert(1);
        }

        for tool in &result.tools_mentioned {
            tool_counts
                .entry(tool.clone())
                .and_modify(|count| *count += 1)
                .or_insert(1);
        }
    }

    println!(
        "Topic Analysis - {} conversations analyzed\n",
        results.len()
    );

    if let Some(ref project) = project_filter {
        println!("Filtered by project: {project}\n");
    }

    // Top technologies
    if !tech_counts.is_empty() {
        println!("🔧 Top Technologies:");
        let mut sorted_tech: Vec<_> = tech_counts
            .iter()
            .collect();
        sorted_tech.sort_by(|a, b| {
            b.1.cmp(a.1)
        });

        for (tech, count) in sorted_tech
            .iter()
            .take(limit)
        {
            println!("   {tech} ({count})");
        }
        println!();
    }

    // Top programming languages
    if !lang_counts.is_empty() {
        println!("💻 Top Programming Languages:");
        let mut sorted_lang: Vec<_> = lang_counts
            .iter()
            .collect();
        sorted_lang.sort_by(|a, b| {
            b.1.cmp(a.1)
        });

        for (lang, count) in sorted_lang
            .iter()
            .take(limit)
        {
            println!("   {lang} ({count})");
        }
        println!();
    }

    // Top tools mentioned
    if !tool_counts.is_empty() {
        println!("🔨 Top Tools Mentioned:");
        let mut sorted_tools: Vec<_> = tool_counts
            .iter()
            .collect();
        sorted_tools.sort_by(|a, b| {
            b.1.cmp(a.1)
        });

        for (tool, count) in sorted_tools
            .iter()
            .take(limit)
        {
            println!("   {tool} ({count})");
        }
        println!();
    }

    // Project breakdown (if not filtering by project)
    if project_filter.is_none() && !project_counts.is_empty() {
        println!("📂 Project Activity:");
        let mut sorted_projects: Vec<_> = project_counts
            .iter()
            .collect();
        sorted_projects.sort_by(|a, b| {
            b.1.cmp(a.1)
        });

        for (project, count) in sorted_projects
            .iter()
            .take(limit)
        {
            println!("   {project} ({count} conversations)");
        }
    }

    Ok(())
}

fn show_stats(index_path: &Path, project_filter: Option<String>) -> Result<()> {
    if !index_path.exists() {
        println!("Index not found. Please run 'agent-recall index' first.");
        return Ok(());
    }

    let cache_manager = CacheManager::new(index_path)?;
    let cache_stats = cache_manager.get_stats();
    let search_engine = SearchEngine::new(
        index_path,
        cache_manager
            .get_session_counts()
            .clone(),
    )?;

    // Get conversation stats
    let query = SearchQuery {
        text: "*".to_string(),
        source_filter: None,
        project_filter: project_filter.clone(),
        session_filter: None,
        limit: 1_000_000,
        sort_by: SortOrder::default(),
        after: None,
        before: None,
    };

    let results = search_engine.search(query)?;

    let mut code_conversations = 0;
    let mut error_conversations = 0;
    let mut total_interactions = 0;
    let mut session_counts = HashMap::new();

    for result in &results {
        if result.has_code {
            code_conversations += 1;
        }
        if result.has_error {
            error_conversations += 1;
        }
        total_interactions += result.interaction_count;

        session_counts
            .entry(
                result
                    .session_id
                    .clone(),
            )
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }

    if let Some(ref project) = project_filter {
        println!("📊 Statistics for project: {project}\n");
    } else {
        println!("📊 Overall Statistics\n");
    }

    println!("Cache Information:");
    println!("  📁 Total files indexed: {}", cache_stats.total_files);
    println!("  💾 Cache size: {:.2} MB", cache_stats.cache_size_mb);

    if let Some(last_updated) = cache_stats.last_updated {
        println!(
            "  🕒 Last updated: {}",
            last_updated.format("%Y-%m-%d %H:%M UTC")
        );
    }

    println!();

    let total_indexed = cache_stats.total_entries as usize;
    let sampled = results.len();

    println!("Conversation Analysis:");
    println!("  💬 Total messages indexed: {}", total_indexed);
    println!("  🏗️ Unique sessions: {}", session_counts.len());
    if sampled < total_indexed {
        println!(
            "  📊 Sampled for stats: {} ({:.1}%)",
            sampled,
            (sampled as f64 / total_indexed as f64) * 100.0
        );
    }
    println!(
        "  📝 Messages with code: {} ({:.1}%)",
        code_conversations,
        (code_conversations as f64 / sampled as f64) * 100.0
    );
    println!(
        "  🚨 Messages with errors: {} ({:.1}%)",
        error_conversations,
        (error_conversations as f64 / sampled as f64) * 100.0
    );
    println!(
        "  💬 Total interactions: {} (avg: {} per conversation)",
        total_interactions,
        if !results.is_empty() {
            total_interactions / results.len()
        } else {
            0
        }
    );

    // Show most active sessions
    if !session_counts.is_empty() {
        println!();
        println!("Most Active Sessions:");
        let mut sorted_sessions: Vec<_> = session_counts
            .iter()
            .collect();
        sorted_sessions.sort_by(|a, b| {
            b.1.cmp(a.1)
        });

        for (session_id, count) in sorted_sessions
            .iter()
            .take(5)
        {
            let short_id = if session_id.len() > 12 {
                format!("{}…", &session_id[..12])
            } else {
                session_id.to_string()
            };
            println!("  {short_id} ({count} messages)");
        }
    }

    Ok(())
}

fn view_session(
    index_path: &Path,
    session_id: String,
    source: Option<shared::Source>,
    truncate_length: usize,
    center_on: Option<String>,
    context_before: usize,
    context_after: usize,
) -> Result<()> {
    let entries = if index_path.exists() {
        let cache = CacheManager::new(index_path)?;
        let search_engine = SearchEngine::new(
            index_path,
            cache
                .get_session_counts()
                .clone(),
        )?;
        let results = if let Some(source) = source {
            search_engine.get_conversation_messages(source, &session_id)?
        } else {
            search_engine.get_session_messages(&session_id)?
        };
        if let Some(first) = results.first()
            && first
                .source_artifact
                .exists()
        {
            shared::conversation_source(first.source).parse(&first.source_artifact, true)?
        } else {
            eprintln!(
                "Warning: source artifact not found, falling back to index (content may be truncated)"
            );
            return view_session_from_results(
                results,
                &session_id,
                truncate_length,
                center_on,
                context_before,
                context_after,
            );
        }
    } else {
        println!("No JSONL file or index found for session: {session_id}");
        return Ok(());
    };

    if entries.is_empty() {
        println!("No messages found for session: {session_id}");
        return Ok(());
    }

    let displayable: Vec<_> = entries
        .iter()
        .filter(|e| e.is_displayable())
        .collect();
    let total = displayable.len();

    if total == 0 {
        println!("No displayable messages for session: {session_id}");
        return Ok(());
    }

    // Determine window: center_on mode vs full session
    let (window, center_idx) = if let Some(ref uuid) = center_on {
        let idx = displayable
            .iter()
            .position(|m| {
                m.uuid
                    .starts_with(uuid.as_str())
            })
            .unwrap_or_else(|| {
                eprintln!("Warning: message {uuid} not found, showing from start");
                0
            });
        let start = idx.saturating_sub(context_before);
        let end = (idx + context_after + 1).min(total);
        (&displayable[start..end], Some(idx))
    } else {
        (&displayable[..], None)
    };

    let project_path = shared::home_to_tilde(&entries[0].project_path);
    let time_range = format!(
        "{} - {}",
        entries[0]
            .timestamp
            .format("%Y-%m-%d %H:%M"),
        entries
            .last()
            .unwrap()
            .timestamp
            .format("%H:%M")
    );

    if center_on.is_some() {
        println!(
            "📁 {} 🗒️ {} ({}/{} msgs) ⏱️ {}",
            project_path,
            session_id,
            window.len(),
            total,
            time_range
        );
    } else {
        println!(
            "📁 {} 🗒️ {} ({} msgs) ⏱️ {}",
            project_path, session_id, total, time_range
        );
    }

    if center_on.is_none() {
        let mut techs = std::collections::HashSet::new();
        let mut langs = std::collections::HashSet::new();
        let mut has_code = false;
        let mut has_errors = false;
        for e in &entries {
            techs.extend(
                e.technologies
                    .iter()
                    .cloned(),
            );
            langs.extend(
                e.code_languages
                    .iter()
                    .cloned(),
            );
            has_code |= e.has_code;
            has_errors |= e.has_error;
        }
        let mut tags = Vec::new();
        if !techs.is_empty() {
            let mut t: Vec<_> = techs
                .into_iter()
                .collect();
            t.sort();
            tags.push(t.join(","));
        }
        if !langs.is_empty() {
            let mut l: Vec<_> = langs
                .into_iter()
                .collect();
            l.sort();
            tags.push(l.join(","));
        }
        if has_code {
            tags.push("code".to_string());
        }
        if has_errors {
            tags.push("error".to_string());
        }
        if !tags.is_empty() {
            println!("tags: {}", tags.join(" "));
        }
    }
    println!();

    for entry in window {
        let time = entry
            .timestamp
            .format("%H:%M:%S");
        let marker = if center_idx.is_some()
            && Some(&entry.uuid)
                == center_on
                    .as_ref()
                    .and_then(|u| {
                        if entry
                            .uuid
                            .starts_with(u.as_str())
                        {
                            Some(&entry.uuid)
                        } else {
                            None
                        }
                    }) {
            "»"
        } else {
            " "
        };
        let role = entry
            .message_type
            .short_name();
        let content = if truncate_length > 0 {
            let truncated: String = entry
                .content
                .chars()
                .take(truncate_length)
                .collect();
            let ellipsis = if entry
                .content
                .chars()
                .count()
                > truncate_length
            {
                "…"
            } else {
                ""
            };
            format!(
                "{}{}",
                truncated
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                ellipsis
            )
        } else {
            entry
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!("{marker} [{time}] {role}: {content}");
    }

    if truncate_length > 0
        && window
            .iter()
            .any(|e| {
                e.content
                    .chars()
                    .count()
                    > truncate_length
            })
    {
        println!("\nUse --full or --truncate 0 for complete content");
    }

    Ok(())
}

/// Fallback: display session from Tantivy SearchResult objects (pre-truncated content)
fn view_session_from_results(
    mut results: Vec<shared::SearchResult>,
    session_id: &str,
    truncate_length: usize,
    center_on: Option<String>,
    context_before: usize,
    context_after: usize,
) -> Result<()> {
    if results.is_empty() {
        println!("No messages found for session: {session_id}");
        println!("Tip: Use 'agent-recall stats' to see available session IDs");
        return Ok(());
    }

    results.sort_by_key(|r| r.timestamp);
    let displayable: Vec<_> = results
        .iter()
        .filter(|r| r.is_displayable())
        .collect();
    let total = displayable.len();

    let (window, center_idx) = if let Some(ref uuid) = center_on {
        let idx = displayable
            .iter()
            .position(|m| {
                m.uuid
                    .starts_with(uuid.as_str())
            })
            .unwrap_or(0);
        let start = idx.saturating_sub(context_before);
        let end = (idx + context_after + 1).min(total);
        (&displayable[start..end], Some(idx))
    } else {
        (&displayable[..], None)
    };

    let project_path = results[0].project_path_display();
    let time_range = format!(
        "{} - {}",
        results[0]
            .timestamp
            .format("%Y-%m-%d %H:%M"),
        results
            .last()
            .unwrap()
            .timestamp
            .format("%H:%M")
    );

    if center_on.is_some() {
        println!(
            "📁 {} 🗒️ {} ({}/{} msgs) ⏱️ {}",
            project_path,
            session_id,
            window.len(),
            total,
            time_range
        );
    } else {
        println!(
            "📁 {} 🗒️ {} ({} msgs) ⏱️ {}",
            project_path, session_id, total, time_range
        );
    }
    println!();

    for result in window {
        let time = result
            .timestamp
            .format("%H:%M:%S");
        let marker = if center_idx.is_some()
            && center_on
                .as_ref()
                .is_some_and(|u| {
                    result
                        .uuid
                        .starts_with(u.as_str())
                }) {
            "»"
        } else {
            " "
        };
        let content = if truncate_length > 0 {
            let truncated: String = result
                .content
                .chars()
                .take(truncate_length)
                .collect();
            let ellipsis = if result
                .content
                .chars()
                .count()
                > truncate_length
            {
                "…"
            } else {
                ""
            };
            format!(
                "{}{}",
                truncated
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                ellipsis
            )
        } else {
            result
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!("{marker} [{time}] {}: {content}", result.role_display());
    }

    Ok(())
}

fn summarize_session(index_path: &Path, session_id: String) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if !index_path.exists() {
        println!("Index not found. Please run 'agent-recall index' first.");
        return Ok(());
    }

    let cache = CacheManager::new(index_path)?;
    let search_engine = SearchEngine::new(
        index_path,
        cache
            .get_session_counts()
            .clone(),
    )?;
    let mut results = search_engine.get_session_messages(&session_id)?;

    if results.is_empty() {
        println!("No messages found for session: {session_id}");
        return Ok(());
    }

    // Sort and filter displayable
    results.sort_by_key(|r| r.sequence_num);
    let results: Vec<_> = results
        .into_iter()
        .filter(|r| r.is_displayable())
        .collect();

    // Build conversation text
    let mut conversation = String::new();
    for r in &results {
        let content: String = r
            .content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        conversation.push_str(&format!("{}: {}\n", r.role_display(), content));
    }

    // Create jail directory in temp dir (XDG_RUNTIME_DIR on Unix, %TEMP% on Windows)
    #[cfg(unix)]
    let temp_dir = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    #[cfg(windows)]
    let temp_dir = std::env::temp_dir();
    let jail_dir = temp_dir.join("claude-summary-jail");
    std::fs::create_dir_all(&jail_dir)?;

    let prompt = format!(
        "Summarize this conversation concisely. Include: topic, key decisions, outcome.\n\n{}",
        conversation
    );

    // Run claude --print in jailed directory with no tools, using haiku for cost
    let mut child = Command::new("claude")
        .args([
            "--print",
            "--tools",
            "",
            "--no-session-persistence",
            "--model",
            "haiku",
        ])
        .current_dir(&jail_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    if let Some(mut stdin) = child
        .stdin
        .take()
    {
        stdin.write_all(prompt.as_bytes())?;
    }

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("claude exited with status: {}", status);
    }

    Ok(())
}
