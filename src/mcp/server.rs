use anyhow::Result;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tracing::{debug, error, info};

use crate::shared::path_utils::{discover_jsonl_files, globally_active_session_jsonl};
use crate::shared::{
    CacheManager, DisplayOptions, SearchEngine, SearchQuery, SortOrder, auto_index, get_cache_dir,
    get_config, short_uuid,
};

const HAIKU_CONTEXT_WINDOW: usize = 200_000;
const CONTEXT_SAFETY_MARGIN: f64 = 0.75;

/// Extract Vec<String> from JSON array value
fn json_strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse date string: YYYY-MM-DD (as start of day UTC) or full ISO 8601
fn parse_date(s: &str) -> Result<DateTime<Utc>, String> {
    // Try full ISO 8601 first
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Try YYYY-MM-DD
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(Utc.from_utc_datetime(
            &date
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        ));
    }
    Err(format!("Invalid date '{}': use YYYY-MM-DD or ISO 8601", s))
}

// MCP Protocol Structures
#[derive(Debug, Default)]
enum RequestId {
    #[default]
    Missing,
    Present(Value),
}

fn deserialize_request_id<'de, D>(deserializer: D) -> Result<RequestId, D::Error>
where
    D: Deserializer<'de>,
{
    Value::deserialize(deserializer).map(RequestId::Present)
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(rename = "jsonrpc")]
    _jsonrpc: String,
    #[serde(default, deserialize_with = "deserialize_request_id")]
    id: RequestId,
    method: String,
    params: Option<Value>,
}

impl JsonRpcRequest {
    fn is_notification(&self) -> bool {
        matches!(self.id, RequestId::Missing)
    }

