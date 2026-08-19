use super::models::{ConversationEntry, MessageType, RecordKind};
use super::source::Source;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use tracing::warn;

/// Parser for the `conversations.json` archive exported by Claude's web client.
/// The export is one JSON array containing many conversations, unlike the
/// per-session JSONL artifacts produced by Claude Code.
#[derive(Default)]
pub struct ClaudeExportParser;

#[derive(Debug, Deserialize)]
struct ExportConversation {
    uuid: String,
    name: Option<String>,
    #[serde(default)]
    chat_messages: Vec<ExportMessage>,
}

#[derive(Debug, Deserialize)]
struct ExportMessage {
    uuid: String,
    #[serde(default)]
    text: String,
    sender: String,
    created_at: String,
    #[serde(default)]
    parent_message_uuid: Option<String>,
    /// Claude exports use these fields for rich content that has no `text`
    /// mirror (for example a document attachment or content-block array).
    /// Keep them as JSON so the parser can deliberately select only
    /// human-visible fields instead of serializing arbitrary export payloads.
    #[serde(default)]
    content: Value,
    #[serde(default)]
    files: Value,
    #[serde(default)]
    attachments: Value,
    #[serde(default)]
    hidden_in_chat: bool,
    #[serde(default)]
    is_hidden: bool,
}

impl ClaudeExportParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse_file(&self, path: &Path) -> Result<Vec<ConversationEntry>> {
        let conversations = read_export_conversations(path)?;
        let mut entries = Vec::new();

        for conversation in conversations {
            let title = conversation
                .name
                .filter(|name| {
                    !name
                        .trim()
                        .is_empty()
                })
                .unwrap_or_else(|| "Untitled Claude conversation".to_string());

            for (sequence_num, message) in conversation
                .chat_messages
                .into_iter()
                .enumerate()
            {
                if message.hidden_in_chat || message.is_hidden {
                    continue;
                }
                let message_type = match message
                    .sender
                    .as_str()
                {
                    "human" => MessageType::User,
                    "assistant" => MessageType::Assistant,
                    other => {
                        warn!(
                            "Skipping Claude export message {} with unknown sender {other}",
                            message.uuid
                        );
                        continue;
                    }
                };
                let content = normalized_message_content(&message);
                if content.is_empty() {
                    continue;
                }
                let timestamp = match DateTime::parse_from_rfc3339(&message.created_at) {
                    Ok(timestamp) => timestamp.with_timezone(&Utc),
                    Err(error) => {
                        warn!(
                            "Skipping Claude export message {} with invalid timestamp: {error}",
                            message.uuid
                        );
                        continue;
                    }
                };

                entries.push(ConversationEntry {
                    source: Source::ClaudeWeb,
                    uuid: message.uuid,
                    parent_uuid: message.parent_message_uuid,
                    session_id: conversation
                        .uuid
                        .clone(),
                    source_artifact: path.to_path_buf(),
                    project_path: title.clone(),
                    timestamp,
                    message_type,
                    record_kind: RecordKind::Conversation,
                    content,
                    model: None,
                    // SearchResult historically displays `cwd` as the project
                    // path. A Claude web export has no cwd, so retain its
                    // conversation title in both supported metadata fields.
                    cwd: Some(title.clone()),
                    sequence_num,
                    is_sidechain: false,
                    agent_id: None,
                    technologies: vec![],
                    has_code: false,
                    code_languages: vec![],
                    has_error: false,
                    tools_mentioned: vec![],
                });
            }
        }
        Ok(entries)
    }
}

/// Split a one-off Claude web export into one managed source artifact per
/// native conversation. The files retain the original conversation JSON, so
/// a later parser upgrade can re-normalize it without needing the user's
/// Downloads copy. Existing conversation IDs are atomically replaced; IDs
/// absent from a later import are intentionally retained as history.
pub fn import_export_file(input: &Path, destination: &Path) -> Result<Vec<PathBuf>> {
    let raw_conversations = read_export_values(input)?;
    let mut staged = Vec::with_capacity(raw_conversations.len());

    for raw in raw_conversations {
        // Validate every conversation before creating the managed directory or
        // writing any artifacts. Deserializing by reference avoids cloning the
        // complete JSON tree while preserving the parser's structural checks.
        let conversation = ExportConversation::deserialize(&raw)?;
        if conversation
            .uuid
            .trim()
            .is_empty()
        {
            anyhow::bail!("Claude export conversation has an empty uuid");
        }
        let filename = format!(
            "conversation-{}.claude-web.json",
            safe_file_component(&conversation.uuid)
        );
        staged.push((filename, serde_json::to_vec_pretty(&raw)?));
    }

    super::claude_web_import::ensure_private_directory(destination)?;
    let mut imported = Vec::with_capacity(staged.len());
    for (filename, content) in staged {
        let output = destination.join(&filename);
        let temporary = destination.join(format!(".import-{}.tmp", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&content)?;
        file.sync_all()?;
        // On Unix this replaces the prior conversation atomically. On Windows
        // a managed import is still safe, but rename cannot replace a target.
        #[cfg(windows)]
        if output.exists() {
            std::fs::remove_file(&output)?;
        }
        std::fs::rename(temporary, &output)?;
        imported.push(output);
    }
    Ok(imported)
}

fn read_export_conversations(path: &Path) -> Result<Vec<ExportConversation>> {
    read_export_values(path)?
        .into_iter()
        .map(serde_json::from_value)
        .collect::<serde_json::Result<_>>()
        .map_err(Into::into)
}

fn read_export_values(path: &Path) -> Result<Vec<Value>> {
    let raw: Value = serde_json::from_reader(BufReader::new(File::open(path)?))?;
    match raw {
        Value::Array(conversations) => Ok(conversations),
        Value::Object(_) => Ok(vec![raw]),
        _ => anyhow::bail!("Claude export must be a conversation object or array"),
    }
}

fn safe_file_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => (byte as char).to_string(),
            _ => format!("_{byte:02x}"),
        })
        .collect()
}

