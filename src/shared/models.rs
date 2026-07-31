use super::source::Source;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Raw JSONL message structure for parsing Claude Code logs
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RawJsonlMessage {
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
    pub session_id: Option<String>,
    #[serde(rename = "type")]
    pub message_type: Option<String>,
    pub timestamp: Option<String>,
    pub cwd: Option<String>,
    pub message: Option<RawMessage>,
    pub is_sidechain: Option<bool>,
    pub agent_id: Option<String>,
    // Summary type fields
    pub summary: Option<String>,
    pub leaf_uuid: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RawMessage {
    pub role: Option<String>,
    pub content: Option<serde_json::Value>,
    pub model: Option<String>,
}

/// Content block types in assistant messages
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(String),
    Thinking(String),
    ToolUse {
        name: String,
        input_preview: String,
    },
    ToolResult {
        content_preview: String,
        is_error: bool,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConversationEntry {
    #[serde(default)]
    pub source: Source,
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub session_id: String,
    pub project_path: String,
    pub timestamp: DateTime<Utc>,
    pub message_type: MessageType,
    pub content: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub sequence_num: usize,
    pub is_sidechain: bool,
    pub agent_id: Option<String>,

    // Enhanced metadata for better search and categorization
    pub technologies: Vec<String>,
    pub has_code: bool,
    pub code_languages: Vec<String>,
    pub has_error: bool,
    pub tools_mentioned: Vec<String>,
}

impl ConversationEntry {
    pub fn is_displayable(&self) -> bool {
        matches!(
            self.message_type,
            MessageType::User | MessageType::Assistant | MessageType::Summary
        ) && self
            .content
            .trim()
            != "Warmup"
    }

    pub fn project_path_display(&self) -> String {
        super::path_utils::home_to_tilde(&self.project_path)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum MessageType {
    User,
    Assistant,
    Summary,
    System,
}

impl MessageType {
    /// Short display name for output (User, AI, Sum, Sys)
    pub fn short_name(&self) -> &'static str {
        match self {
            MessageType::User => "User",
            MessageType::Assistant => "AI",
            MessageType::Summary => "Sum",
            MessageType::System => "Sys",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum SortOrder {
    #[default]
    Relevance,
    DateDesc,
    DateAsc,
}

#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub text: String,
    pub source_filter: Option<Source>,
    pub project_filter: Option<String>,
    pub session_filter: Option<String>,
    pub limit: usize,
    pub sort_by: SortOrder,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub source: Source,
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub content: String,
    pub project: String,
    pub project_path: String,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub score: f32,
    pub snippet: String,
    pub technologies: Vec<String>,
    pub code_languages: Vec<String>,
    pub tools_mentioned: Vec<String>,
    pub has_code: bool,
    pub has_error: bool,
    pub interaction_count: usize,
    pub sequence_num: usize,
    pub is_sidechain: bool,
    pub agent_id: Option<String>,
    pub message_type: String,
}

impl SearchResult {
    /// Check if message should be displayed (filters noise like Warmup, tool_result dumps)
    pub fn is_displayable(&self) -> bool {
        // Filter by message type
        if !matches!(
            self.message_type
                .as_str(),
            "User" | "Assistant" | "Summary"
        ) {
            return false;
        }
        // Filter internal warmup messages
        if self
            .content
            .trim()
            == "Warmup"
        {
            return false;
        }
        true
    }

    /// Get project path with ~ for home directory
    pub fn project_path_display(&self) -> String {
        super::path_utils::home_to_tilde(&self.project_path)
    }

    /// Short display name for message type (User, AI, Sum, Sys)
    pub fn role_display(&self) -> &'static str {
        match self
            .message_type
            .as_str()
        {
            "User" => "User",
            "Assistant" => "AI",
            "Summary" => "Sum",
            _ => "?",
        }
    }
}