    fn response_id(&self) -> Option<Value> {
        match &self.id {
            RequestId::Missing => None,
            RequestId::Present(id) => Some(id.clone()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InitializeResponse {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    server_info: ServerInfo,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServerCapabilities {
    #[serde(default)]
    experimental: HashMap<String, Value>,
    #[serde(default)]
    logging: HashMap<String, Value>,
    #[serde(default)]
    prompts: HashMap<String, Value>,
    #[serde(default)]
    resources: HashMap<String, Value>,
    tools: HashMap<String, Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServerInfo {
    name: String,
    version: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListToolsResponse {
    tools: Vec<Tool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Tool {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct CallToolRequest {
    name: String,
    arguments: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CallToolResponse {
    pub content: Vec<ToolResult>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolResult {
    #[serde(rename = "type")]
    pub result_type: String,
    pub text: String,
}

pub struct McpServer {
    search_engine: SearchEngine,
    cache_dir: std::path::PathBuf,
}

impl McpServer {
    pub fn new() -> Result<Self> {
        let cache_dir = get_cache_dir()?;

        // Auto-index if needed
        auto_index(&cache_dir)?;

        let cache = CacheManager::new(&cache_dir)?;
        let counts = cache
            .get_session_counts()
            .clone();
        let search_engine = SearchEngine::new(&cache_dir, counts)?;

        Ok(Self {
            search_engine,
            cache_dir,
        })
    }

    #[cfg(test)]
    fn new_with_dir(cache_dir: std::path::PathBuf) -> Result<Self> {
        if !cache_dir
            .join("meta.json")
            .exists()
        {
            let _indexer = crate::shared::SearchIndexer::new(&cache_dir)?;
        } else {
            auto_index(&cache_dir)?;
        }

        let cache = CacheManager::new(&cache_dir)?;
        let counts = cache
            .get_session_counts()
            .clone();
        let search_engine = SearchEngine::new(&cache_dir, counts)?;

        Ok(Self {
            search_engine,
            cache_dir,
        })
    }

    /// Check if a source artifact is stale and reindex if needed.
    /// Returns true if reindexing occurred.
    fn ensure_artifact_fresh(&mut self, source_artifact: &std::path::Path) -> Result<bool> {
        if !source_artifact.exists() {
            return Ok(false);
        }

        let cache = CacheManager::new(&self.cache_dir)?;
        if !cache.needs_indexing(source_artifact)? {
            return Ok(false);
        }

        info!("Reindexing stale artifact {}", source_artifact.display());

        // Reindex just this file
        let mut indexer = crate::shared::SearchIndexer::open(&self.cache_dir)?;
        let mut cache = CacheManager::new(&self.cache_dir)?;
        cache.update_incremental(&mut indexer, vec![source_artifact.to_path_buf()])?;

        // Reload search engine
        let counts = cache
            .get_session_counts()
            .clone();
        self.search_engine = SearchEngine::new(&self.cache_dir, counts)?;

        Ok(true)
    }

    async fn handle_initialize(&self, params: Option<Value>) -> Result<Value> {
        debug!("Handling initialize request: {:?}", params);

        let response = InitializeResponse {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ServerCapabilities {
                experimental: HashMap::new(),
                logging: HashMap::new(),
                prompts: HashMap::new(),
                resources: HashMap::new(),
                tools: {
                    let mut tools = HashMap::new();
                    tools.insert("listChanged".to_string(), Value::Bool(true));
                    tools
                },
            },
            server_info: ServerInfo {
                name: "agent-recall".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        Ok(serde_json::to_value(response)?)
    }

    async fn handle_list_tools(&self) -> Result<Value> {
        debug!("Handling list_tools request");

        let tools = vec![
            Tool {
                name: "search_conversations".to_string(),
                description: "Search conversation history (Tantivy/BM25). Exact terms for functions (`_fix_ssh_agent`), natural language for concepts. Workflow: search → get_messages(ids)/truncate_length:0 for full text → summarize_session for AI summary.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query. Field syntax: 'session_id:abc', 'project:name'"
                        },
                        "source": {
                            "type": "string",
                            "enum": ["claude", "codex"],
                            "description": "Filter by conversation source client (claude or codex)",
                            "optional": true
                        },
                        "project": {
                            "type": "string",
                            "description": "Filter by project name",
                            "optional": true
                        },
                        "session": {
                            "type": "string",
                            "description": "Filter by session ID (prefix match)",
                            "optional": true
                        },
                        "-C": {
                            "type": "integer",
                            "description": "Messages before and after match (like grep -C)",
                            "optional": true,
                            "default": 2
                        },
                        "-B": {
                            "type": "integer",
                            "description": "Messages before match (like grep -B)",
                            "optional": true
                        },
                        "-A": {
                            "type": "integer",
                            "description": "Messages after match (like grep -A)",
                            "optional": true
                        },
                        "exclude_projects": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Project names to exclude",
                            "optional": true
                        },
                        "exclude_patterns": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Regex patterns to exclude",
                            "optional": true
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max results",
                            "optional": true,
                            "default": 10
                        },
                        "sort_by": {
                            "type": "string",
                            "enum": ["relevance", "date_desc", "date_asc"],
                            "optional": true,
                            "default": "relevance"
                        },
                        "after": {
                            "type": "string",
                            "description": "Results after date (YYYY-MM-DD or ISO 8601)",
                            "optional": true
                        },
                        "before": {
                            "type": "string",
                            "description": "Results before date (YYYY-MM-DD or ISO 8601)",
                            "optional": true
                        },
                        "include": {
                            "type": "array",
                            "items": { "type": "string", "enum": ["thinking", "tools", "current_session"] },
                            "description": "Include: thinking, tools, current_session",
                            "optional": true
                        },
                        "truncate_length": {
                            "type": "integer",
                            "description": "Chars shown per message around match. 0 = full content",
                            "optional": true,
                            "default": 300
                        },
                        "debug": {
                            "type": "boolean",
                            "optional": true
                        }
                    },
                    "required": ["query"]
                }),
            },
            Tool {
                name: "reindex".to_string(),
                description: "Update index for stale/new files. Use when search results seem incomplete or index warning shown.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "full": { "type": "boolean", "description": "Force full rebuild (default: incremental)", "optional": true }
                    }
                }),
            },
            Tool {
                name: "get_session_messages".to_string(),
                description: "Paginate session messages. Use offset/limit for sequential reading, or center_on with -B/-A/-C to jump to a specific message.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Session ID to retrieve messages for"
                        },
                        "source": {
                            "type": "string",
                            "enum": ["claude", "codex"],
                            "description": "Source for an otherwise ambiguous session ID",
                            "optional": true
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Starting message index",
                            "optional": true,
                            "default": 0
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Messages per page",
                            "optional": true,
                            "default": 20
                        },
                        "center_on": {
                            "type": "string",
                            "description": "Message UUID to center around (from 💬 in search). Overrides offset/limit.",
                            "optional": true
                        },
                        "-C": {
                            "type": "integer",
                            "description": "Messages before and after center_on (like grep -C)",
                            "optional": true,
                            "default": 10
                        },
                        "-B": {
                            "type": "integer",
                            "description": "Messages before center_on (like grep -B)",
                            "optional": true
                        },
                        "-A": {
                            "type": "integer",
                            "description": "Messages after center_on (like grep -A)",
                            "optional": true
                        },
                        "truncate_length": {
                            "type": "integer",
                            "description": "Chars shown per message. 0 = full content",
                            "optional": true,
                            "default": 500
                        },
                        "include": {
                            "type": "array",
                            "items": { "type": "string", "enum": ["tools"] },
                            "description": "Include neighboring tool calls/results; a centered tool record is always included",
                            "optional": true
                        }
                    },
                    "required": ["session_id"]
                }),
            },
            Tool {
                name: "summarize_session".to_string(),
                description: "Get Task tool instructions to summarize a session with haiku. Use for long sessions when you need an AI-generated overview.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Session ID to summarize"
                        },
                        "source": {
                            "type": "string",
                            "enum": ["claude", "codex"],
                            "description": "Source for an otherwise ambiguous session ID",
                            "optional": true
                        }
                    },
                    "required": ["session_id"]
                }),
            },
            Tool {
                name: "get_messages".to_string(),
                description: "Get full content of specific records by message ID. Pass source and session_id from the search result for exact source-qualified retrieval.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Message UUIDs (from 💬 in search results)"
                        },
                        "source": {
                            "type": "string",
                            "enum": ["claude", "codex"],
                            "description": "Source for otherwise ambiguous message IDs",
                            "optional": true
                        },
                        "session_id": {
                            "type": "string",
                            "description": "Session ID for exact source-qualified retrieval; requires source",
                            "optional": true
                        }
                    },
                    "required": ["ids"]
                }),
            },
            Tool {
                name: "respawn_server".to_string(),
                description: "Respawn the MCP server to reload with latest changes".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ];

        let response = ListToolsResponse { tools };
        Ok(serde_json::to_value(response)?)
    }

    async fn handle_call_tool(&mut self, params: Value) -> Result<Value> {
        let request: CallToolRequest = serde_json::from_value(params)?;
        debug!("Handling tool call: {}", request.name);

        let result = match request
            .name
            .as_str()
        {
            "search_conversations" => {
                self.tool_search_conversations(request.arguments)
                    .await?
            }
            "respawn_server" => {
                self.tool_respawn()
                    .await?
            }
            "reindex" => {
                self.tool_reindex(request.arguments)
                    .await?
            }
            "get_session_messages" => {
                self.tool_get_session_messages(request.arguments)
                    .await?
            }
            "summarize_session" => {
                self.tool_summarize_session(request.arguments)
                    .await?
            }
            "get_messages" => {
                self.tool_get_messages(request.arguments)
                    .await?
            }
            _ => {
                return Ok(serde_json::to_value(CallToolResponse {
                    content: vec![ToolResult {
                        result_type: "text".to_string(),
                        text: format!("Unknown tool: {}", request.name),
                    }],
                    is_error: Some(true),
                })?);
            }
        };

        Ok(result)
    }

    async fn tool_search_conversations(&self, args: Option<Value>) -> Result<Value> {
        let args = args.unwrap_or_default();
        let query_text = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?
            .to_string();

        let debug_mode = args
            .get("debug")
            .and_then(|v| v.as_str())
            .map(|s| s == "true")
            .unwrap_or(false);

        let project_filter = args
            .get("project")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let session_filter = args
            .get("session")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Parse grep-style context: -C (both), -B (before), -A (after)
        let context_c = args
            .get("-C")
            .and_then(|v| v.as_u64())
            .unwrap_or(2);
        let context_before = args
            .get("-B")
            .and_then(|v| v.as_u64())
            .unwrap_or(context_c) as usize;
        let context_after = args
            .get("-A")
            .and_then(|v| v.as_u64())
            .unwrap_or(context_c) as usize;

        let exclude_projects = json_strings(args.get("exclude_projects"));

        let exclude_patterns: Vec<String> = args
            .get("exclude_patterns")
            .map(|v| {
                if let Some(arr) = v.as_array() {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_string())
                        .collect()
                } else if let Some(s) = v.as_str() {
                    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default();

        let config = get_config();
        let all_files = discover_jsonl_files()?;

        // Exclude the active session from stale checks (it is always being written to).
        // Use globally_active_session_jsonl since the MCP server cwd is fixed at startup
        // and does not reflect the currently active project.
        let current_session_file = globally_active_session_jsonl();
        let current_session_file_ref = current_session_file.as_deref();
        let files_for_stale_check: Vec<_> = all_files
            .iter()
            .filter(|f| Some(f.as_path()) != current_session_file_ref)
            .cloned()
            .collect();

        let cache = CacheManager::new(&config.get_cache_dir()?)?;
        let (stale_count, new_count) = cache.quick_health_check(&files_for_stale_check);

        let mut all_exclude_patterns = config
            .search
            .exclude_patterns
            .clone();
        all_exclude_patterns.extend(exclude_patterns.clone());

        let exclude_regexes: Vec<Regex> = all_exclude_patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let sort_by = match args
            .get("sort_by")
            .and_then(|v| v.as_str())
            .unwrap_or("relevance")
        {
            "date_desc" => SortOrder::DateDesc,
            "date_asc" => SortOrder::DateAsc,
            _ => SortOrder::Relevance,
        };

        let after = if let Some(s) = args
            .get("after")
            .and_then(|v| v.as_str())
        {
            match parse_date(s) {
                Ok(dt) => Some(dt),
                Err(e) => {
                    return Ok(serde_json::to_value(CallToolResponse {
                        content: vec![ToolResult {
                            result_type: "text".to_string(),
                            text: e,
                        }],
                        is_error: Some(true),
                    })?);
                }
            }
        } else {
            None
        };

        let before = if let Some(s) = args
            .get("before")
            .and_then(|v| v.as_str())
        {
            match parse_date(s) {
                Ok(dt) => Some(dt),
                Err(e) => {
                    return Ok(serde_json::to_value(CallToolResponse {
                        content: vec![ToolResult {
                            result_type: "text".to_string(),
                            text: e,
                        }],
                        is_error: Some(true),
                    })?);
                }
            }
        } else {
            None
        };

        // Parse include parameter
        let include = json_strings(args.get("include"));

        let truncate_length = args
            .get("truncate_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(300) as usize;

        let display_opts = DisplayOptions {
            include_thinking: include.contains(&"thinking".to_string()),
            include_tools: include.contains(&"tools".to_string()),
            truncate_length,
        };

        let include_current_session = include.contains(&"current_session".to_string());

        // Get current session ID from file detected earlier
        let current_session_id: Option<String> = if !include_current_session {
            current_session_file
                .as_ref()
                .and_then(|p| {
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
        } else {
            None
        };

        let source_filter = args
            .get("source")
            .and_then(|v| v.as_str())
            .map(|s| s.parse::<crate::shared::Source>())
            .transpose()
            .map_err(|e| anyhow::anyhow!(e))?;

        let query = SearchQuery {
            text: query_text,
            source_filter,
            project_filter,
            session_filter,
            limit: limit * 3,
            sort_by,
            after,
            before,
        };

        let search_engine = &self.search_engine;
        let results_with_context = search_engine.search_with_context_options(
            query,
            context_before,
            context_after,
            display_opts.include_tools,
        )?;

        // Filter and deduplicate
        let mut session_seen = std::collections::HashSet::new();
        let filtered: Vec<_> = results_with_context
            .into_iter()
            .filter(|r| {
                let proj = &r
                    .matched_message
                    .project;
                let path = &r
                    .matched_message
                    .project_path;
                let session = &r
                    .matched_message
                    .session_id;

                // Exclude current session unless explicitly included
                if let Some(ref current) = current_session_id
                    && session == current
                {
                    return false;
                }

                if exclude_projects.contains(proj) {
                    return false;
                }
                for regex in &exclude_regexes {
                    if regex.is_match(proj) || regex.is_match(path) {
                        return false;
                    }
                }
                // Deduplicate by session
                session_seen.insert(
                    r.matched_message
                        .source
                        .conversation_key(session),
                )
            })
            .take(limit)
            .collect();

        let mut output = String::new();

        if debug_mode {
            output.push_str(&format!(
                "DEBUG: query={:?}, -B={}, -A={}, limit={}, exclude_projects={:?}, patterns={:?}\n\n",
                args.get("query"),
                context_before,
                context_after,
                limit,
                exclude_projects,
                all_exclude_patterns
            ));
        }

        if !exclude_projects.is_empty() || !all_exclude_patterns.is_empty() {
            output.push_str(&format!(
                "Excluding: {} projects, {} patterns\n",
                exclude_projects.len(),
                all_exclude_patterns.len()
            ));
        }

        if stale_count > 1 || new_count > 0 {
            output.push_str(&format!(
                "Note: index is stale ({} modified, {} new files). Call reindex tool for fresher results.\n",
                stale_count, new_count
            ));
        }

        if filtered.is_empty() {
            output.push_str("No results found.\n");
        } else {
            for (i, result) in filtered
                .iter()
                .enumerate()
            {
                output.push_str(&result.format_compact_with_options(i, &display_opts));
                if i < filtered.len() - 1 {
                    output.push('\n');
                }
            }
            if filtered.len() == limit {
                output.push_str(&format!("\n+more: limit={}\n", limit));
            }
        }

        Ok(serde_json::to_value(CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: output,
            }],
            is_error: None,
        })?)
    }

    async fn tool_get_session_messages(&mut self, args: Option<Value>) -> Result<Value> {
        let args = args.unwrap_or_default();
        let session_id = args
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'session_id' parameter"))?;
        let source = args
            .get("source")
            .and_then(|value| value.as_str())
            .map(str::parse::<crate::shared::Source>)
            .transpose()
            .map_err(|error| anyhow::anyhow!(error))?;
        let indexed_messages = if let Some(source) = source {
            self.search_engine
                .get_conversation_messages(source, session_id)?
        } else {
            self.search_engine
                .get_session_messages(session_id)?
        };

        let artifact = indexed_messages
            .first()
            .and_then(|first| {
                (!first
                    .source_artifact
                    .as_os_str()
                    .is_empty())
                .then(|| {
                    (
                        first.source,
                        first
                            .source_artifact
                            .clone(),
                    )
                })
            });

        if let Some((artifact_source, artifact_path)) = artifact
            && artifact_path.exists()
        {
            self.ensure_artifact_fresh(&artifact_path)?;

            // Read the source artifact directly for full-fidelity content.
            let entries =
                crate::shared::conversation_source(artifact_source).parse(&artifact_path, true)?;
            return self.format_session_entries(entries, session_id, &args);
        };

        self.format_session_from_index(indexed_messages, session_id, &args)
    }

    fn format_session_entries(
        &self,
        entries: Vec<crate::shared::ConversationEntry>,
        session_id: &str,
        args: &Value,
    ) -> Result<Value> {
        if entries.is_empty() {
            return Ok(serde_json::to_value(CallToolResponse {
                content: vec![ToolResult {
                    result_type: "text".to_string(),
                    text: format!("No messages found for session {}", session_id),
                }],
                is_error: Some(true),
            })?);
        }

        let all_messages: Vec<_> = entries
            .into_iter()
            .filter(|e| e.is_displayable())
            .collect();
        let center_on = args
            .get("center_on")
            .and_then(|value| value.as_str());
        let include_tools = json_strings(args.get("include")).contains(&"tools".to_string());
        let messages: Vec<_> = all_messages
            .into_iter()
            .filter(|message| {
                include_tools
                    || !message.is_tool_record()
                    || center_on.is_some_and(|uuid| {
                        message
                            .uuid
                            .starts_with(uuid)
                    })
            })
            .collect();

        let total = messages.len();
        let project = messages
            .first()
            .map(|m| m.project_path_display())
            .unwrap_or_default();
        let short_session = short_uuid(session_id);
        let truncate_length = args
            .get("truncate_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(500) as usize;

        let (start, end, center_idx) = if let Some(uuid) = center_on {
            let matching: Vec<_> = messages
                .iter()
                .enumerate()
                .filter_map(|(index, message)| {
                    message
                        .uuid
                        .starts_with(uuid)
                        .then_some(index)
                })
                .collect();
            let idx = match matching.as_slice() {
                [index] => *index,
                [] => anyhow::bail!("Message ID '{}' was not found in this session", uuid),
                _ => anyhow::bail!(
                    "Ambiguous message ID prefix '{}'; use the complete ID from search",
                    uuid
                ),
            };
            let context_c = args
                .get("-C")
                .and_then(|v| v.as_u64())
                .unwrap_or(10);
            let before = args
                .get("-B")
                .and_then(|v| v.as_u64())
                .unwrap_or(context_c) as usize;
            let after = args
                .get("-A")
                .and_then(|v| v.as_u64())
                .unwrap_or(context_c) as usize;
            let start = idx.saturating_sub(before);
            let end = (idx + after + 1).min(total);
            (start, end, Some(idx))
        } else {
            let offset = args
                .get("offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(20) as usize;
            let start = offset.min(total);
            let end = (offset + limit).min(total);
            (start, end, None)
        };

        let page_messages = &messages[start..end];
        let has_more = end < total;

        let mut output = format!(
            "📁 {} 🗒️ {} ({} msgs) [{}-{}/{}]\n\n",
            project,
            short_session,
            total,
            start,
            end.saturating_sub(1),
            total
        );

        for (i, msg) in page_messages
            .iter()
            .enumerate()
        {
            let idx = start + i;
            let time = msg
                .timestamp
                .format("%H:%M");
            let msg_type = msg
                .message_type
                .short_name();
            let marker = if center_idx == Some(idx) { "»" } else { " " };
            let content = if truncate_length > 0 {
                let truncated: String = msg
                    .content
                    .chars()
                    .take(truncate_length)
                    .collect();
                let ellipsis = if msg
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
                msg.content
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            output.push_str(&format!(
                "{}[{}] {} {}: {}\n",
                marker, idx, time, msg_type, content
            ));
        }

        if has_more {
            output.push_str(&format!("\n+more: offset={}\n", end));
        }

        Ok(serde_json::to_value(CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: output,
            }],
            is_error: None,
        })?)
    }

    /// Fallback: format session from Tantivy SearchResult objects (pre-truncated content)
    fn format_session_from_index(
        &self,
        mut messages: Vec<crate::shared::SearchResult>,
        session_id: &str,
        args: &Value,
    ) -> Result<Value> {
        if messages.is_empty() {
            return Ok(serde_json::to_value(CallToolResponse {
                content: vec![ToolResult {
                    result_type: "text".to_string(),
                    text: format!("No messages found for session {}", session_id),
                }],
                is_error: Some(true),
            })?);
        }

        messages.sort_by_key(|m| m.sequence_num);
        let center_on = args
            .get("center_on")
            .and_then(|value| value.as_str());
        let include_tools = json_strings(args.get("include")).contains(&"tools".to_string());
        let messages: Vec<_> = messages
            .into_iter()
            .filter(|message| {
                message.is_displayable()
                    && (include_tools
                        || !message.is_tool_record()
                        || center_on.is_some_and(|uuid| {
                            message
                                .uuid
                                .starts_with(uuid)
                        }))
            })
            .collect();

        let total = messages.len();
        let project = messages
            .first()
            .map(|m| m.project_path_display())
            .unwrap_or_default();
        let short_session = short_uuid(session_id);
        let truncate_length = args
            .get("truncate_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(500) as usize;

        let (start, end, center_idx) = if let Some(uuid) = center_on {
            let matching: Vec<_> = messages
                .iter()
                .enumerate()
                .filter_map(|(index, message)| {
                    message
                        .uuid
                        .starts_with(uuid)
                        .then_some(index)
                })
                .collect();
            let idx = match matching.as_slice() {
                [index] => *index,
                [] => anyhow::bail!("Message ID '{}' was not found in this session", uuid),
                _ => anyhow::bail!(
                    "Ambiguous message ID prefix '{}'; use the complete ID from search",
                    uuid
                ),
            };
            let context_c = args
                .get("-C")
                .and_then(|v| v.as_u64())
                .unwrap_or(10);
            let before = args
                .get("-B")
                .and_then(|v| v.as_u64())
                .unwrap_or(context_c) as usize;
            let after = args
                .get("-A")
                .and_then(|v| v.as_u64())
                .unwrap_or(context_c) as usize;
            let start = idx.saturating_sub(before);
            let end = (idx + after + 1).min(total);
            (start, end, Some(idx))
        } else {
            let offset = args
                .get("offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(20) as usize;
            let start = offset.min(total);
            let end = (offset + limit).min(total);
            (start, end, None)
        };

        let page_messages = &messages[start..end];
        let has_more = end < total;

        let mut output = format!(
            "📁 {} 🗒️ {} ({} msgs, from index) [{}-{}/{}]\n\n",
            project,
            short_session,
            total,
            start,
            end.saturating_sub(1),
            total
        );

        for (i, msg) in page_messages
            .iter()
            .enumerate()
        {
            let idx = start + i;
            let time = msg
                .timestamp
                .format("%H:%M");
            let msg_type = msg.role_display();
            let marker = if center_idx == Some(idx) { "»" } else { " " };
            let content = if truncate_length > 0 {
                let truncated: String = msg
                    .content
                    .chars()
                    .take(truncate_length)
                    .collect();
                let ellipsis = if msg
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
                msg.content
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            output.push_str(&format!(
                "{}[{}] {} {}: {}\n",
                marker, idx, time, msg_type, content
            ));
        }

        if has_more {
            output.push_str(&format!("\n+more: offset={}\n", end));
        }

        Ok(serde_json::to_value(CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: output,
            }],
            is_error: None,
        })?)
    }

    async fn tool_summarize_session(&self, args: Option<Value>) -> Result<Value> {
        let args = args.unwrap_or_default();
        let session_id = args
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'session_id' parameter"))?;
        let source = args
            .get("source")
            .and_then(|value| value.as_str())
            .map(str::parse::<crate::shared::Source>)
            .transpose()
            .map_err(|error| anyhow::anyhow!(error))?;

        // Get session stats for size estimation
        let search_engine = &self.search_engine;
        let messages = if let Some(source) = source {
            search_engine.get_conversation_messages(source, session_id)?
        } else {
            search_engine.get_session_messages(session_id)?
        };
        let msg_count = messages.len();
        let total_chars: usize = messages
            .iter()
            .map(|m| {
                m.content
                    .len()
            })
            .sum();
        let approx_tokens = total_chars / 4; // rough estimate: ~4 chars per token

        let safe_limit = (HAIKU_CONTEXT_WINDOW as f64 * CONTEXT_SAFETY_MARGIN) as usize;
        let size_note = if approx_tokens > safe_limit {
            " (large - may need multiple agents)"
        } else {
            ""
        };

        let source_arg = source
            .map(|source| format!(", source=\"{}\"", source.as_str()))
            .unwrap_or_default();
        let output = format!(
            r#"Session {session_id}: {msg_count} messages, ~{approx_tokens} tokens{size_note}

Task(
  subagent_type: "general-purpose",
  model: "haiku",
  prompt: "Summarize session {session_id}:
1. Call get_session_messages(session_id=\"{session_id}\"{source_arg})
2. If output ends with '+more: offset=N', call again with that offset
3. Repeat until no '+more' appears
4. Return a concise summary: topic, key decisions, outcome"
)"#
        );

        Ok(serde_json::to_value(CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: output,
            }],
            is_error: None,
        })?)
    }

    async fn tool_get_messages(&self, args: Option<Value>) -> Result<Value> {
        let args = args.unwrap_or_default();
        let ids = json_strings(args.get("ids"));
        let source = args
            .get("source")
            .and_then(|value| value.as_str())
            .map(str::parse::<crate::shared::Source>)
            .transpose()
            .map_err(|error| anyhow::anyhow!(error))?;
        let session_id = args
            .get("session_id")
            .and_then(Value::as_str);
        if session_id.is_some() && source.is_none() {
            anyhow::bail!("'session_id' requires 'source' for exact message retrieval");
        }

        if ids.is_empty() {
            return Ok(serde_json::to_value(CallToolResponse {
                content: vec![ToolResult {
                    result_type: "text".to_string(),
                    text: "No message IDs provided".to_string(),
                }],
                is_error: Some(true),
            })?);
        }

        let search_engine = &self.search_engine;
        let messages = match (source, session_id) {
            (Some(source), Some(session_id)) => {
                search_engine.get_messages_by_uuid_for_conversation(source, session_id, &ids)?
            }
            (Some(source), None) => search_engine.get_messages_by_uuid_for_source(source, &ids)?,
            (None, None) => search_engine.get_messages_by_uuid(&ids)?,
            (None, Some(_)) => unreachable!("validated above"),
        };

        if messages.is_empty() {
            return Ok(serde_json::to_value(CallToolResponse {
                content: vec![ToolResult {
                    result_type: "text".to_string(),
                    text: "No messages found for provided IDs".to_string(),
                }],
                is_error: None,
            })?);
        }

        let mut output = String::new();
        for msg in &messages {
            output.push_str(&format!(
                "💬 {} 📅 {} [{}:{}]\n{}\n\n",
                short_uuid(&msg.uuid),
                msg.timestamp
                    .format("%Y-%m-%d %H:%M"),
                msg.source,
                msg.message_type,
                msg.content
            ));
        }

        Ok(serde_json::to_value(CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: output,
            }],
            is_error: None,
        })?)
    }

    #[cfg(unix)]
    async fn tool_respawn(&self) -> Result<Value> {
        // Try to find the release binary first, fallback to current_exe
        let current_dir = std::env::current_dir()
            .map_err(|e| anyhow::anyhow!("Failed to get current directory: {}", e))?;

        let release_path = current_dir.join("target/release/agent-recall");
        let exe_path = if release_path.exists() {
            release_path
        } else {
            std::env::current_exe()
                .map_err(|e| anyhow::anyhow!("Failed to get current executable path: {}", e))?
        };

        // Prepare response
        let response = CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: "Respawning MCP server...".to_string(),
            }],
            is_error: None,
        };

        // Schedule respawn after a short delay to allow response to be sent
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Replace current process with new instance using exec
            let args: Vec<String> = std::env::args().collect();
            let err = exec::execvp(&exe_path, &args);
            eprintln!("Failed to exec with {}: {}", exe_path.display(), err);
        });

        Ok(serde_json::to_value(response)?)
    }

    #[cfg(windows)]
    async fn tool_respawn(&self) -> Result<Value> {
        Ok(serde_json::to_value(CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: "respawn_server is not supported on Windows".to_string(),
            }],
            is_error: Some(true),
        })?)
    }

    async fn tool_reindex(&mut self, args: Option<Value>) -> Result<Value> {
        let args = args.unwrap_or_default();
        let full_rebuild = args
            .get("full")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let all_files = discover_jsonl_files()?;

        let result = if full_rebuild {
            // Full rebuild - clear and recreate
            if self
                .cache_dir
                .exists()
            {
                std::fs::remove_dir_all(&self.cache_dir)?;
            }
            let mut indexer = crate::shared::SearchIndexer::new(&self.cache_dir)?;
            let mut cache = crate::shared::CacheManager::new(&self.cache_dir)?;
            cache.remove_missing_files(&mut indexer, &all_files)?;
            cache.update_incremental(&mut indexer, all_files)?;
            let counts = cache
                .get_session_counts()
                .clone();
            self.search_engine = crate::shared::SearchEngine::new(&self.cache_dir, counts)?;
            "Full rebuild complete".to_string()
        } else {
            // Incremental update
            let mut indexer = crate::shared::SearchIndexer::open(&self.cache_dir)?;
            let mut cache = crate::shared::CacheManager::new(&self.cache_dir)?;
            let (stale, new) = cache.quick_health_check(&all_files);
            cache.remove_missing_files(&mut indexer, &all_files)?;
            cache.update_incremental(&mut indexer, all_files)?;
            let counts = cache
                .get_session_counts()
                .clone();
            self.search_engine = crate::shared::SearchEngine::new(&self.cache_dir, counts)?;
            format!(
                "Incremental update: {} stale + {} new files reindexed",
                stale, new
            )
        };
        Ok(serde_json::to_value(CallToolResponse {
            content: vec![ToolResult {
                result_type: "text".to_string(),
                text: result,
            }],
            is_error: None,
        })?)
    }

    async fn handle_request(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let is_notification = request.is_notification();
        let response_id = request.response_id();

        let (result, error_code) = match request
            .method
            .as_str()
        {
            "initialize" => (
                self.handle_initialize(request.params)
                    .await,
                -32603,
            ),
            "tools/list" => (
                self.handle_list_tools()
                    .await,
                -32603,
            ),
            "tools/call" => (
                self.handle_call_tool(
                    request
                        .params
                        .unwrap_or_default(),
                )
                .await,
                -32603,
            ),
            _ => (
                Err(anyhow::anyhow!("Unknown method: {}", request.method)),
                -32601,
            ),
        };

        if is_notification {
            return None;
        }

        let response = match result {
            Ok(result) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: response_id,
                result: Some(result),
                error: None,
            },
            Err(e) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: error_code,
                    message: e.to_string(),
                    data: None,
                }),
            },
        };

        Some(response)
    }
}

