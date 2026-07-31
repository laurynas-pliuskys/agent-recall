use super::models::{SearchQuery, SearchResult, SortOrder};
use super::path_utils::{session_jsonl_path, short_uuid};
use super::source::Source;
use super::terminal::file_hyperlink;
use super::utils::truncate_content;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Value};
use tantivy::{Index, IndexReader, ReloadPolicy, TantivyDocument, Term};

/// Extract project name from a path and split into TEXT-tokenizer segments.
/// Tantivy's default TEXT tokenizer splits on non-alphanumeric characters,
/// so "/path/to/my-project_name" → ["my", "project", "name"].
fn project_filter_segments(filter: &str) -> Vec<&str> {
    let path = Path::new(filter);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(filter);
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect()
}

fn build_project_query(project_field: Field, filter: &str) -> Box<dyn tantivy::query::Query> {
    let segments = project_filter_segments(filter);
    let segment_queries: Vec<_> = segments
        .iter()
        .map(|seg| {
            let term = Term::from_field_text(project_field, &seg.to_lowercase());
            (
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                    as Box<dyn tantivy::query::Query>,
            )
        })
        .collect();
    Box::new(BooleanQuery::new(segment_queries))
}

fn project_matches(project_path: &str, filter: &str) -> bool {
    let filter_name = Path::new(filter)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(filter);
    let result_name = Path::new(project_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(project_path);
    result_name == filter_name
}

/// Maximum messages to retrieve per session.
/// Claude Code sessions rarely exceed 1000 messages; this limit prevents
/// runaway queries while covering all realistic session sizes.
const MAX_SESSION_MESSAGES: usize = 5000;

pub struct SearchEngine {
    index: Index,
    reader: IndexReader,
    uuid_field: Field,
    parent_uuid_field: Field,
    content_field: Field,
    project_field: Field,
    session_field: Field,
    timestamp_field: Field,
    message_type_field: Field,
    technologies_field: Field,
    code_languages_field: Field,
    tools_mentioned_field: Field,
    has_code_field: Field,
    has_error_field: Field,
    cwd_field: Field,
    sequence_num_field: Field,
    is_sidechain_field: Field,
    agent_id_field: Field,
    source_field: Field,
    conversation_key_field: Field,
    document_key_field: Field,
    source_artifact_field: Field,
    interaction_counts: HashMap<String, usize>,
}

impl SearchEngine {
    pub fn new(index_path: &Path, session_counts: HashMap<String, usize>) -> Result<Self> {
        let index = Index::open_in_dir(index_path)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        let schema = index.schema();
        let uuid_field = schema.get_field("uuid")?;
        let parent_uuid_field = schema.get_field("parent_uuid")?;
        let content_field = schema.get_field("content")?;
        let project_field = schema.get_field("project")?;
        let session_field = schema.get_field("session_id")?;
        let timestamp_field = schema.get_field("timestamp")?;
        let message_type_field = schema.get_field("message_type")?;
        let technologies_field = schema.get_field("technologies")?;
        let code_languages_field = schema.get_field("code_languages")?;
        let tools_mentioned_field = schema.get_field("tools_mentioned")?;
        let has_code_field = schema.get_field("has_code")?;
        let has_error_field = schema.get_field("has_error")?;
        let cwd_field = schema.get_field("cwd")?;
        let sequence_num_field = schema.get_field("sequence_num")?;
        let is_sidechain_field = schema.get_field("is_sidechain")?;
        let agent_id_field = schema.get_field("agent_id")?;
        let source_field = schema.get_field("source")?;
        let conversation_key_field = schema.get_field("conversation_key")?;
        let document_key_field = schema.get_field("document_key")?;
        let source_artifact_field = schema.get_field("source_artifact")?;

        Ok(Self {
            index,
            reader,
            uuid_field,
            parent_uuid_field,
            content_field,
            project_field,
            session_field,
            timestamp_field,
            message_type_field,
            technologies_field,
            code_languages_field,
            tools_mentioned_field,
            has_code_field,
            has_error_field,
            cwd_field,
            sequence_num_field,
            is_sidechain_field,
            agent_id_field,
            source_field,
            conversation_key_field,
            document_key_field,
            source_artifact_field,
            interaction_counts: session_counts,
        })
    }

    pub fn search(&self, query: SearchQuery) -> Result<Vec<SearchResult>> {
        let searcher = self
            .reader
            .searcher();

        let query_parser = QueryParser::for_index(
            &self.index,
            vec![self.content_field, self.session_field, self.project_field],
        );
        let text_query = query_parser.parse_query(&query.text)?;

        let mut final_query_parts = vec![(
            Occur::Must,
            Box::new(text_query) as Box<dyn tantivy::query::Query>,
        )];

        if let Some(ref project_filter) = query.project_filter {
            let project_query = build_project_query(self.project_field, project_filter);
            final_query_parts.push((Occur::Must, project_query));
        }

        if let Some(ref session_filter) = query.session_filter {
            // Split on hyphens like get_session_messages - TEXT fields tokenize at hyphens
            let segments: Vec<_> = session_filter
                .split('-')
                .collect();
            let segment_queries: Vec<_> = segments
                .iter()
                .map(|seg| {
                    let term = Term::from_field_text(self.session_field, seg);
                    (
                        Occur::Must,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                            as Box<dyn tantivy::query::Query>,
                    )
                })
                .collect();
            let session_query = BooleanQuery::new(segment_queries);
            final_query_parts.push((Occur::Must, Box::new(session_query)));
        }

        if let Some(ref source_filter) = query.source_filter {
            let term = Term::from_field_text(self.source_field, source_filter.as_str());
            let source_query = TermQuery::new(term, IndexRecordOption::Basic);
            final_query_parts.push((Occur::Must, Box::new(source_query)));
        }

        let final_query = if final_query_parts.len() > 1 {
            Box::new(BooleanQuery::new(final_query_parts)) as Box<dyn tantivy::query::Query>
        } else {
            final_query_parts
                .into_iter()
                .next()
                .unwrap()
                .1
        };

        let top_docs = searcher.search(&*final_query, &TopDocs::with_limit(query.limit))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let result = self.doc_to_result(&searcher.doc(doc_address)?, score, &query.text)?;

            // Apply source filter
            if let Some(ref source_filter) = query.source_filter
                && result.source != *source_filter
            {
                continue;
            }

            // Apply session prefix filter (Tantivy matches segments, but we need prefix precision)
            if let Some(ref session_filter) = query.session_filter
                && !result
                    .session_id
                    .starts_with(session_filter.as_str())
            {
                continue;
            }

            // Apply project post-filter (Tantivy matches segments, verify full project name)
            if let Some(ref project_filter) = query.project_filter
                && !project_matches(&result.project_path, project_filter)
            {
                continue;
            }

            // Apply date range filters
            if let Some(after) = query.after
                && result.timestamp < after
            {
                continue;
            }
            if let Some(before) = query.before
                && result.timestamp > before
            {
                continue;
            }

            results.push(result);
        }

        Ok(results)
    }

    /// Search with context - returns matches with surrounding messages (grep -C style)
    pub fn search_with_context(
        &self,
        query: SearchQuery,
        context_before: usize,
        context_after: usize,
    ) -> Result<Vec<SearchResultWithContext>> {
        self.search_with_context_options(query, context_before, context_after, false)
    }

    /// Search with a context window measured in records that will actually be
    /// returned. By default technical tool records remain searchable but do not
    /// consume the neighboring conversational-message budget.
    pub fn search_with_context_options(
        &self,
        query: SearchQuery,
        context_before: usize,
        context_after: usize,
        include_tools: bool,
    ) -> Result<Vec<SearchResultWithContext>> {
        // Save sort order before consuming query
        let sort_by = query
            .sort_by
            .clone();

        // First, get the matching messages
        let matches = self.search(query)?;

        let mut results_with_context = Vec::new();

        for match_result in matches {
            let session_messages =
                self.get_conversation_messages(match_result.source, &match_result.session_id)?;

            // If we can't get session messages, still return the match with just itself as context
            if session_messages.is_empty() {
                results_with_context.push(SearchResultWithContext {
                    matched_message: match_result.clone(),
                    context_messages: vec![match_result],
                    match_index: 0,
                    total_session_messages: 1,
                });
                continue;
            }

            // Sort by sequence number
            let mut session_messages = session_messages;
            session_messages.sort_by_key(|m| m.sequence_num);

            // Count records visible under the selected retrieval policy.
            let total_session_messages = session_messages
                .iter()
                .filter(|message| {
                    message.is_displayable() && (include_tools || !message.is_tool_record())
                })
                .count();

            // Find the matching message index by UUID or by content/timestamp as fallback
            let match_idx = session_messages
                .iter()
                .position(|m| m.uuid == match_result.uuid)
                .or_else(|| {
                    // Fallback: find by sequence number
                    session_messages
                        .iter()
                        .position(|m| m.sequence_num == match_result.sequence_num)
                });

            if let Some(idx) = match_idx {
                let visible_indices: Vec<_> = session_messages
                    .iter()
                    .enumerate()
                    .filter_map(|(message_idx, message)| {
                        (message.is_displayable()
                            && (include_tools || !message.is_tool_record() || message_idx == idx))
                            .then_some(message_idx)
                    })
                    .collect();
                let visible_match_idx = visible_indices
                    .iter()
                    .position(|message_idx| *message_idx == idx)
                    .unwrap_or(0);
                let start = visible_match_idx.saturating_sub(context_before);
                let end = (visible_match_idx + context_after + 1).min(visible_indices.len());
                let context_messages: Vec<_> = visible_indices[start..end]
                    .iter()
                    .map(|message_idx| session_messages[*message_idx].clone())
                    .collect();
                let mut new_match_idx = visible_match_idx - start;

                // If no context found (e.g., all filtered out), use match as its own context
                let mut context_messages = context_messages;
                if context_messages.is_empty() {
                    context_messages.push(match_result.clone());
                    new_match_idx = 0;
                }

                results_with_context.push(SearchResultWithContext {
                    matched_message: match_result,
                    context_messages,
                    match_index: new_match_idx,
                    total_session_messages,
                });
            } else {
                // UUID/sequence not found in session, return match with itself as context
                results_with_context.push(SearchResultWithContext {
                    matched_message: match_result.clone(),
                    context_messages: vec![match_result],
                    match_index: 0,
                    total_session_messages,
                });
            }
        }

        // Apply sorting based on sort_by
        match sort_by {
            SortOrder::DateDesc => {
                results_with_context.sort_by(|a, b| {
                    b.matched_message
                        .timestamp
                        .cmp(
                            &a.matched_message
                                .timestamp,
                        )
                });
            }
            SortOrder::DateAsc => {
                results_with_context.sort_by(|a, b| {
                    a.matched_message
                        .timestamp
                        .cmp(
                            &b.matched_message
                                .timestamp,
                        )
                });
            }
            SortOrder::Relevance => {
                // Already sorted by BM25 score from Tantivy
            }
        }

        Ok(results_with_context)
    }

    /// Get all messages for a session
    pub fn get_session_messages(&self, session_id: &str) -> Result<Vec<SearchResult>> {
        let results = self.get_session_messages_matching(None, session_id)?;
        let sources: std::collections::HashSet<_> = results
            .iter()
            .map(|result| result.source)
            .collect();
        if sources.len() > 1 {
            let mut source_names: Vec<_> = sources
                .into_iter()
                .map(|source| source.as_str())
                .collect();
            source_names.sort_unstable();
            anyhow::bail!(
                "Ambiguous session ID '{}'; select a source: {}",
                session_id,
                source_names.join(", ")
            );
        }
        Ok(results)
    }

    /// Get all messages for an exact source-qualified conversation.
    pub fn get_conversation_messages(
        &self,
        source: Source,
        session_id: &str,
    ) -> Result<Vec<SearchResult>> {
        let searcher = self
            .reader
            .searcher();
        let query = TermQuery::new(
            Term::from_field_text(
                self.conversation_key_field,
                &source.conversation_key(session_id),
            ),
            IndexRecordOption::Basic,
        );
        let top_docs = searcher.search(&query, &TopDocs::with_limit(MAX_SESSION_MESSAGES))?;
        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            results.push(self.doc_to_result(&searcher.doc(doc_address)?, score, "")?);
        }
        results.sort_by_key(|result| result.sequence_num);
        Ok(results)
    }

    fn get_session_messages_matching(
        &self,
        source: Option<Source>,
        session_id: &str,
    ) -> Result<Vec<SearchResult>> {
        let searcher = self
            .reader
            .searcher();

        // Use TermQuery on each UUID segment for exact matching
        // Session IDs are UUIDs like "9e1e6a58-cd5a-4651-a9fd-c24c04cb8809"
        // TEXT field tokenizes at hyphens, so we match all segments with AND
        let segments: Vec<_> = session_id
            .split('-')
            .collect();
        let segment_queries: Vec<_> = segments
            .iter()
            .map(|seg| {
                let term = Term::from_field_text(self.session_field, seg);
                (
                    Occur::Must,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                        as Box<dyn tantivy::query::Query>,
                )
            })
            .collect();
        let mut query_parts = segment_queries;
        if let Some(source) = source {
            query_parts.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.source_field, source.as_str()),
                    IndexRecordOption::Basic,
                )),
            ));
        }
        let query = BooleanQuery::new(query_parts);

        let top_docs = searcher.search(&query, &TopDocs::with_limit(MAX_SESSION_MESSAGES))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let result = self.doc_to_result(&searcher.doc(doc_address)?, score, "")?;
            // Filter to session_id match - support prefix matching for short IDs
            if (result.session_id == session_id
                || result
                    .session_id
                    .starts_with(session_id))
                && source.is_none_or(|expected| result.source == expected)
            {
                results.push(result);
            }
        }

        // Sort by sequence number
        results.sort_by_key(|r| r.sequence_num);

        Ok(results)
    }

    /// Get specific messages by their UUIDs
    pub fn get_messages_by_uuid(&self, uuids: &[String]) -> Result<Vec<SearchResult>> {
        self.get_messages_by_uuid_matching(None, uuids)
    }

    pub fn get_messages_by_uuid_for_source(
        &self,
        source: Source,
        uuids: &[String],
    ) -> Result<Vec<SearchResult>> {
        self.get_messages_by_uuid_matching(Some(source), uuids)
    }

    /// Retrieve exact records using the same source/session/message identity
    /// used for deduplication. This is the safest follow-up to a search result.
    pub fn get_messages_by_uuid_for_conversation(
        &self,
        source: Source,
        session_id: &str,
        uuids: &[String],
    ) -> Result<Vec<SearchResult>> {
        let searcher = self
            .reader
            .searcher();
        let mut results = Vec::new();
        for uuid in uuids {
            let query = TermQuery::new(
                Term::from_field_text(self.document_key_field, &source.doc_key(session_id, uuid)),
                IndexRecordOption::Basic,
            );
            let docs = searcher.search(&query, &TopDocs::with_limit(1))?;
            if let Some((score, address)) = docs
                .into_iter()
                .next()
            {
                results.push(self.doc_to_result(&searcher.doc(address)?, score, "")?);
            }
        }
        Ok(results)
    }

    fn get_messages_by_uuid_matching(
        &self,
        source: Option<Source>,
        uuids: &[String],
    ) -> Result<Vec<SearchResult>> {
        let searcher = self
            .reader
            .searcher();
        let mut results = Vec::new();

        for uuid in uuids {
            // UUID is stored as TEXT, tokenized at hyphens
            let segments: Vec<_> = uuid
                .split('-')
                .collect();
            let mut segment_queries: Vec<_> = segments
                .iter()
                .map(|seg| {
                    let term = Term::from_field_text(self.uuid_field, seg);
                    (
                        Occur::Must,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                            as Box<dyn tantivy::query::Query>,
                    )
                })
                .collect();
            if let Some(source) = source {
                segment_queries.push((
                    Occur::Must,
                    Box::new(TermQuery::new(
                        Term::from_field_text(self.source_field, source.as_str()),
                        IndexRecordOption::Basic,
                    )),
                ));
            }
            let query = BooleanQuery::new(segment_queries);

            let top_docs = searcher.search(&query, &TopDocs::with_limit(10))?;

            let mut matches = Vec::new();
            for (score, doc_address) in top_docs {
                let result = self.doc_to_result(&searcher.doc(doc_address)?, score, "")?;
                // Exact match or prefix match
                if result.uuid == *uuid
                    || result
                        .uuid
                        .starts_with(uuid)
                {
                    matches.push(result);
                }
            }
            let sources: std::collections::HashSet<_> = matches
                .iter()
                .map(|result| result.source)
                .collect();
            if source.is_none() && sources.len() > 1 {
                anyhow::bail!(
                    "Ambiguous message ID '{}'; select source=claude or source=codex",
                    uuid
                );
            }
            if matches.len() > 1 {
                anyhow::bail!(
                    "Ambiguous message ID '{}'; provide source and session_id for exact retrieval",
                    uuid
                );
            }
            if let Some(result) = matches
                .into_iter()
                .next()
            {
                results.push(result);
            }
        }

        Ok(results)
    }

    fn doc_to_result(
        &self,
        doc: &TantivyDocument,
        score: f32,
        query_text: &str,
    ) -> Result<SearchResult> {
        let uuid = doc
            .get_first(self.uuid_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let parent_uuid = doc
            .get_first(self.parent_uuid_field)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let content = doc
            .get_first(self.content_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let project = doc
            .get_first(self.project_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let project_path = doc
            .get_first(self.cwd_field)
            .and_then(|v| v.as_str())
            .unwrap_or(&project)
            .to_string();

        let session_id = doc
            .get_first(self.session_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let timestamp = doc
            .get_first(self.timestamp_field)
            .and_then(|v| v.as_datetime())
            .map(|dt| {
                DateTime::from_timestamp_millis(dt.into_timestamp_millis()).unwrap_or_else(Utc::now)
            })
            .unwrap_or_else(Utc::now);

        let message_type = doc
            .get_first(self.message_type_field)
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let snippet = if query_text.is_empty() {
            truncate_content(&content, 150, false)
        } else {
            self.generate_snippet(&content, query_text)
        };

        let technologies = doc
            .get_first(self.technologies_field)
            .and_then(|v| v.as_str())
            .map(|s| {
                s.split_whitespace()
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let code_languages = doc
            .get_first(self.code_languages_field)
            .and_then(|v| v.as_str())
            .map(|s| {
                s.split_whitespace()
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let tools_mentioned = doc
            .get_first(self.tools_mentioned_field)
            .and_then(|v| v.as_str())
            .map(|s| {
                s.split_whitespace()
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let has_code = doc
            .get_first(self.has_code_field)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let has_error = doc
            .get_first(self.has_error_field)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let sequence_num = doc
            .get_first(self.sequence_num_field)
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let is_sidechain = doc
            .get_first(self.is_sidechain_field)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let agent_id = doc
            .get_first(self.agent_id_field)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let source_str = doc
            .get_first(self.source_field)
            .and_then(|v| v.as_str())
            .unwrap_or("claude");
        let source = source_str
            .parse()
            .unwrap_or(super::source::Source::Claude);

        let source_artifact = doc
            .get_first(self.source_artifact_field)
            .and_then(|value| value.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_default();

        let interaction_count = self.get_interaction_count(source, &session_id);

        Ok(SearchResult {
            source,
            uuid,
            parent_uuid,
            content,
            project,
            project_path,
            session_id,
            source_artifact,
            timestamp,
            score,
            snippet,
            technologies,
            code_languages,
            tools_mentioned,
            has_code,
            has_error,
            interaction_count,
            sequence_num,
            is_sidechain,
            agent_id,
            message_type,
        })
    }

    fn generate_snippet(&self, content: &str, query: &str) -> String {
        let words: Vec<&str> = content
            .split_whitespace()
            .collect();
        let query_words: Vec<&str> = query
            .split_whitespace()
            .collect();

        if words.len() <= 30 {
            return content.to_string();
        }

        let mut best_start = 0;
        let mut best_score = 0;

        for (i, window) in words
            .windows(30)
            .enumerate()
        {
            let window_text = window.join(" ");
            let mut score = 0;

            for query_word in &query_words {
                if window_text
                    .to_lowercase()
                    .contains(&query_word.to_lowercase())
                {
                    score += 1;
                }
            }

            if score > best_score {
                best_score = score;
                best_start = i;
            }
        }

        let snippet_words = &words[best_start..std::cmp::min(best_start + 30, words.len())];
        let mut snippet = snippet_words.join(" ");

        if best_start > 0 {
            snippet = format!("...{snippet}");
        }
        if best_start + 30 < words.len() {
            snippet = format!("{snippet}...");
        }

        snippet
    }

    fn get_interaction_count(&self, source: Source, session_id: &str) -> usize {
        self.interaction_counts
            .get(&source.conversation_key(session_id))
            .copied()
            .unwrap_or(0)
    }

    pub fn get_all_documents(
        &self,
        project_filter: Option<String>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let searcher = self
            .reader
            .searcher();

        let query: Box<dyn tantivy::query::Query> = if let Some(ref project_filter) = project_filter
        {
            build_project_query(self.project_field, project_filter)
        } else {
            Box::new(tantivy::query::AllQuery)
        };

        let top_docs = searcher.search(&*query, &TopDocs::with_limit(limit))?;

        let mut results = Vec::new();
        for (_score, doc_address) in top_docs {
            let result = self.doc_to_result(&searcher.doc(doc_address)?, 1.0, "")?;

            if let Some(ref project_filter) = project_filter
                && !project_matches(&result.project_path, project_filter)
            {
                continue;
            }

            results.push(result);
        }

        Ok(results)
    }
}

/// Search result with surrounding context messages
#[derive(Debug, Clone)]
pub struct SearchResultWithContext {
    pub matched_message: SearchResult,
    pub context_messages: Vec<SearchResult>,
    pub match_index: usize,
    pub total_session_messages: usize,
}

/// Options for what to include in search result display
#[derive(Debug, Clone)]
pub struct DisplayOptions {
    pub include_thinking: bool,
    pub include_tools: bool,
    /// Characters shown per message around match (0 = full content)
    pub truncate_length: usize,
}

impl Default for DisplayOptions {
    fn default() -> Self {
        Self {
            include_thinking: false,
            include_tools: false,
            truncate_length: 300,
        }
    }
}

/// Filter content based on display options
fn filter_content(s: &str, opts: &DisplayOptions) -> Option<String> {
    // Check if content should be hidden
    if !opts.include_thinking && s.starts_with("[thinking]") {
        return None;
    }
    if !opts.include_tools && super::models::looks_like_tool_record(s) {
        return None;
    }
    Some(s.to_string())
}

impl SearchResultWithContext {
    /// Format as grep -C style output - compact and dense
    /// Format: N. 📁 ~/path 🗒️ session (M msgs) 💬 msg_uuid
    ///            User: content preview...
    ///         »  AI: matched content...
    ///            User: content...
    pub fn format_compact(&self, index: usize) -> String {
        self.format_compact_with_options(index, &DisplayOptions::default())
    }

    /// Format with display options
    pub fn format_compact_with_options(&self, index: usize, opts: &DisplayOptions) -> String {
        let mut output = String::new();

        let project_path_full = &self
            .matched_message
            .project_path;
        let project_path_display = self
            .matched_message
            .project_path_display();
        let session_id = &self
            .matched_message
            .session_id;

        let jsonl_path = if self
            .matched_message
            .source_artifact
            .as_os_str()
            .is_empty()
        {
            session_jsonl_path(project_path_full, session_id).unwrap_or_default()
        } else {
            self.matched_message
                .source_artifact
                .clone()
        };
        let jsonl_path_str = jsonl_path.to_string_lossy();

        let short_session = short_uuid(session_id);
        let short_msg = short_uuid(
            &self
                .matched_message
                .uuid,
        );

        let path_link = file_hyperlink(project_path_full, &project_path_display);
        let session_link = file_hyperlink(&jsonl_path_str, short_session);

        output.push_str(&format!(
            "{}. [{}] 📁 {} 🗒️ {} ({} msgs) 💬 {} 📅 {}\n",
            index + 1,
            self.matched_message
                .source,
            path_link,
            session_link,
            self.total_session_messages,
            short_msg,
            self.matched_message
                .timestamp
                .format("%Y-%m-%d %H:%M"),
        ));
        output.push_str(&format!(
            "↪ {}\n",
            self.matched_message
                .source
                .resume_hint(session_id)
        ));

        let mut tags = Vec::new();
        tags.extend(
            self.matched_message
                .technologies
                .iter()
                .take(3)
                .cloned(),
        );
        tags.extend(
            self.matched_message
                .code_languages
                .iter()
                .take(2)
                .cloned(),
        );
        if self
            .matched_message
            .has_error
        {
            tags.push("error".to_string());
        }
        if !tags.is_empty() {
            output.push_str(&format!("🎟️{}\n", tags.join(",")));
        }

        self.format_context_messages(&mut output, opts);
        output
    }

    fn format_context_messages(&self, output: &mut String, opts: &DisplayOptions) {
        for (i, msg) in self
            .context_messages
            .iter()
            .enumerate()
        {
            // Filter content based on options
            // A tool record that caused the hit is evidence, not optional
            // context. Hide only neighboring tool noise unless requested.
            if i != self.match_index && filter_content(&msg.content, opts).is_none() {
                continue;
            }

            let prefix = if i == self.match_index { "»  " } else { "   " };
            let content = if opts.truncate_length == 0 {
                msg.content
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                truncate_content(&msg.content, opts.truncate_length, true)
            };

            output.push_str(&format!("{}{}: {}\n", prefix, msg.role_display(), content));
        }
    }

    /// Format with more detail for verbose output
    pub fn format_verbose(&self, index: usize) -> String {
        let mut output = String::new();

        output.push_str(&format!(
            "{}. [{}] {} | {} | score: {:.2}\n",
            index + 1,
            self.matched_message
                .project,
            self.matched_message
                .timestamp
                .format("%Y-%m-%d %H:%M"),
            short_uuid(
                &self
                    .matched_message
                    .session_id
            ),
            self.matched_message
                .score,
        ));
        output.push_str(&format!(
            "   {} msgs in session | uuid: {}\n",
            self.total_session_messages,
            short_uuid(
                &self
                    .matched_message
                    .uuid
            ),
        ));

        // Metadata tags on one line
        let mut tags = Vec::new();
        if !self
            .matched_message
            .technologies
            .is_empty()
        {
            tags.push(
                self.matched_message
                    .technologies
                    .join(","),
            );
        }
        if !self
            .matched_message
            .code_languages
            .is_empty()
        {
            tags.push(
                self.matched_message
                    .code_languages
                    .join(","),
            );
        }
        if self
            .matched_message
            .has_code
        {
            tags.push("code".to_string());
        }
        if self
            .matched_message
            .has_error
        {
            tags.push("error".to_string());
        }
        if !tags.is_empty() {
            output.push_str(&format!("   tags: {}\n", tags.join(" ")));
        }

        // Context messages
        let default_opts = DisplayOptions::default();
        for (i, msg) in self
            .context_messages
            .iter()
            .enumerate()
        {
            let prefix = if i == self.match_index { ">> " } else { "   " };
            let content = truncate_content(&msg.content, default_opts.truncate_length, true);
            output.push_str(&format!("{}{}: {}\n", prefix, msg.role_display(), content));
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::indexer::SearchIndexer;
    use crate::shared::models::{ConversationEntry, MessageType};
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_entry(
        uuid: &str,
        session_id: &str,
        msg_type: MessageType,
        content: &str,
        seq: usize,
    ) -> ConversationEntry {
        ConversationEntry {
            source: crate::shared::Source::Claude,
            uuid: uuid.to_string(),
            parent_uuid: None,
            session_id: session_id.to_string(),
            source_artifact: format!("/fixtures/claude/{session_id}.jsonl").into(),
            project_path: "/test/project".to_string(),
            timestamp: Utc::now(),
            message_type: msg_type,
            record_kind: crate::shared::RecordKind::Conversation,
            content: content.to_string(),
            model: None,
            cwd: None,
            sequence_num: seq,
            is_sidechain: false,
            agent_id: None,
            technologies: vec![],
            has_code: false,
            code_languages: vec![],
            has_error: false,
            tools_mentioned: vec![],
        }
    }

    #[test]
    fn test_get_session_messages_returns_all_indexed() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path();

        // Create 100 messages for a session
        let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let entries: Vec<_> = (0..100)
            .map(|i| {
                let msg_type = if i % 2 == 0 {
                    MessageType::User
                } else {
                    MessageType::Assistant
                };
                make_entry(
                    &format!("uuid-{:04}", i),
                    session_id,
                    msg_type,
                    &format!("Message {}", i),
                    i,
                )
            })
            .collect();

        // Index them
        let mut indexer = SearchIndexer::new(index_path).unwrap();
        indexer
            .index_conversations(entries)
            .unwrap();
        indexer
            .commit()
            .unwrap();
        drop(indexer);

        // Retrieve with SearchEngine
        let engine = SearchEngine::new(index_path, HashMap::new()).unwrap();
        let messages = engine
            .get_session_messages(session_id)
            .unwrap();

        assert_eq!(
            messages.len(),
            100,
            "Should retrieve all 100 indexed messages"
        );
    }

    #[test]
    fn test_get_session_messages_with_short_id() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path();

        let session_id = "12345678-abcd-efgh-ijkl-mnopqrstuvwx";
        let entries = vec![
            make_entry("uuid-1", session_id, MessageType::User, "Hello", 0),
            make_entry("uuid-2", session_id, MessageType::Assistant, "Hi there", 1),
        ];

        let mut indexer = SearchIndexer::new(index_path).unwrap();
        indexer
            .index_conversations(entries)
            .unwrap();
        indexer
            .commit()
            .unwrap();
        drop(indexer);

        let engine = SearchEngine::new(index_path, HashMap::new()).unwrap();

        // Test with short ID (first 8 chars)
        let messages = engine
            .get_session_messages("12345678")
            .unwrap();
        assert_eq!(
            messages.len(),
            2,
            "Should find messages with short session ID"
        );
    }

    fn make_entry_with_project(
        uuid: &str,
        session_id: &str,
        msg_type: MessageType,
        content: &str,
        seq: usize,
        project_name: &str,
        cwd: &str,
    ) -> ConversationEntry {
        ConversationEntry {
            source: crate::shared::Source::Claude,
            uuid: uuid.to_string(),
            parent_uuid: None,
            session_id: session_id.to_string(),
            source_artifact: format!("/fixtures/claude/{session_id}.jsonl").into(),
            project_path: project_name.to_string(),
            timestamp: Utc::now(),
            message_type: msg_type,
            record_kind: crate::shared::RecordKind::Conversation,
            content: content.to_string(),
            model: None,
            cwd: Some(cwd.to_string()),
            sequence_num: seq,
            is_sidechain: false,
            agent_id: None,
            technologies: vec![],
            has_code: false,
            code_languages: vec![],
            has_error: false,
            tools_mentioned: vec![],
        }
    }

    #[test]
    fn test_project_filter_with_full_path() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path();

        let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let entries = vec![
            make_entry_with_project(
                "uuid-1",
                session_id,
                MessageType::User,
                "hello world",
                0,
                "my-cool-project",
                "/home/user/GIT/my-cool-project",
            ),
            make_entry_with_project(
                "uuid-2",
                session_id,
                MessageType::Assistant,
                "hi there",
                1,
                "my-cool-project",
                "/home/user/GIT/my-cool-project",
            ),
            make_entry_with_project(
                "uuid-3",
                session_id,
                MessageType::User,
                "other stuff",
                2,
                "other-project",
                "/home/user/GIT/other-project",
            ),
        ];

        let mut indexer = SearchIndexer::new(index_path).unwrap();
        indexer
            .index_conversations(entries)
            .unwrap();
        indexer
            .commit()
            .unwrap();
        drop(indexer);

        let engine = SearchEngine::new(index_path, HashMap::new()).unwrap();

        // Filter by full path (how users pass --project)
        let results = engine
            .search(SearchQuery {
                text: "hello".to_string(),
                limit: 10,
                project_filter: Some("/home/user/GIT/my-cool-project".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(
            results.len(),
            1,
            "Should find 1 result with full path project filter"
        );
        assert_eq!(results[0].uuid, "uuid-1");

        // Filter by short project name
        let results = engine
            .search(SearchQuery {
                text: "hello".to_string(),
                limit: 10,
                project_filter: Some("my-cool-project".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(
            results.len(),
            1,
            "Should find 1 result with short project name filter"
        );

        // Filter should exclude non-matching projects
        let results = engine
            .search(SearchQuery {
                text: "hello".to_string(),
                limit: 10,
                project_filter: Some("other-project".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(results.len(), 0, "Should find 0 results for wrong project");
    }

    #[test]
    fn test_project_filter_get_all_documents() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path();

        let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let entries = vec![
            make_entry_with_project(
                "uuid-1",
                session_id,
                MessageType::User,
                "hello",
                0,
                "freeswitch-database_utils",
                "/mnt/bcachefs/@home/user/GIT/freeswitch-database_utils",
            ),
            make_entry_with_project(
                "uuid-2",
                session_id,
                MessageType::User,
                "world",
                1,
                "agent-recall",
                "/mnt/bcachefs/@home/user/GIT/agent-recall",
            ),
        ];

        let mut indexer = SearchIndexer::new(index_path).unwrap();
        indexer
            .index_conversations(entries)
            .unwrap();
        indexer
            .commit()
            .unwrap();
        drop(indexer);

        let engine = SearchEngine::new(index_path, HashMap::new()).unwrap();

        let results = engine
            .get_all_documents(
                Some("/mnt/bcachefs/@home/user/GIT/freeswitch-database_utils".to_string()),
                10,
            )
            .unwrap();

        assert_eq!(
            results.len(),
            1,
            "Should find 1 document for freeswitch-database_utils"
        );
    }

    #[test]
    fn test_session_filter_with_full_uuid() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path();

        let session_a = "aaaaaaaa-1111-2222-3333-444444444444";
        let session_b = "bbbbbbbb-5555-6666-7777-888888888888";
        let entries = vec![
            make_entry("uuid-1", session_a, MessageType::User, "hello world", 0),
            make_entry("uuid-2", session_b, MessageType::User, "hello world", 0),
        ];

        let mut indexer = SearchIndexer::new(index_path).unwrap();
        indexer
            .index_conversations(entries)
            .unwrap();
        indexer
            .commit()
            .unwrap();
        drop(indexer);

        let engine = SearchEngine::new(index_path, HashMap::new()).unwrap();

        // Full session ID
        let results = engine
            .search(SearchQuery {
                text: "hello".to_string(),
                limit: 10,
                session_filter: Some(session_a.to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(results.len(), 1, "Should find 1 result for session A");
        assert_eq!(results[0].uuid, "uuid-1");

        // Short prefix
        let results = engine
            .search(SearchQuery {
                text: "hello".to_string(),
                limit: 10,
                session_filter: Some("aaaaaaaa".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(
            results.len(),
            1,
            "Should find 1 result with short session prefix"
        );
        assert_eq!(results[0].uuid, "uuid-1");
    }

    #[test]
    fn test_get_session_messages_by_prefix() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path();

        let session_id = "aabbccdd-1122-3344-5566-778899001122";
        let entries = vec![
            make_entry("uuid-1", session_id, MessageType::User, "first", 0),
            make_entry("uuid-2", session_id, MessageType::Assistant, "second", 1),
            make_entry("uuid-3", session_id, MessageType::User, "third", 2),
        ];

        let mut indexer = SearchIndexer::new(index_path).unwrap();
        indexer
            .index_conversations(entries)
            .unwrap();
        indexer
            .commit()
            .unwrap();
        drop(indexer);

        let engine = SearchEngine::new(index_path, HashMap::new()).unwrap();

        // Full ID
        let messages = engine
            .get_session_messages(session_id)
            .unwrap();
        assert_eq!(messages.len(), 3);

        // Short prefix
        let messages = engine
            .get_session_messages("aabbccdd")
            .unwrap();
        assert_eq!(
            messages.len(),
            3,
            "Should find all messages with short prefix"
        );

        // Non-matching prefix
        let messages = engine
            .get_session_messages("xxxxxxxx")
            .unwrap();
        assert_eq!(
            messages.len(),
            0,
            "Should find no messages for wrong prefix"
        );
    }

    #[test]
    fn test_displayable_count_matches_retrieval() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path();

        let session_id = "testtest-1234-5678-abcd-ef0123456789";
        let entries = vec![
            make_entry("uuid-1", session_id, MessageType::User, "User message", 0),
            make_entry(
                "uuid-2",
                session_id,
                MessageType::Assistant,
                "Assistant message",
                1,
            ),
            make_entry(
                "uuid-3",
                session_id,
                MessageType::System,
                "System message",
                2,
            ),
            make_entry("uuid-4", session_id, MessageType::Summary, "Summary", 3),
            make_entry("uuid-5", session_id, MessageType::User, "Warmup", 4), // Should be filtered
        ];

        let mut indexer = SearchIndexer::new(index_path).unwrap();
        indexer
            .index_conversations(entries)
            .unwrap();
        indexer
            .commit()
            .unwrap();
        drop(indexer);

        let engine = SearchEngine::new(index_path, HashMap::new()).unwrap();
        let messages = engine
            .get_session_messages(session_id)
            .unwrap();

        // Count displayable
        let displayable_count = messages
            .iter()
            .filter(|m| m.is_displayable())
            .count();
        // User, Assistant, Summary are displayable; System is not; "Warmup" content filtered
        assert_eq!(
            displayable_count, 3,
            "Should have 3 displayable messages (User, Assistant, Summary)"
        );
    }

    #[test]
    fn compact_context_skips_neighboring_tools_but_keeps_a_tool_match() {
        let temp_dir = TempDir::new().unwrap();
        let session_id = "tool-context-session";
        let entries = vec![
            make_entry(
                "uuid-user-before",
                session_id,
                MessageType::User,
                "Check how production access works",
                0,
            ),
            make_entry(
                "uuid-tool-call",
                session_id,
                MessageType::Assistant,
                "[tool:database.query]\nSELECT access_method FROM audit_log",
                1,
            ),
            make_entry(
                "uuid-tool-result",
                session_id,
                MessageType::Assistant,
                "[tool_result:database.query]\nservice-account",
                2,
            ),
            make_entry(
                "uuid-assistant",
                session_id,
                MessageType::Assistant,
                "The evidence shows a service account.",
                3,
            ),
            make_entry(
                "uuid-user-after",
                session_id,
                MessageType::User,
                "Record that decision.",
                4,
            ),
        ];
        let mut indexer = SearchIndexer::new(temp_dir.path()).unwrap();
        indexer
            .index_conversations(entries)
            .unwrap();
        indexer
            .commit()
            .unwrap();
        drop(indexer);

        let engine = SearchEngine::new(temp_dir.path(), HashMap::new()).unwrap();
        let conversational = engine
            .search_with_context(
                SearchQuery {
                    text: "evidence".to_string(),
                    limit: 10,
                    ..Default::default()
                },
                1,
                1,
            )
            .unwrap();
        assert_eq!(
            conversational[0]
                .context_messages
                .len(),
            3
        );
        assert_eq!(
            conversational[0].context_messages[0].uuid,
            "uuid-user-before"
        );
        assert_eq!(
            conversational[0].context_messages[2].uuid,
            "uuid-user-after"
        );

        let technical = engine
            .search_with_context(
                SearchQuery {
                    text: "SELECT".to_string(),
                    limit: 10,
                    ..Default::default()
                },
                1,
                1,
            )
            .unwrap();
        assert_eq!(
            technical[0]
                .matched_message
                .uuid,
            "uuid-tool-call"
        );
        assert!(
            technical[0]
                .format_compact(0)
                .contains("SELECT access_method")
        );
        assert_eq!(
            technical[0]
                .context_messages
                .len(),
            3
        );
    }
}
