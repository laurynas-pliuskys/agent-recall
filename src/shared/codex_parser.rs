use super::metadata;
use super::models::{ConversationEntry, MessageType, RecordKind};
use super::source::Source;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use strip_ansi_escapes::strip_str;

/// Parser for the rollout JSONL written by the currently installed Codex app.
///
/// The source artifact remains the full-fidelity record. Parsing preserves
/// textual tool payloads as explicitly typed references, while the Codex source
/// adapter keeps those references out of the primary Tantivy index.
pub struct CodexParser;

impl Default for CodexParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexParser {
    pub fn new() -> Self {
        Self
    }

    pub fn with_full_content() -> Self {
        Self
    }

    fn is_mirrored_approval_context(text: &str) -> bool {
        const MARKERS: &[&str] = &[
            "The following is the Codex agent history whose request action you are assessing.",
            "The following is the Codex agent history added since your last approval assessment.",
            ">>> TRANSCRIPT START",
            ">>> TRANSCRIPT DELTA START",
        ];
        MARKERS
            .iter()
            .any(|marker| text.contains(marker))
    }

    fn is_injected_context(text: &str) -> bool {
        const MARKERS: &[&str] = &[
            "<environment_context>",
            "<permissions_context>",
            "<workspace_context>",
            "<permissions instructions>",
            "<app-context>",
            "<recommended_plugins>",
            "<collaboration_mode>",
            "<apps_instructions>",
            "<plugins_instructions>",
            "<skills_instructions>",
        ];
        MARKERS
            .iter()
            .any(|marker| text.contains(marker))
            || Self::is_mirrored_approval_context(text)
    }

    fn timestamp(
        value: &Value,
        payload: &Value,
        session_timestamp: Option<DateTime<Utc>>,
        path: &Path,
    ) -> Result<DateTime<Utc>> {
        value
            .get("timestamp")
            .or_else(|| payload.get("timestamp"))
            .and_then(Value::as_str)
            .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
            .map(|timestamp| timestamp.with_timezone(&Utc))
            .or(session_timestamp)
            .with_context(|| format!("record without a valid timestamp in {}", path.display()))
    }

    fn value_text(value: &Value) -> Option<String> {
        match value {
            Value::Null => None,
            Value::String(text) => Some(text.clone()),
            Value::Array(items) => {
                let parts: Vec<_> = items
                    .iter()
                    .filter_map(Self::value_text)
                    .collect();
                (!parts.is_empty()).then(|| parts.join("\n"))
            }
            Value::Object(map) => {
                // Codex tool outputs use typed text blocks. Deliberately do not
                // serialize image/audio URLs or other binary-oriented blocks.
                if let Some(text) = map
                    .get("text")
                    .and_then(Value::as_str)
                {
                    return Some(text.to_string());
                }
                if let Some(content) = map.get("content") {
                    return Self::value_text(content);
                }
                None
            }
            Value::Bool(_) | Value::Number(_) => Some(value.to_string()),
        }
    }

    fn serialized_field(payload: &Value, fields: &[&str]) -> Option<String> {
        fields
            .iter()
            .find_map(|field| {
                let value = payload.get(*field)?;
                if let Some(text) = value.as_str() {
                    Some(text.to_string())
                } else if value.is_null() {
                    None
                } else {
                    serde_json::to_string(value).ok()
                }
            })
    }

    fn tool_name(payload: &Value) -> String {
        let name = payload
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        match payload
            .get("namespace")
            .and_then(Value::as_str)
        {
            Some(namespace) if !namespace.is_empty() => format!("{namespace}.{name}"),
            _ => name.to_string(),
        }
    }

    fn record_id(payload: &Value, session_id: &str, sequence: usize) -> String {
        payload
            .get("id")
            .or_else(|| payload.get("call_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{session_id}-{sequence}"))
    }

