use super::metadata;
use super::models::{ContentBlock, ConversationEntry, MessageType, RawJsonlMessage};
use super::utils::truncate_content;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::io::BufReader;
use std::path::Path;
use strip_ansi_escapes::strip_str;
use tracing::warn;

use super::config::get_config;

/// Read text file, skipping UTF-8 BOM if present
fn read_text_file(path: &Path) -> Result<String> {
    use std::fs::File;
    use std::io::Read;

    let mut file = BufReader::new(File::open(path)?);
    let mut first3 = [0u8; 3];

    // Check for UTF-8 BOM (EF BB BF)
    match file.read_exact(&mut first3) {
        Ok(()) if first3 == [0xEF, 0xBB, 0xBF] => {
            // BOM found, skip it
        }
        Ok(()) => {
            // No BOM, include these bytes
            let mut content = String::from_utf8(first3.to_vec())?;
            file.read_to_string(&mut content)?;
            return Ok(content);
        }
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            // File shorter than 3 bytes
            return Ok(String::from_utf8(first3[..].to_vec())?);
        }
        Err(e) => return Err(e.into()),
    }

    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok(content)
}

#[derive(Default)]
pub struct JsonlParser {
    full_content: bool,
}

impl JsonlParser {
    pub fn with_full_content() -> Self {
        Self { full_content: true }
    }

    pub fn parse_file(&self, path: &Path) -> Result<Vec<ConversationEntry>> {
        let content = read_text_file(path)?;
        let mut entries = Vec::new();
        let project_name = self.extract_project_name(path);

        // Detect if this is an agent file
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let is_agent_file = filename.starts_with("agent-");
        let file_agent_id = if is_agent_file {
            filename
                .strip_prefix("agent-")
                .and_then(|s| s.strip_suffix(".jsonl"))
                .map(|s| s.to_string())
        } else {
            None
        };

        let mut sequence_counter = 0;
        for (line_num, line) in content
            .lines()
            .enumerate()
        {
            if line
                .trim()
                .is_empty()
            {
                continue;
            }

            match serde_json::from_str::<RawJsonlMessage>(line) {
                Ok(raw) => {
                    if let Some(entry) = self.parse_raw_message(
                        raw,
                        &project_name,
                        sequence_counter,
                        &file_agent_id,
                        path,
                    ) {
                        entries.push(entry);
                        sequence_counter += 1;
                    }
                }
                Err(e) => {
                    warn!("Invalid JSON at {}:{}: {}", path.display(), line_num + 1, e);
                }
            }
        }

        Ok(entries)
    }

    fn parse_raw_message(
        &self,
        raw: RawJsonlMessage,
        fallback_project: &str,
        sequence_num: usize,
        file_agent_id: &Option<String>,
        source_artifact: &Path,
    ) -> Option<ConversationEntry> {
        let msg_type = raw
            .message_type
            .as_deref()?;

        // Filter out noise message types
        match msg_type {
            "file-history-snapshot" | "queue-operation" => return None,
            "user" | "assistant" | "summary" => {}
            _ => return None, // Skip unknown types
        }

        // Get required fields
        let uuid = raw
            .uuid
            .clone()?;
        let session_id = raw
            .session_id
            .clone()?;
        let timestamp_str = raw
            .timestamp
            .as_deref()?;
        let timestamp: DateTime<Utc> = timestamp_str
            .parse()
            .ok()?;

        // Determine message type
        let message_type = match msg_type {
            "user" => MessageType::User,
            "assistant" => MessageType::Assistant,
            "summary" => MessageType::Summary,
            _ => MessageType::System,
        };

        // Extract searchable content, stripping ANSI escape sequences from tool output
        let (content, mut has_error, tools_used) = if msg_type == "summary" {
            (
                raw.summary
                    .unwrap_or_default(),
                false,
                Vec::new(),
            )
        } else {
            self.extract_searchable_content(&raw)
        };
        let content = strip_str(&content);

        // Skip empty content
        if content
            .trim()
            .is_empty()
        {
            return None;
        }

        // Get project path from cwd or fallback
        let project_path = raw
            .cwd
            .as_ref()
            .map(|cwd| self.extract_project_name_from_path(cwd))
            .unwrap_or_else(|| fallback_project.to_string());

        // Get model from message
        let model = raw
            .message
            .as_ref()
            .and_then(|m| {
                m.model
                    .clone()
            });

        // Use agent_id from message or from filename
        let agent_id = raw
            .agent_id
            .or_else(|| file_agent_id.clone());

        let (technologies, code_languages, has_code, tools_mentioned) = if get_config()
            .index
            .enable_tagging
        {
            let (techs, tools, langs, has_code, content_has_error) =
                metadata::extract_all_metadata(&content);
            has_error |= content_has_error;
            let mut all_tools = tools;
            for tool in tools_used {
                if !all_tools.contains(&tool) {
                    all_tools.push(tool);
                }
            }
            (techs, langs, has_code, all_tools)
        } else {
            (vec![], vec![], false, tools_used)
        };

        Some(ConversationEntry {
            source: super::source::Source::Claude,
            uuid,
            parent_uuid: raw.parent_uuid,
            session_id,
            source_artifact: source_artifact.to_path_buf(),
            project_path,
            timestamp,
            message_type,
            record_kind: super::models::RecordKind::Conversation,
            content,
            model,
            cwd: raw.cwd,
            sequence_num,
            is_sidechain: raw
                .is_sidechain
                .unwrap_or(false),
            agent_id,
            technologies,
            has_code,
            code_languages,
            has_error,
            tools_mentioned,
        })
    }

