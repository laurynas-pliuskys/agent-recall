use super::{ConversationEntry, SearchEngine, Source, conversation_source, short_uuid};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Resolve the source artifact retained by the primary index for a conversation.
pub fn conversation_artifact(
    search_engine: &SearchEngine,
    source: Source,
    session_id: &str,
) -> Result<Option<PathBuf>> {
    Ok(search_engine
        .get_conversation_messages(source, session_id)?
        .first()
        .map(|message| {
            message
                .source_artifact
                .clone()
        })
        .filter(|path| {
            !path
                .as_os_str()
                .is_empty()
        }))
}

/// Search technical references inside one selected source artifact. Reference
/// content is parsed from the original transcript and is never added to the
/// primary Tantivy index by this operation.
pub fn search_conversation_references(
    source: Source,
    artifact: &Path,
    session_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<ConversationEntry>> {
    let query = query
        .trim()
        .to_lowercase();
    if query.is_empty() {
        bail!("Reference query must not be empty");
    }

    let query_terms: Vec<_> = query
        .split_whitespace()
        .collect();
    let adapter = conversation_source(source);
    let mut matches: Vec<_> = adapter
        .parse(artifact, true)?
        .into_iter()
        .filter(|entry| entry.session_id == session_id && adapter.is_reference_record(entry))
        .filter_map(|entry| {
            let content = entry
                .content
                .to_lowercase();
            let score = if content.contains(&query) {
                2
            } else if query_terms
                .iter()
                .all(|term| content.contains(term))
            {
                1
            } else {
                return None;
            };
            Some((score, entry))
        })
        .collect();

    matches.sort_by(|(score_a, entry_a), (score_b, entry_b)| {
        score_b
            .cmp(score_a)
            .then_with(|| {
                entry_a
                    .sequence_num
                    .cmp(&entry_b.sequence_num)
            })
    });
    Ok(matches
        .into_iter()
        .take(limit)
        .map(|(_, entry)| entry)
        .collect())
}

pub fn format_reference_matches(
    source: Source,
    session_id: &str,
    query: &str,
    matches: &[ConversationEntry],
    truncate_length: usize,
) -> String {
    if matches.is_empty() {
        return format!(
            "No source-backed references found in [{}:{}] for '{}'.",
            source, session_id, query
        );
    }

    let mut output = format!(
        "Found {} source-backed references in [{}:{}] for '{}':\n",
        matches.len(),
        source,
        session_id,
        query
    );
    for (index, entry) in matches
        .iter()
        .enumerate()
    {
        let collapsed = entry
            .content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let preview: String = if truncate_length == 0 {
            collapsed
        } else {
            let truncated: String = collapsed
                .chars()
                .take(truncate_length)
                .collect();
            if collapsed
                .chars()
                .count()
                > truncate_length
            {
                format!("{truncated}…")
            } else {
                truncated
            }
        };
        output.push_str(&format!(
            "{}. 💬 {} 📅 {}\n   {}\n",
            index + 1,
            short_uuid(&entry.uuid),
            entry
                .timestamp
                .format("%Y-%m-%d %H:%M"),
            preview
        ));
    }
    output.push_str(&format!(
        "↪ get_session_messages(source=\"{}\", session_id=\"{}\", center_on=\"<message-id>\", -C=0, include=[\"tools\"], truncate_length=0)",
        source, session_id
    ));
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::{CodexParser, SearchIndexer, SearchQuery};
    use std::collections::HashMap;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sample_rollout.jsonl")
    }

    #[test]
    fn searches_codex_references_without_primary_messages() {
        let matches = search_conversation_references(
            Source::Codex,
            &fixture(),
            "019faf9d-7ce8-70b0-be6b-3ceacc3500e2",
            "SELECT access_method",
            10,
        )
        .unwrap();

        assert_eq!(matches.len(), 1);
        assert!(
            matches[0]
                .content
                .contains("SELECT access_method")
        );
        assert!(matches[0].is_tool_record());

        let all_entries = CodexParser::new()
            .parse_file(&fixture())
            .unwrap();
        let adapter = conversation_source(Source::Codex);
        assert_eq!(
            all_entries
                .iter()
                .filter(|entry| adapter.is_primary_index_record(entry))
                .count(),
            2
        );
        assert_eq!(
            all_entries
                .iter()
                .filter(|entry| adapter.is_reference_record(entry))
                .count(),
            2
        );
    }

    #[test]
    fn codex_reference_payloads_do_not_enter_primary_search() {
        let all_entries = CodexParser::new()
            .parse_file(&fixture())
            .unwrap();
        let adapter = conversation_source(Source::Codex);
        let primary_entries: Vec<_> = all_entries
            .into_iter()
            .filter(|entry| adapter.is_primary_index_record(entry))
            .collect();
        let temp_dir = tempfile::tempdir().unwrap();
        let mut indexer = SearchIndexer::new(temp_dir.path()).unwrap();
        indexer
            .index_conversations(primary_entries)
            .unwrap();
        indexer
            .commit()
            .unwrap();
        drop(indexer);

        let search = SearchEngine::new(temp_dir.path(), HashMap::new()).unwrap();
        let tool_only = search
            .search(SearchQuery {
                text: "SELECT access_method".to_string(),
                source_filter: Some(Source::Codex),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert!(tool_only.is_empty());

        let conversation = search
            .search(SearchQuery {
                text: "OAuth2 authentication".to_string(),
                source_filter: Some(Source::Codex),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(conversation.len(), 2);
        assert!(
            conversation
                .iter()
                .all(|result| !result.is_tool_record())
        );
    }

    #[test]
    fn formats_bounded_reference_previews_with_exact_expansion_hint() {
        let matches = search_conversation_references(
            Source::Codex,
            &fixture(),
            "019faf9d-7ce8-70b0-be6b-3ceacc3500e2",
            "service-account",
            10,
        )
        .unwrap();
        let output = format_reference_matches(
            Source::Codex,
            "019faf9d-7ce8-70b0-be6b-3ceacc3500e2",
            "service-account",
            &matches,
            30,
        );

        assert!(output.contains("service-account"));
        assert!(output.contains("include=[\"tools\"]"));
        assert!(output.contains("truncate_length=0"));
    }
}
