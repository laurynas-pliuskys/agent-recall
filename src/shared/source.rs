use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use super::models::ConversationEntry;
use anyhow::Result;

/// Identifies the conversation client/source
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Claude,
    #[serde(rename = "claude-web", alias = "claudeweb")]
    ClaudeWeb,
    Codex,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::ClaudeWeb => "claude-web",
            Source::Codex => "codex",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Source::Claude => "Claude Code",
            Source::ClaudeWeb => "Claude web export",
            Source::Codex => "Codex",
        }
    }

    pub fn resume_hint(&self, session_id: &str) -> String {
        match self {
            Source::Claude => format!("claude --resume {}", session_id),
            Source::ClaudeWeb => "Claude web export (no native resume command)".to_string(),
            Source::Codex => format!("codex resume {}", session_id),
        }
    }

    /// Construct a unique document key scoped by source
    pub fn doc_key(&self, session_id: &str, message_id: &str) -> String {
        format!("{}\0{}\0{}", self.as_str(), session_id, message_id)
    }

    /// Construct a unique session/conversation key scoped by source
    pub fn conversation_key(&self, session_id: &str) -> String {
        format!("{}\0{}", self.as_str(), session_id)
    }

    /// Construct a unique key for a source artifact such as a JSONL file.
    pub fn artifact_key(&self, artifact: &str) -> String {
        format!("{}\0{}", self.as_str(), artifact)
    }
}

/// Source-specific discovery and parsing boundary used by the shared cache and
/// indexing pipeline. Navigation remains separate from read-only ingestion.
pub trait ConversationSource: Sync {
    fn source(&self) -> Source;
    fn parser_version(&self) -> u32;
    fn discover(&self) -> Result<Vec<PathBuf>>;
    fn parse(&self, path: &Path, full_content: bool) -> Result<Vec<ConversationEntry>>;

    /// Whether a parsed record belongs in the primary conversation-search index.
    fn is_primary_index_record(&self, _entry: &ConversationEntry) -> bool {
        true
    }

    /// Whether a parsed record is source-backed technical evidence. References
    /// can be searched after selecting a conversation without entering Tantivy.
    fn is_reference_record(&self, _entry: &ConversationEntry) -> bool {
        false
    }
}

struct ClaudeSource;
struct ClaudeWebSource;
struct CodexSource;

impl ConversationSource for ClaudeSource {
    fn source(&self) -> Source {
        Source::Claude
    }

    fn parser_version(&self) -> u32 {
        1
    }

    fn discover(&self) -> Result<Vec<PathBuf>> {
        super::path_utils::discover_claude_jsonl_files()
    }

    fn parse(&self, path: &Path, full_content: bool) -> Result<Vec<ConversationEntry>> {
        let parser = if full_content {
            super::parser::JsonlParser::with_full_content()
        } else {
            super::parser::JsonlParser::default()
        };
        parser.parse_file(path)
    }
}

impl ConversationSource for ClaudeWebSource {
    fn source(&self) -> Source {
        Source::ClaudeWeb
    }

    fn parser_version(&self) -> u32 {
        // v3 parses durable per-conversation imports and preserves rich export
        // text/attachment labels instead of dropping empty `text` messages.
        3
    }

    fn discover(&self) -> Result<Vec<PathBuf>> {
        super::claude_web_import::discover_managed_exports()
    }

    fn parse(&self, path: &Path, _full_content: bool) -> Result<Vec<ConversationEntry>> {
        super::claude_export_parser::ClaudeExportParser::new().parse_file(path)
    }
}

impl ConversationSource for CodexSource {
    fn source(&self) -> Source {
        Source::Codex
    }

    fn parser_version(&self) -> u32 {
        // v3 keeps canonical tool records source-backed instead of indexing
        // their payloads in the primary conversation-search index.
        3
    }

    fn discover(&self) -> Result<Vec<PathBuf>> {
        super::path_utils::discover_codex_jsonl_files()
    }

