use super::metadata;
use super::models::{ConversationEntry, MessageType};
use super::source::Source;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::Path;

pub struct CodexParser {
    #[allow(dead_code)]
    full_content: bool,
}

impl Default for CodexParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexParser {
    pub fn new() -> Self {
        Self {
            full_content: false,
        }
    }

    pub fn with_full_content() -> Self {
        Self { full_content: true }
    }

    pub fn parse_file(&self, path: &Path) -> Result<Vec<ConversationEntry>> {
        let content = std::fs::read_to_string(path)?;
        let mut entries = Vec::new();
        let mut session_id = String::new();
        let mut project_path = String::new();
        let mut sequence_counter = 0;

        for line in content.lines() {
            let line_trimmed = line.trim();
            if line_trimmed.is_empty() {
                continue;
            }

            let value: Value = match serde_json::from_str(line_trimmed) {
                Ok(v) => v,
                Err(_) => {
                    // Gracefully skip invalid/partial JSON line (e.g. incomplete trailing line)
                    continue;
                }
            };

            let entry_type = value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // 1. Session metadata line
            if entry_type == "session_meta" {
                if let Some(payload) = value.get("payload") {
                    if let Some(sid) = payload
                        .get("session_id")
                        .and_then(|v| v.as_str())
                    {
                        session_id = sid.to_string();
                    } else if let Some(id) = payload
                        .get("id")
                        .and_then(|v| v.as_str())
                    {
                        session_id = id.to_string();
                    }
                    if let Some(cwd) = payload
                        .get("cwd")
                        .and_then(|v| v.as_str())
                    {
                        project_path = cwd.to_string();
                    }
                }
                continue;
            }

            // Fallback session ID if missing in meta
            if session_id.is_empty() {
                session_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
            }

            // 2. Conversational message line
            if (entry_type == "response_item" || entry_type == "user_message")
                && let Some(payload) = value.get("payload")
            {
                let role_str = payload
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if role_str == "developer" || role_str == "system" {
                    // Skip system instructions
                    continue;
                }

                let message_type = match role_str {
                    "user" => MessageType::User,
                    "assistant" => MessageType::Assistant,
                    _ => continue,
                };

                let timestamp_str = value
                    .get("timestamp")
                    .or_else(|| payload.get("timestamp"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let timestamp = DateTime::parse_from_rfc3339(timestamp_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                // Extract content text
                let mut text_parts = Vec::new();
                if let Some(content_arr) = payload
                    .get("content")
                    .and_then(|v| v.as_array())
                {
                    for item in content_arr {
                        if let Some(text) = item
                            .get("text")
                            .and_then(|v| v.as_str())
                            && !text.contains("<environment_context>")
                            && !text.contains("<permissions_context>")
                            && !text.contains("<workspace_context>")
                        {
                            text_parts.push(text);
                        }
                    }
                } else if let Some(text) = payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    && !text.contains("<environment_context>")
                {
                    text_parts.push(text);
                }

                let text_combined = text_parts
                    .join("\n")
                    .trim()
                    .to_string();
                if text_combined.is_empty() {
                    continue;
                }

                let item_id = payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let uuid = if !item_id.is_empty() {
                    item_id
                } else {
                    format!("{}-{}", session_id, sequence_counter)
                };

                let (technologies, tools_mentioned, code_languages, has_code, has_error) =
                    metadata::extract_all_metadata(&text_combined);

                entries.push(ConversationEntry {
                    source: Source::Codex,
                    uuid,
                    parent_uuid: None,
                    session_id: session_id.clone(),
                    project_path: project_path.clone(),
                    timestamp,
                    message_type,
                    content: text_combined,
                    model: payload
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    cwd: if !project_path.is_empty() {
                        Some(project_path.clone())
                    } else {
                        None
                    },
                    sequence_num: sequence_counter,
                    is_sidechain: false,
                    agent_id: None,
                    technologies,
                    has_code,
                    code_languages,
                    has_error,
                    tools_mentioned,
                });

                sequence_counter += 1;
            }
        }

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_sample_codex_rollout() {
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/codex/sample_rollout.jsonl");

        let parser = CodexParser::new();
        let entries = parser
            .parse_file(&fixture_path)
            .expect("Failed to parse fixture");

        assert_eq!(entries.len(), 2, "Expected 2 entries (1 user, 1 assistant)");
        assert_eq!(entries[0].source, Source::Codex);
        assert_eq!(
            entries[0].session_id,
            "019faf9d-7ce8-70b0-be6b-3ceacc3500e2"
        );
        assert_eq!(entries[0].project_path, "/home/user/projects/auth-app");
        assert_eq!(entries[0].message_type, MessageType::User);
        assert!(
            entries[0]
                .content
                .contains("OAuth2 authentication")
        );

        assert_eq!(entries[1].source, Source::Codex);
        assert_eq!(entries[1].message_type, MessageType::Assistant);
        assert!(
            entries[1]
                .content
                .contains("oauth2 crate")
        );
    }
}
