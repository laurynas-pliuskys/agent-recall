//! Durable, explicit ingestion for one-off Claude web exports.
//!
//! Imported conversations live outside the Tantivy cache so `index rebuild`
//! can recreate the index after the original export has disappeared.

use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

const IMPORT_DIRECTORY: &str = "claude-web";

/// Managed source directory for one-off Claude web imports.
pub fn managed_import_dir() -> Result<PathBuf> {
    let data_dir = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or_else(|| anyhow!("Could not determine a local data directory"))?;
    Ok(data_dir
        .join("agent-recall")
        .join("imports")
        .join(IMPORT_DIRECTORY))
}

/// Import an export into durable managed storage and return the number of
/// source-native conversations imported.
pub fn import_claude_web_export(input: &Path) -> Result<usize> {
    let destination = managed_import_dir()?;
    super::claude_export_parser::import_export_file(input, &destination)
}

/// Discover managed per-conversation artifacts. No user Downloads or export
/// directories are scanned automatically.
pub fn discover_managed_exports() -> Result<Vec<PathBuf>> {
    let directory = managed_import_dir()?;
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut artifacts = std::fs::read_dir(directory)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_managed_export_artifact(path))
        .collect::<Vec<_>>();
    artifacts.sort();
    Ok(artifacts)
}

pub fn is_managed_export_artifact(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("conversation-") && name.ends_with(".claude-web.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn recognizes_only_managed_artifact_names() {
        assert!(is_managed_export_artifact(Path::new(
            "conversation-abc.claude-web.json"
        )));
        assert!(!is_managed_export_artifact(Path::new("conversations.json")));
    }

    #[test]
    fn reimport_upserts_without_removing_absent_conversations() {
        let temporary = TempDir::new().unwrap();
        let source = temporary
            .path()
            .join("export.json");
        let managed = temporary
            .path()
            .join("managed");
        std::fs::write(
            &source,
            r#"[{"uuid":"one","chat_messages":[]},{"uuid":"two","chat_messages":[]}]"#,
        )
        .unwrap();
        assert_eq!(
            super::super::claude_export_parser::import_export_file(&source, &managed).unwrap(),
            2
        );

        std::fs::write(
            &source,
            r#"[{"uuid":"one","name":"updated","chat_messages":[]}]"#,
        )
        .unwrap();
        assert_eq!(
            super::super::claude_export_parser::import_export_file(&source, &managed).unwrap(),
            1
        );
        assert!(
            managed
                .join("conversation-one.claude-web.json")
                .exists()
        );
        assert!(
            managed
                .join("conversation-two.claude-web.json")
                .exists()
        );
    }
}
