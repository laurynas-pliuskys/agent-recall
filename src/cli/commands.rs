use crate::cli::index;
use crate::shared::{self, CacheManager, DisplayOptions, SearchEngine, SearchQuery, SortOrder};
use anyhow::{Context, Result};
use chrono::{NaiveDate, TimeZone, Utc};
use clap::{Subcommand, ValueEnum};
use regex::Regex;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

#[derive(Subcommand)]
pub enum CliCommands {
    /// Import a one-off conversation archive into agent-recall managed storage
    Import {
        #[command(subcommand)]
        source: ImportSource,
    },
    /// Index management
    Index {
        #[command(subcommand)]
        action: Option<IndexAction>,
    },
    /// Search conversations (auto-indexes if needed)
    Search {
        /// Search query
        query: String,
        /// Filter by source client (claude, claude-web, or codex)
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
    /// Search source-backed technical references within one conversation
    References {
        /// Session ID selected from primary conversation search
        session_id: String,
        /// Technical query to find in tool calls/results
        query: String,
        /// Source client (Codex references are supported now; Claude is deferred)
        #[arg(long, default_value = "codex")]
        source: String,
        /// Results limit
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Characters shown per reference (0 = full content)
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
    /// Register with Claude Code and/or Codex MCP
    Install {
        /// MCP client to configure
        #[arg(long, value_enum, default_value_t = InstallClient::All)]
        client: InstallClient,
        /// Use project scope for Claude Code; Codex configuration remains global
        #[arg(long)]
        project: bool,
    },
}

#[derive(Subcommand)]
pub enum ImportSource {
    /// Import a downloaded Claude web/desktop `conversations.json` export
    ClaudeWeb {
        /// Path to the downloaded `conversations.json` file
        path: PathBuf,
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

#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallClient {
    #[default]
    All,
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstallTarget {
    Claude,
    Codex,
}

impl InstallTarget {
    fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    fn executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct InstallCommand {
    program: &'static str,
    args: Vec<OsString>,
}

#[derive(Debug, PartialEq, Eq)]
struct ClientInstallPlan {
    target: InstallTarget,
    commands: Vec<InstallCommand>,
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
    Rebuild {
        /// Acknowledge that deleted native source artifacts cannot be restored
        #[arg(long)]
        allow_history_loss: bool,
    },
    /// Clean up deleted entries from index
    Vacuum {
        /// Acknowledge that deleted native source artifacts cannot be restored
        #[arg(long)]
        allow_history_loss: bool,
    },
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
        CliCommands::Import { source } => match source {
            ImportSource::ClaudeWeb { path } => {
                let imported = shared::import_claude_web_export(&path)?;
                let index_path = shared::get_config().get_cache_dir()?;
                shared::index_now_forced(&index_path, imported.clone())?;
                println!(
                    "Imported {} Claude web conversations into managed storage and indexed them.",
                    imported.len()
                );
            }
        },
        CliCommands::Index { action } => {
            let config = shared::get_config();
            let index_path = config.get_cache_dir()?;
            match action.unwrap_or_default() {
                IndexAction::Status => index::show_status(&index_path)?,
                IndexAction::Rebuild { allow_history_loss } => {
                    index::rebuild(&index_path, allow_history_loss)?
                }
                IndexAction::Vacuum { allow_history_loss } => {
                    index::vacuum(&index_path, allow_history_loss)?
                }
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
        CliCommands::References {
            session_id,
            query,
            source,
            limit,
            truncate,
        } => {
            let config = shared::get_config();
            let index_path = config.get_cache_dir()?;
            shared::auto_index(&index_path)?;
            search_references(
                &index_path,
                source
                    .parse::<shared::Source>()
                    .map_err(|error| anyhow::anyhow!(error))?,
                &session_id,
                &query,
                limit,
                truncate,
            )?;
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
        CliCommands::Install { client, project } => install(client, project)?,
    }

    Ok(())
}

fn install(client: InstallClient, project_scope: bool) -> Result<()> {
    use std::process::Command;

    let exe = std::env::current_exe()?;
    let plans = install_plan(client, project_scope, &exe)?;
    if client == InstallClient::All && project_scope {
        println!("Codex MCP configuration is global; --project applies only to Claude Code.");
    }

    let mut installed = 0;
    for plan in plans {
        match run_install_plan(&plan, |command| {
            Command::new(command.program)
                .args(&command.args)
                .status()
        })? {
            InstallAttempt::Installed => {
                installed += 1;
                match plan.target {
                    InstallTarget::Claude => {
                        let scope = if project_scope { "project" } else { "user" };
                        println!("Registered agent-recall with Claude Code ({scope} scope).");
                    }
                    InstallTarget::Codex => {
                        println!("Registered agent-recall with Codex (global scope).");
                    }
                }
            }
            InstallAttempt::MissingExecutable if client == InstallClient::All => {
                println!(
                    "Skipping {}: '{}' was not found on PATH.",
                    plan.target
                        .display_name(),
                    plan.target
                        .executable()
                );
            }
            InstallAttempt::MissingExecutable => {
                anyhow::bail!(
                    "{} executable '{}' was not found on PATH",
                    plan.target
                        .display_name(),
                    plan.target
                        .executable()
                );
            }
        }
    }

    if installed == 0 {
        anyhow::bail!(
            "No supported MCP client executable was found on PATH (looked for 'claude' and 'codex')"
        );
    }

    println!("agent-recall MCP command: {} mcp", exe.display());
    Ok(())
}

enum InstallAttempt {
    Installed,
    MissingExecutable,
}

fn install_plan(
    client: InstallClient,
    project_scope: bool,
    exe: &Path,
) -> Result<Vec<ClientInstallPlan>> {
    if client == InstallClient::Codex && project_scope {
        anyhow::bail!(
            "--project is only supported for Claude Code; Codex MCP configuration is global"
        );
    }

    let targets: &[InstallTarget] = match client {
        InstallClient::All => &[InstallTarget::Claude, InstallTarget::Codex],
        InstallClient::Claude => &[InstallTarget::Claude],
        InstallClient::Codex => &[InstallTarget::Codex],
    };
    let scope = if project_scope { "project" } else { "user" };

    Ok(targets
        .iter()
        .map(|target| {
            let commands = match target {
                InstallTarget::Claude => vec![
                    InstallCommand {
                        program: target.executable(),
                        args: vec![
                            "mcp".into(),
                            "remove".into(),
                            "-s".into(),
                            scope.into(),
                            "agent-recall".into(),
                        ],
                    },
                    InstallCommand {
                        program: target.executable(),
                        args: vec![
                            "mcp".into(),
                            "add".into(),
                            "-s".into(),
                            scope.into(),
                            "agent-recall".into(),
                            "--".into(),
                            exe.as_os_str()
                                .to_os_string(),
                            "mcp".into(),
                        ],
                    },
                ],
                InstallTarget::Codex => vec![
                    InstallCommand {
                        program: target.executable(),
                        args: vec!["mcp".into(), "remove".into(), "agent-recall".into()],
                    },
                    InstallCommand {
                        program: target.executable(),
                        args: vec![
                            "mcp".into(),
                            "add".into(),
                            "agent-recall".into(),
                            "--".into(),
                            exe.as_os_str()
                                .to_os_string(),
                            "mcp".into(),
                        ],
                    },
                ],
            };
            ClientInstallPlan {
                target: *target,
                commands,
            }
        })
        .collect())
}

fn run_install_plan<F>(plan: &ClientInstallPlan, mut run: F) -> Result<InstallAttempt>
where
    F: FnMut(&InstallCommand) -> std::io::Result<std::process::ExitStatus>,
{
    let remove = &plan.commands[0];
    match run(remove) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InstallAttempt::MissingExecutable);
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to run {} MCP removal command",
                    plan.target
                        .display_name()
                )
            });
        }
    }

    let add = &plan.commands[1];
    let status = match run(add) {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InstallAttempt::MissingExecutable);
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to run {} MCP registration command",
                    plan.target
                        .display_name()
                )
            });
        }
    };
    if !status.success() {
        anyhow::bail!(
            "{} MCP registration command failed with {status}",
            plan.target
                .display_name()
        );
    }
    Ok(InstallAttempt::Installed)
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
    let mut cache_manager = CacheManager::new_for_destructive_reset(index_path)?;
    cache_manager.clear_cache()?;
    println!(
        "Cache cleared. Retained indexed history was permanently discarded; run 'agent-recall index' to rebuild from available sources."
    );
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