/// Normalization is intentionally conservative: retain text users can see and
/// concise attachment labels, but never stringify opaque tool/internal JSON.
fn normalized_message_content(message: &ExportMessage) -> String {
    let mut parts = Vec::new();
    push_nonempty(&mut parts, &message.text);
    extract_visible_content(&message.content, &mut parts, 0);
    extract_attachment_labels("Attached file", &message.files, &mut parts);
    extract_attachment_labels("Attachment", &message.attachments, &mut parts);

    let mut seen = HashSet::new();
    parts
        .into_iter()
        .filter(|part| seen.insert(part.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_nonempty(parts: &mut Vec<String>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }
}

fn extract_visible_content(value: &Value, parts: &mut Vec<String>, depth: usize) {
    if depth > 4 {
        return;
    }
    match value {
        Value::String(text) => push_nonempty(parts, text),
        Value::Array(values) => {
            for value in values {
                extract_visible_content(value, parts, depth + 1);
            }
        }
        Value::Object(object) => {
            if object
                .get("hidden_in_chat")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || object
                    .get("is_hidden")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                return;
            }
            match object
                .get("type")
                .and_then(Value::as_str)
            {
                Some("text" | "input_text" | "output_text") | None => {
                    if let Some(Value::String(text)) = object.get("text") {
                        push_nonempty(parts, text);
                    }
                }
                // Do not recurse into `content`: real tool_result blocks can
                // contain very large opaque payloads. The raw managed export
                // remains available for future, source-scoped handling.
                _ => {}
            }
        }
        _ => {}
    }
}

fn extract_attachment_labels(kind: &str, value: &Value, parts: &mut Vec<String>) {
    let values: Vec<&Value> = match value {
        Value::Array(values) => values
            .iter()
            .collect(),
        Value::Null => Vec::new(),
        value => vec![value],
    };
    for value in values {
        let Value::Object(object) = value else {
            continue;
        };
        let label = [
            "name",
            "file_name",
            "filename",
            "fileName",
            "title",
            "display_name",
        ]
        .iter()
        .find_map(|field| {
            object
                .get(*field)
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|label| !label.is_empty());
        let mime = ["mime_type", "mimeType", "content_type"]
            .iter()
            .find_map(|field| {
                object
                    .get(*field)
                    .and_then(Value::as_str)
            })
            .map(str::trim)
            .filter(|mime| !mime.is_empty());
        if let Some(label) = label {
            let suffix = mime
                .map(|mime| format!(" ({mime})"))
                .unwrap_or_default();
            parts.push(format!("[{kind}: {label}{suffix}]"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_each_conversation_with_native_ids_and_titles() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude_export/conversations.json");
        let entries = ClaudeExportParser::new()
            .parse_file(&path)
            .unwrap();

        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .all(|entry| entry.source == Source::ClaudeWeb)
        );
        assert_eq!(entries[0].session_id, "conversation-a");
        assert_eq!(entries[0].project_path, "First conversation");
        assert_eq!(
            entries[0]
                .cwd
                .as_deref(),
            Some("First conversation")
        );
        assert_eq!(entries[1].message_type, MessageType::Assistant);
        assert_eq!(entries[2].session_id, "conversation-b");
        assert_eq!(entries[2].project_path, "Untitled Claude conversation");
        assert_eq!(
            entries[2].content,
            "A separate topic from structured content.\n[Attached file: design.md (text/markdown)]\n[Attachment: wireframe.png (image/png)]"
        );
        assert!(
            !entries[2]
                .content
                .contains("SECRET_TOOL_PAYLOAD")
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.uuid != "message-b2")
        );
    }

    #[test]
    fn reads_only_the_requested_conversation_from_a_shared_export() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude_export/conversations.json");
        let entries = super::super::source::read_conversation(
            Source::ClaudeWeb,
            &path,
            "conversation-b",
            true,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert!(
            entries
                .iter()
                .all(|entry| entry.session_id == "conversation-b")
        );
    }

    #[test]
    fn import_rejects_malformed_conversations_before_writing_artifacts() {
        let temporary = TempDir::new().unwrap();
        let source = temporary
            .path()
            .join("export.json");
        let managed = temporary
            .path()
            .join("managed");
        std::fs::write(
            &source,
            r#"[{"uuid":"valid-conv","chat_messages":[]},{"uuid":"bad-conv","chat_messages":"not-an-array"}]"#,
        )
        .unwrap();

        let error = import_export_file(&source, &managed)
            .unwrap_err()
            .to_string();

        assert!(error.contains("expected a sequence"));
        assert!(!managed.exists());
    }
}
