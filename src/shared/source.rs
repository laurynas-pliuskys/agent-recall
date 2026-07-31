use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Identifies the conversation client/source
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Claude,
    Codex,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Source::Claude => "Claude Code",
            Source::Codex => "Codex",
        }
    }

    pub fn resume_hint(&self, session_id: &str) -> String {
        match self {
            Source::Claude => format!("claude --resume {}", session_id),
            Source::Codex => format!("codex resume {}", session_id),
        }
    }

    /// Construct a unique document key scoped by source
    pub fn doc_key(&self, session_id: &str, message_id: &str) -> String {
        format!("{}:{}:{}", self.as_str(), session_id, message_id)
    }

    /// Construct a unique session/conversation key scoped by source
    pub fn conversation_key(&self, session_id: &str) -> String {
        format!("{}:{}", self.as_str(), session_id)
    }
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
            "codex" => Ok(Source::Codex),
            other => Err(format!(
                "Unknown source: '{}'. Expected 'claude' or 'codex'.",
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
        assert_eq!(Source::Codex.as_str(), "codex");

        assert_eq!("claude".parse::<Source>(), Ok(Source::Claude));
        assert_eq!("Codex".parse::<Source>(), Ok(Source::Codex));
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
        assert_eq!(Source::Codex.resume_hint("xyz-789"), "codex resume xyz-789");
    }

    #[test]
    fn test_source_keys() {
        assert_eq!(Source::Claude.conversation_key("sess1"), "claude:sess1");
        assert_eq!(Source::Codex.doc_key("sess2", "msg1"), "codex:sess2:msg1");
    }
}