fn search_references(
    index_path: &Path,
    source: shared::Source,
    session_id: &str,
    query: &str,
    limit: usize,
    truncate_length: usize,
) -> Result<()> {
    if source != shared::Source::Codex {
        anyhow::bail!(
            "Source-backed reference search is currently enabled only for Codex; Claude tool evidence remains in the primary index"
        );
    }
    if !index_path.exists() {
        anyhow::bail!("Index not found. Run 'agent-recall index rebuild' first");
    }

    let cache = CacheManager::new(index_path)?;
    let search_engine = SearchEngine::new(
        index_path,
        cache
            .get_session_counts()
            .clone(),
    )?;
    let artifact = shared::conversation_artifact(&search_engine, source, session_id)?
        .ok_or_else(|| anyhow::anyhow!("Codex conversation '{}' was not found", session_id))?;
    if !artifact.exists() {
        anyhow::bail!("Source artifact is unavailable: {}", artifact.display());
    }

    let matches =
        shared::search_conversation_references(source, &artifact, session_id, query, limit)?;
    println!(
        "{}",
        shared::format_reference_matches(source, session_id, query, &matches, truncate_length,)
    );
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

    let conversation_stats =
        search_engine.aggregate_conversation_stats(project_filter.as_deref())?;

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

    println!("Conversation Analysis:");
    println!(
        "  💬 Total messages indexed: {}",
        conversation_stats.total_messages
    );
    println!(
        "  🏗️ Unique sessions: {}",
        conversation_stats.session_count()
    );
    println!(
        "  📝 Messages with code: {} ({:.1}%)",
        conversation_stats.code_messages,
        percentage(
            conversation_stats.code_messages,
            conversation_stats.total_messages
        )
    );
    println!(
        "  🚨 Messages with errors: {} ({:.1}%)",
        conversation_stats.error_messages,
        percentage(
            conversation_stats.error_messages,
            conversation_stats.total_messages
        )
    );
    println!(
        "  💬 Total turns: {} (avg: {} per conversation)",
        conversation_stats.total_turns,
        conversation_stats.average_turns_per_session()
    );

    // Show most active sessions
    if !conversation_stats
        .conversation_message_counts
        .is_empty()
    {
        println!();
        println!("Most Active Sessions:");
        let mut sorted_sessions: Vec<_> = conversation_stats
            .conversation_message_counts
            .iter()
            .filter_map(|(key, count)| {
                let (source, session_id) = key.split_once('\0')?;
                let source = source
                    .parse::<shared::Source>()
                    .ok()?;
                Some(((source, session_id), *count))
            })
            .collect();
        sorted_sessions.sort_by(|a, b| {
            b.1.cmp(&a.1)
        });

        for ((source, session_id), count) in sorted_sessions
            .iter()
            .take(5)
        {
            let short_id = if session_id.len() > 12 {
                format!("{}…", &session_id[..12])
            } else {
                session_id.to_string()
            };
            println!("  {source}:{short_id} ({count} messages)");
        }
    }

    Ok(())
}