    /// Extract searchable content from message, filtering noise
    fn extract_searchable_content(&self, raw: &RawJsonlMessage) -> (String, bool, Vec<String>) {
        let message = match &raw.message {
            Some(m) => m,
            None => return (String::new(), false, Vec::new()),
        };

        let content_value = match &message.content {
            Some(c) => c,
            None => return (String::new(), false, Vec::new()),
        };

        // Handle string content (simple user messages)
        if let Some(text) = content_value.as_str() {
            return (text.to_string(), false, Vec::new());
        }

        // Handle array content (assistant messages with blocks)
        let blocks = match content_value.as_array() {
            Some(arr) => arr,
            None => return (String::new(), false, Vec::new()),
        };

        let mut parts = Vec::new();
        let mut has_error = false;
        let mut tools_used = Vec::new();

        for block in blocks {
            if let Some(content_block) = self.parse_content_block(block) {
                match content_block {
                    ContentBlock::Text(text) => {
                        parts.push(text);
                    }
                    ContentBlock::Thinking(thinking) => {
                        // Include thinking - valuable reasoning content
                        parts.push(format!("[thinking] {}", thinking));
                    }
                    ContentBlock::ToolUse {
                        name,
                        input_preview,
                    } => {
                        // Include tool name and truncated input
                        tools_used.push(name.clone());
                        if !input_preview.is_empty() {
                            parts.push(format!("[{}] {}", name, input_preview));
                        }
                    }
                    ContentBlock::ToolResult {
                        content_preview,
                        is_error,
                    } => {
                        // Include truncated result and error flag
                        if is_error {
                            has_error = true;
                            parts.push(format!("[error] {}", content_preview));
                        } else if !content_preview
                            .trim()
                            .is_empty()
                        {
                            // Only include non-empty, non-error results (truncated)
                            parts.push(format!("[result] {}", content_preview));
                        }
                    }
                }
            }
        }

        (parts.join("\n"), has_error, tools_used)
    }

    fn parse_content_block(&self, block: &serde_json::Value) -> Option<ContentBlock> {
        let block_type = block
            .get("type")?
            .as_str()?;

        match block_type {
            "text" => {
                let text = block
                    .get("text")?
                    .as_str()?;
                Some(ContentBlock::Text(text.to_string()))
            }
            "thinking" => {
                let thinking = block
                    .get("thinking")?
                    .as_str()?;
                Some(ContentBlock::Thinking(thinking.to_string()))
            }
            "tool_use" => {
                let name = block
                    .get("name")?
                    .as_str()?
                    .to_string();
                let input = block.get("input");
                let input_preview = input
                    .map(|v| {
                        let s = v.to_string();
                        if self.full_content {
                            s
                        } else {
                            truncate_content(
                                &s,
                                get_config()
                                    .limits
                                    .tool_input_max_chars,
                                false,
                            )
                        }
                    })
                    .unwrap_or_default();
                Some(ContentBlock::ToolUse {
                    name,
                    input_preview,
                })
            }
            "tool_result" => {
                let is_error = block
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let content = block.get("content");
                let extracted = content.and_then(|v| {
                    if let Some(s) = v.as_str() {
                        Some(s.to_string())
                    } else if let Some(arr) = v.as_array() {
                        let texts: Vec<&str> = arr
                            .iter()
                            .filter_map(|item| {
                                item.get("text")
                                    .and_then(|t| t.as_str())
                            })
                            .collect();
                        Some(texts.join(" "))
                    } else {
                        None
                    }
                });
                let content_preview = extracted
                    .map(|s| {
                        if self.full_content {
                            s
                        } else {
                            truncate_content(
                                &s,
                                get_config()
                                    .limits
                                    .tool_result_max_chars,
                                false,
                            )
                        }
                    })
                    .unwrap_or_default();
                Some(ContentBlock::ToolResult {
                    content_preview,
                    is_error,
                })
            }
            _ => None,
        }
    }

