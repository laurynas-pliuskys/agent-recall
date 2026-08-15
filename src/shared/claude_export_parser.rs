use super::models::{ConversationEntry, MessageType, RecordKind};
use super::source::Source;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use tracing::warn;

/// Parser for the `conversations.json` archive exported by Claude's web client.
/// The export is one JSON array containing many conversations, unlike the
/// per-session JSONL artifacts produced by Claude Code.
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
    text: String,
    sender: String,
    created_at: String,
    #[serde(default)]
    parent_message_uuid: Option<String>,
}

impl ClaudeExportParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse_file(&self, path: &Path) -> Result<Vec<ConversationEntry>> {
        let conversations: Vec<ExportConversation> =
            serde_json::from_reader(BufReader::new(File::open(path)?))?;
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
                if message
                    .text
                    .trim()
                    .is_empty()
                {
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
                    content: message.text,
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