async fn process_line(server: &mut McpServer, line: &str) -> Result<Option<String>> {
    match serde_json::from_str::<JsonRpcRequest>(line) {
        Ok(request) => server
            .handle_request(request)
            .await
            .map(|response| serde_json::to_string(&response))
            .transpose()
            .map_err(Into::into),
        Err(e) => {
            error!("Failed to parse JSON-RPC request: {}", e);
            let error_response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: None,
                result: None,
                error: Some(JsonRpcError {
                    code: -32700,
                    message: format!("Parse error: {e}"),
                    data: None,
                }),
            };
            Ok(Some(serde_json::to_string(&error_response)?))
        }
    }
}

pub async fn run_mcp_server() -> Result<()> {
    // Initialize logging to stderr so it doesn't interfere with JSON-RPC
    // Only show CRITICAL/ERROR level logs to avoid JSON parsing issues
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("error")
        .init();

    let mut server = McpServer::new()?;
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = AsyncBufReader::new(stdin).lines();

    while let Some(line) = reader
        .next_line()
        .await?
    {
        if line
            .trim()
            .is_empty()
        {
            continue;
        }

        debug!("Received line: {}", line);

        if let Some(response_json) = process_line(&mut server, &line).await? {
            debug!("Sending response: {}", response_json);

            stdout
                .write_all(response_json.as_bytes())
                .await?;
            stdout
                .write_all(b"\n")
                .await?;
            stdout
                .flush()
                .await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jsonrpc_request_notification_detection() {
        let request_json = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();
        assert!(request.is_notification());
        assert_eq!(request.method, "notifications/initialized");

        let req_with_id_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let req_with_id: JsonRpcRequest = serde_json::from_str(req_with_id_json).unwrap();
        assert!(!req_with_id.is_notification());

        let req_with_null_id_json = r#"{"jsonrpc":"2.0","id":null,"method":"initialize"}"#;
        let req_with_null_id: JsonRpcRequest = serde_json::from_str(req_with_null_id_json).unwrap();
        assert!(!req_with_null_id.is_notification());
        assert_eq!(req_with_null_id.response_id(), Some(Value::Null));
    }

    #[tokio::test]
    async fn test_handle_request_notification_returns_none() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir
            .path()
            .join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();

        let mut server = McpServer::new_with_dir(cache_dir).unwrap();

        let notification: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();

        let response = server
            .handle_request(notification)
            .await;
        assert!(
            response.is_none(),
            "Notifications MUST NOT receive a response"
        );

        let req_with_id: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":42,"method":"initialize"}"#).unwrap();

        let response = server
            .handle_request(req_with_id)
            .await;
        assert!(
            response.is_some(),
            "Requests with ID must return a response"
        );
        let resp = response.unwrap();
        assert_eq!(resp.id, Some(serde_json::json!(42)));
        assert!(
            resp.error
                .is_none()
        );
        assert!(
            resp.result
                .is_some()
        );

        let req_with_null_id: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":null,"method":"initialize"}"#).unwrap();
        let response = server
            .handle_request(req_with_null_id)
            .await
            .unwrap();
        assert_eq!(response.id, Some(Value::Null));
        assert!(
            response
                .result
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_handle_request_unknown_method_with_id() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir
            .path()
            .join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();

        let mut server = McpServer::new_with_dir(cache_dir).unwrap();

        let req: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":99,"method":"non_existent_method"}"#)
                .unwrap();

        let response = server
            .handle_request(req)
            .await;
        assert!(response.is_some());
        let resp = response.unwrap();
        assert_eq!(resp.id, Some(serde_json::json!(99)));
        assert!(
            resp.error
                .is_some()
        );
        assert_eq!(
            resp.error
                .as_ref()
                .unwrap()
                .code,
            -32601
        );
        assert_eq!(
            resp.error
                .unwrap()
                .message,
            "Unknown method: non_existent_method"
        );
    }

    #[tokio::test]
    async fn test_notification_shaped_method_with_id_is_not_a_notification() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir
            .path()
            .join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let mut server = McpServer::new_with_dir(cache_dir).unwrap();

        for method in ["notifications/not-a-notification", "$/not-a-notification"] {
            let request: JsonRpcRequest = serde_json::from_value(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": method
            }))
            .unwrap();
            let response = server
                .handle_request(request)
                .await
                .unwrap();
            assert_eq!(
                response
                    .error
                    .unwrap()
                    .code,
                -32601
            );
        }
    }

    #[tokio::test]
    async fn test_process_line_suppresses_only_notification_response() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir
            .path()
            .join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let mut server = McpServer::new_with_dir(cache_dir).unwrap();

        let lines = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ];
        let mut responses = Vec::new();
        for line in lines {
            if let Some(response) = process_line(&mut server, line)
                .await
                .unwrap()
            {
                responses.push(serde_json::from_str::<Value>(&response).unwrap());
            }
        }

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[1]["id"], 2);
    }

    #[tokio::test]
    async fn test_process_line_returns_parse_error_for_malformed_json() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir
            .path()
            .join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let mut server = McpServer::new_with_dir(cache_dir).unwrap();

        let response = process_line(&mut server, "{")
            .await
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], Value::Null);
        assert_eq!(response["error"]["code"], -32700);
    }
}