    fn parse(&self, path: &Path, _full_content: bool) -> Result<Vec<ConversationEntry>> {
        // Codex preserves complete textual records in the source artifact;
        // adapter policy below decides which records enter the primary index.
        super::codex_parser::CodexParser::new().parse_file(path)
    }

    fn is_primary_index_record(&self, entry: &ConversationEntry) -> bool {
        entry.record_kind == super::models::RecordKind::Conversation
    }

    fn is_reference_record(&self, entry: &ConversationEntry) -> bool {
        matches!(
            entry.record_kind,
            super::models::RecordKind::ToolCall | super::models::RecordKind::ToolResult
        )
    }
}

static CLAUDE_SOURCE: ClaudeSource = ClaudeSource;
static CLAUDE_WEB_SOURCE: ClaudeWebSource = ClaudeWebSource;
static CODEX_SOURCE: CodexSource = CodexSource;

pub fn conversation_source(source: Source) -> &'static dyn ConversationSource {
    match source {
        Source::Claude => &CLAUDE_SOURCE,
        Source::ClaudeWeb => &CLAUDE_WEB_SOURCE,
        Source::Codex => &CODEX_SOURCE,
    }
}

pub fn conversation_sources() -> [&'static dyn ConversationSource; 3] {
    [&CLAUDE_SOURCE, &CLAUDE_WEB_SOURCE, &CODEX_SOURCE]
}

/// Read one source-qualified conversation from its source artifact. This
/// filter is required for legacy shared archives and is a no-op for managed
/// Claude web imports, which use one artifact per conversation.
pub fn read_conversation(
    source: Source,
    artifact: &Path,
    session_id: &str,
    full_content: bool,
) -> Result<Vec<ConversationEntry>> {
    Ok(conversation_source(source)
        .parse(artifact, full_content)?
        .into_iter()
        .filter(|entry| entry.session_id == session_id)
        .collect())
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for Source {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s
            .to_lowercase()
            .trim()
        {
            "claude" => Ok(Source::Claude),
            "claude-web" => Ok(Source::ClaudeWeb),
            "codex" => Ok(Source::Codex),
            other => Err(format!(
                "Unknown source: '{}'. Expected 'claude', 'claude-web', or 'codex'.",
                other
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_strings_and_parsing() {
        assert_eq!(Source::Claude.as_str(), "claude");
        assert_eq!(Source::ClaudeWeb.as_str(), "claude-web");
        assert_eq!(Source::Codex.as_str(), "codex");

        assert_eq!("claude".parse::<Source>(), Ok(Source::Claude));
        assert_eq!("claude-web".parse::<Source>(), Ok(Source::ClaudeWeb));
        assert_eq!("Codex".parse::<Source>(), Ok(Source::Codex));
        assert_eq!(
            serde_json::to_string(&Source::ClaudeWeb).unwrap(),
            "\"claude-web\""
        );
        assert_eq!(
            serde_json::from_str::<Source>("\"claudeweb\"").unwrap(),
            Source::ClaudeWeb
        );
        assert!(
            "unknown"
                .parse::<Source>()
                .is_err()
        );
    }

    #[test]
    fn test_resume_hints() {
        assert_eq!(
            Source::Claude.resume_hint("abc-123"),
            "claude --resume abc-123"
        );
        assert_eq!(
            Source::ClaudeWeb.resume_hint("abc-123"),
            "Claude web export (no native resume command)"
        );
        assert_eq!(Source::Codex.resume_hint("xyz-789"), "codex resume xyz-789");
    }

    #[test]
    fn test_source_keys() {
        assert_eq!(Source::Claude.conversation_key("sess1"), "claude\0sess1");
        assert_eq!(Source::Codex.doc_key("sess2", "msg1"), "codex\0sess2\0msg1");
        assert_eq!(
            Source::Codex.artifact_key("/tmp/session.jsonl"),
            "codex\0/tmp/session.jsonl"
        );
    }
}