    fn extract_project_name(&self, path: &Path) -> String {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    }

    fn extract_project_name_from_path(&self, cwd_path: &str) -> String {
        let path = Path::new(cwd_path);
        let components: Vec<&str> = path
            .components()
            .filter_map(|c| {
                c.as_os_str()
                    .to_str()
            })
            .collect();

        // Look for meaningful project name, skip common dirs
        for i in (0..components.len()).rev() {
            let component = components[i];
            if matches!(
                component,
                "src" | "lib" | "bin" | "target" | "node_modules" | ".git"
            ) {
                continue;
            }
            if !component.starts_with('.') && component.len() > 1 {
                return component.to_string();
            }
        }

        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_message() {
        let json = r#"{"uuid":"abc123","sessionId":"sess1","type":"user","timestamp":"2025-12-28T10:00:00Z","message":{"role":"user","content":"Hello world"}}"#;
        let raw: RawJsonlMessage = serde_json::from_str(json).unwrap();
        let parser = JsonlParser::default();
        let entry = parser
            .parse_raw_message(raw, "test", 0, &None, Path::new("/test/session.jsonl"))
            .unwrap();

        assert_eq!(entry.uuid, "abc123");
        assert_eq!(entry.content, "Hello world");
        assert_eq!(entry.message_type, MessageType::User);
    }

    #[test]
    fn test_skip_file_history_snapshot() {
        let json = r#"{"type":"file-history-snapshot","messageId":"xyz"}"#;
        let raw: RawJsonlMessage = serde_json::from_str(json).unwrap();
        let parser = JsonlParser::default();
        let entry =
            parser.parse_raw_message(raw, "test", 0, &None, Path::new("/test/session.jsonl"));

        assert!(entry.is_none());
    }

    #[test]
    fn test_parse_assistant_with_text_block() {
        let json = r#"{"uuid":"abc123","sessionId":"sess1","type":"assistant","timestamp":"2025-12-28T10:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"Here is my response"}]}}"#;
        let raw: RawJsonlMessage = serde_json::from_str(json).unwrap();
        let parser = JsonlParser::default();
        let entry = parser
            .parse_raw_message(raw, "test", 0, &None, Path::new("/test/session.jsonl"))
            .unwrap();

        assert_eq!(entry.content, "Here is my response");
        assert_eq!(entry.message_type, MessageType::Assistant);
    }

    #[test]
    fn test_parse_thinking_block() {
        let json = r#"{"uuid":"abc123","sessionId":"sess1","type":"assistant","timestamp":"2025-12-28T10:00:00Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Let me think about this..."}]}}"#;
        let raw: RawJsonlMessage = serde_json::from_str(json).unwrap();
        let parser = JsonlParser::default();
        let entry = parser
            .parse_raw_message(raw, "test", 0, &None, Path::new("/test/session.jsonl"))
            .unwrap();

        assert!(
            entry
                .content
                .contains("[thinking]")
        );
        assert!(
            entry
                .content
                .contains("Let me think about this")
        );
    }

    #[test]
    fn test_tool_result_truncation() {
        let long_content = "x".repeat(5000);
        let json = format!(
            r#"{{"uuid":"abc123","sessionId":"sess1","type":"assistant","timestamp":"2025-12-28T10:00:00Z","message":{{"role":"assistant","content":[{{"type":"tool_result","content":"{}"}}]}}}}"#,
            long_content
        );
        let raw: RawJsonlMessage = serde_json::from_str(&json).unwrap();
        let parser = JsonlParser::default();
        let entry = parser
            .parse_raw_message(raw, "test", 0, &None, Path::new("/test/session.jsonl"))
            .unwrap();

        // Should be truncated to ~get_config().limits.tool_result_max_chars + "[result] " prefix + "…"
        assert!(
            entry
                .content
                .len()
                < get_config()
                    .limits
                    .tool_result_max_chars
                    + 100
        );
        assert!(
            entry
                .content
                .ends_with('…')
        );
    }
}