fn percentage(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64 * 100.0
    }
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
            shared::read_conversation(first.source, &first.source_artifact, &session_id, true)?
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

#[cfg(test)]
mod install_tests {
    use super::*;

    fn rendered(plan: &[ClientInstallPlan]) -> Vec<(&'static str, Vec<Vec<String>>)> {
        plan.iter()
            .map(|client| {
                (
                    client
                        .target
                        .executable(),
                    client
                        .commands
                        .iter()
                        .map(|command| {
                            command
                                .args
                                .iter()
                                .map(|arg| {
                                    arg.to_string_lossy()
                                        .into_owned()
                                })
                                .collect()
                        })
                        .collect(),
                )
            })
            .collect()
    }

    fn args(values: &[&str]) -> Vec<String> {
        values
            .iter()
            .map(|value| (*value).to_string())
            .collect()
    }

    #[test]
    fn all_clients_plan_uses_explicit_mcp_commands() {
        let plan = install_plan(InstallClient::All, false, Path::new("/tmp/agent-recall")).unwrap();

        assert_eq!(
            rendered(&plan),
            vec![
                (
                    "claude",
                    vec![
                        args(&["mcp", "remove", "-s", "user", "agent-recall"]),
                        args(&[
                            "mcp",
                            "add",
                            "-s",
                            "user",
                            "agent-recall",
                            "--",
                            "/tmp/agent-recall",
                            "mcp",
                        ]),
                    ],
                ),
                (
                    "codex",
                    vec![
                        args(&["mcp", "remove", "agent-recall"]),
                        args(&[
                            "mcp",
                            "add",
                            "agent-recall",
                            "--",
                            "/tmp/agent-recall",
                            "mcp",
                        ]),
                    ],
                ),
            ]
        );
    }

    #[test]
    fn project_scope_applies_only_to_claude() {
        let claude =
            install_plan(InstallClient::Claude, true, Path::new("/tmp/agent-recall")).unwrap();
        assert_eq!(
            rendered(&claude)[0].1[0],
            args(&["mcp", "remove", "-s", "project", "agent-recall"])
        );

        let error =
            install_plan(InstallClient::Codex, true, Path::new("/tmp/agent-recall")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("only supported for Claude Code")
        );
    }

    #[test]
    fn missing_client_executable_is_distinguished_without_running_configuration() {
        let plan =
            install_plan(InstallClient::Codex, false, Path::new("/tmp/agent-recall")).unwrap();
        let attempt = run_install_plan(&plan[0], |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not installed",
            ))
        })
        .unwrap();
        assert!(matches!(attempt, InstallAttempt::MissingExecutable));
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[test]
    fn percentage_returns_zero_for_an_empty_scope() {
        assert_eq!(percentage(0, 0), 0.0);
        assert_eq!(percentage(1, 4), 25.0);
    }
}