    #[allow(clippy::too_many_arguments)]
    fn entry(
        path: &Path,
        session_id: &str,
        project_path: &str,
        timestamp: DateTime<Utc>,
        sequence_num: usize,
        uuid: String,
        message_type: MessageType,
        record_kind: RecordKind,
        content: String,
        model: Option<String>,
        tools_used: Vec<String>,
    ) -> ConversationEntry {
        let content = strip_str(&content);
        let (technologies, mut tools_mentioned, code_languages, has_code, has_error) =
            metadata::extract_all_metadata(&content);
        for tool in tools_used {
            if !tools_mentioned.contains(&tool) {
                tools_mentioned.push(tool);
            }
        }

        ConversationEntry {
            source: Source::Codex,
            uuid,
            parent_uuid: None,
            session_id: session_id.to_string(),
            source_artifact: path.to_path_buf(),
            project_path: project_path.to_string(),
            timestamp,
            message_type,
            record_kind,
            content,
            model,
            cwd: (!project_path.is_empty()).then(|| project_path.to_string()),
            sequence_num,
            is_sidechain: false,
            agent_id: None,
            technologies,
            has_code,
            code_languages,
            has_error,
            tools_mentioned,
        }
    }

    pub fn parse_file(&self, path: &Path) -> Result<Vec<ConversationEntry>> {
        let content = std::fs::read_to_string(path)?;
        let mut entries = Vec::new();
        let mut session_id = String::new();
        let mut project_path = String::new();
        let mut session_timestamp = None;
        let mut sequence_counter = 0;
        let mut tool_names_by_call_id = HashMap::new();

        let lines: Vec<_> = content
            .lines()
            .collect();
        for (line_index, line) in lines
            .iter()
            .enumerate()
        {
            let line_trimmed = line.trim();
            if line_trimmed.is_empty() {
                continue;
            }

            let value: Value = match serde_json::from_str(line_trimmed) {
                Ok(value) => value,
                Err(_) if line_index + 1 == lines.len() => {
                    // Active rollouts may end with one partially-written JSON object.
                    continue;
                }
                Err(error) => bail!(
                    "invalid JSON at {}:{}: {}",
                    path.display(),
                    line_index + 1,
                    error
                ),
            };

            let entry_type = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("");
            if entry_type == "session_meta" {
                if let Some(payload) = value.get("payload") {
                    // `id` is the source-native resumable thread ID. `session_id`
                    // can instead identify a root in fork/subagent session trees.
                    if let Some(id) = payload
                        .get("id")
                        .or_else(|| payload.get("session_id"))
                        .and_then(Value::as_str)
                    {
                        session_id = id.to_string();
                    }
                    session_timestamp = value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
                        .map(|timestamp| timestamp.with_timezone(&Utc));
                    if let Some(cwd) = payload
                        .get("cwd")
                        .and_then(Value::as_str)
                    {
                        project_path = cwd.to_string();
                    }
                }
                continue;
            }

            if session_id.is_empty() {
                session_id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("unknown")
                    .to_string();
            }

            // response_item is the canonical stored stream. event_msg mirrors
            // user/assistant events and is intentionally not indexed twice.
            if entry_type != "response_item" {
                continue;
            }
            let Some(payload) = value.get("payload") else {
                continue;
            };
            let payload_type = payload
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("");
            let timestamp = Self::timestamp(&value, payload, session_timestamp, path)?;

            let entry = match payload_type {
                "message" => {
                    let role = payload
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let message_type = match role {
                        "user" => MessageType::User,
                        "assistant" => MessageType::Assistant,
                        // Developer/system records include instructions and injected
                        // runtime context, not user-owned conversation evidence.
                        "developer" | "system" => continue,
                        _ => continue,
                    };

                    let mut text_parts = Vec::new();
                    if let Some(items) = payload
                        .get("content")
                        .and_then(Value::as_array)
                    {
                        // Approval-assessment rollouts can split their injected
                        // transcript marker and mirrored tool trace across
                        // multiple text blocks. If any block identifies that
                        // synthetic envelope, exclude the whole message.
                        if items
                            .iter()
                            .any(|item| {
                                item.get("text")
                                    .and_then(Value::as_str)
                                    .is_some_and(Self::is_mirrored_approval_context)
                            })
                        {
                            continue;
                        }
                        for item in items {
                            if let Some(text) = item
                                .get("text")
                                .and_then(Value::as_str)
                                && !Self::is_injected_context(text)
                            {
                                text_parts.push(text);
                            }
                        }
                    } else if let Some(text) = payload
                        .get("text")
                        .and_then(Value::as_str)
                        && !Self::is_injected_context(text)
                    {
                        text_parts.push(text);
                    }
                    let content = text_parts
                        .join("\n")
                        .trim()
                        .to_string();
                    if content.is_empty() {
                        continue;
                    }

                    Self::entry(
                        path,
                        &session_id,
                        &project_path,
                        timestamp,
                        sequence_counter,
                        Self::record_id(payload, &session_id, sequence_counter),
                        message_type,
                        RecordKind::Conversation,
                        content,
                        payload
                            .get("model")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        Vec::new(),
                    )
                }
                "custom_tool_call" | "function_call" | "tool_search_call" => {
                    let tool_name = Self::tool_name(payload);
                    if let Some(call_id) = payload
                        .get("call_id")
                        .and_then(Value::as_str)
                    {
                        tool_names_by_call_id.insert(call_id.to_string(), tool_name.clone());
                    }
                    let input = Self::serialized_field(payload, &["input", "arguments"])
                        .unwrap_or_default();
                    let content = if input
                        .trim()
                        .is_empty()
                    {
                        format!("[tool:{tool_name}]")
                    } else {
                        format!("[tool:{tool_name}]\n{input}")
                    };
                    Self::entry(
                        path,
                        &session_id,
                        &project_path,
                        timestamp,
                        sequence_counter,
                        Self::record_id(payload, &session_id, sequence_counter),
                        MessageType::Assistant,
                        RecordKind::ToolCall,
                        content,
                        None,
                        vec![tool_name],
                    )
                }
                "custom_tool_call_output" | "function_call_output" | "tool_search_output" => {
                    let call_id = payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let tool_name = tool_names_by_call_id
                        .get(call_id)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    let output = payload
                        .get("output")
                        .and_then(Self::value_text)
                        .or_else(|| Self::serialized_field(payload, &["tools"]))
                        .unwrap_or_default();
                    if output
                        .trim()
                        .is_empty()
                    {
                        continue;
                    }
                    Self::entry(
                        path,
                        &session_id,
                        &project_path,
                        timestamp,
                        sequence_counter,
                        Self::record_id(payload, &session_id, sequence_counter),
                        MessageType::Assistant,
                        RecordKind::ToolResult,
                        format!("[tool_result:{tool_name}]\n{output}"),
                        None,
                        vec![tool_name],
                    )
                }
                // Encrypted/summarized reasoning is not user-visible evidence and
                // is intentionally excluded along with unknown internal records.
                _ => continue,
            };

            entries.push(entry);
            sequence_counter += 1;
        }

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    #[test]
    fn parses_messages_and_full_textual_tool_evidence() {
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/codex/sample_rollout.jsonl");

        let entries = CodexParser::new()
            .parse_file(&fixture_path)
            .expect("Failed to parse fixture");

        assert_eq!(entries.len(), 4);
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
        assert_eq!(entries[1].message_type, MessageType::Assistant);
        assert!(
            entries[1]
                .content
                .contains("oauth2 crate")
        );
        assert!(
            entries[2]
                .content
                .contains("[tool:database.query]")
        );
        assert!(
            entries[2]
                .content
                .contains("SELECT access_method")
        );
        assert_eq!(entries[2].record_kind, RecordKind::ToolCall);
        assert!(
            entries[3]
                .content
                .contains("[tool_result:database.query]")
        );
        assert!(
            entries[3]
                .content
                .contains("service-account")
        );
        assert_eq!(entries[3].record_kind, RecordKind::ToolResult);
        assert_eq!(entries[0].source_artifact, fixture_path);
    }

    #[test]
    fn rejects_invalid_json_before_the_partial_final_line() {
        let mut fixture = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            fixture,
            r#"{{"timestamp":"2026-07-29T20:42:41Z","type":"session_meta","payload":{{"id":"thread-1","cwd":"/tmp"}}}}"#
        )
        .unwrap();
        writeln!(fixture, "not json").unwrap();
        writeln!(
            fixture,
            r#"{{"timestamp":"2026-07-29T20:43:00Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"hello"}}]}}}}"#
        )
        .unwrap();

        let error = CodexParser::new()
            .parse_file(fixture.path())
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(":2:")
        );
    }
}
