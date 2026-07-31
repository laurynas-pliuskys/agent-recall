use agent_recall::shared::{
    ConversationEntry, MessageType, SearchEngine, SearchIndexer, SearchQuery, Source,
};
use chrono::Utc;
use std::collections::HashMap;
use tempfile::TempDir;

fn make_test_entry(
    source: Source,
    uuid: &str,
    session_id: &str,
    msg_type: MessageType,
    content: &str,
    project: &str,
    seq: usize,
) -> ConversationEntry {
    ConversationEntry {
        source,
        uuid: uuid.to_string(),
        parent_uuid: None,
        session_id: session_id.to_string(),
        project_path: project.to_string(),
        timestamp: Utc::now(),
        message_type: msg_type,
        content: content.to_string(),
        model: Some("gpt-5".to_string()),
        cwd: Some(project.to_string()),
        sequence_num: seq,
        is_sidechain: false,
        agent_id: None,
        technologies: vec!["rust".to_string()],
        has_code: false,
        code_languages: vec![],
        has_error: false,
        tools_mentioned: vec![],
    }
}

#[test]
fn test_union_search_and_source_filtering() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let index_path = temp_dir
        .path()
        .join("index");

    let mut indexer = SearchIndexer::new(&index_path)?;

    // 1. Add Claude entries
    let claude_entries = vec![
        make_test_entry(
            Source::Claude,
            "c-msg-1",
            "sess-shared-123",
            MessageType::User,
            "How do I set up Cloudflare tunnel in Rust?",
            "/projects/backend",
            0,
        ),
        make_test_entry(
            Source::Claude,
            "c-msg-2",
            "sess-shared-123",
            MessageType::Assistant,
            "You can use the cloudflare api crate for managing tunnels.",
            "/projects/backend",
            1,
        ),
    ];

    // 2. Add Codex entries (including duplicate session_id to test collision safety)
    let codex_entries = vec![
        make_test_entry(
            Source::Codex,
            "cx-msg-1",
            "sess-shared-123",
            MessageType::User,
            "How do I set up Cloudflare Access policies in Rust?",
            "/projects/backend",
            0,
        ),
        make_test_entry(
            Source::Codex,
            "cx-msg-2",
            "sess-shared-123",
            MessageType::Assistant,
            "Cloudflare Access policies can be set up via REST API calls.",
            "/projects/backend",
            1,
        ),
    ];

    indexer.index_conversations(claude_entries)?;
    indexer.index_conversations(codex_entries)?;
    indexer.commit()?;

    let mut session_counts = HashMap::new();
    session_counts.insert("sess-shared-123".to_string(), 4);

    let search_engine = SearchEngine::new(&index_path, session_counts)?;

    // Test A: Union Search (Default search finds Cloudflare in both sources)
    let union_query = SearchQuery {
        text: "Cloudflare".to_string(),
        source_filter: None,
        project_filter: None,
        session_filter: None,
        limit: 10,
        sort_by: Default::default(),
        after: None,
        before: None,
    };
    let union_results = search_engine.search(union_query)?;
    assert_eq!(
        union_results.len(),
        4,
        "Union search should find all 4 entries across Claude and Codex"
    );

    let claude_count = union_results
        .iter()
        .filter(|r| r.source == Source::Claude)
        .count();
    let codex_count = union_results
        .iter()
        .filter(|r| r.source == Source::Codex)
        .count();
    assert_eq!(
        claude_count, 2,
        "Union search should contain 2 Claude results"
    );
    assert_eq!(
        codex_count, 2,
        "Union search should contain 2 Codex results"
    );

    // Test B: Filter by source = Claude
    let claude_query = SearchQuery {
        text: "Cloudflare".to_string(),
        source_filter: Some(Source::Claude),
        project_filter: None,
        session_filter: None,
        limit: 10,
        sort_by: Default::default(),
        after: None,
        before: None,
    };
    let claude_results = search_engine.search(claude_query)?;
    assert_eq!(
        claude_results.len(),
        2,
        "Claude filter should return only 2 entries"
    );
    assert!(
        claude_results
            .iter()
            .all(|r| r.source == Source::Claude)
    );

    // Test C: Filter by source = Codex
    let codex_query = SearchQuery {
        text: "Cloudflare".to_string(),
        source_filter: Some(Source::Codex),
        project_filter: None,
        session_filter: None,
        limit: 10,
        sort_by: Default::default(),
        after: None,
        before: None,
    };
    let codex_results = search_engine.search(codex_query)?;
    assert_eq!(
        codex_results.len(),
        2,
        "Codex filter should return only 2 entries"
    );
    assert!(
        codex_results
            .iter()
            .all(|r| r.source == Source::Codex)
    );

    Ok(())
}
