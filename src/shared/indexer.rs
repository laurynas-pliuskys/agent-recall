use super::config::get_config;
use super::models::ConversationEntry;
use super::source::Source;
use anyhow::Result;
use std::path::Path;
use tantivy::schema::{FAST, Field, INDEXED, STORED, STRING, Schema, SchemaBuilder, TEXT};
use tantivy::{Index, IndexWriter, Term, doc};

/// Current schema version - increment when schema changes to trigger rebuild
pub const SCHEMA_VERSION: u32 = 4;

pub struct IndexFields {
    pub uuid_field: Field,
    pub parent_uuid_field: Field,
    pub content_field: Field,
    pub project_field: Field,
    pub session_field: Field,
    pub timestamp_field: Field,
    pub message_type_field: Field,
    pub model_field: Field,
    pub technologies_field: Field,
    pub code_languages_field: Field,
    pub tools_mentioned_field: Field,
    pub has_code_field: Field,
    pub has_error_field: Field,
    pub cwd_field: Field,
    pub sequence_num_field: Field,
    pub is_sidechain_field: Field,
    pub agent_id_field: Field,
    pub source_field: Field,
    pub conversation_key_field: Field,
    pub document_key_field: Field,
    pub artifact_key_field: Field,
    pub source_artifact_field: Field,
}

pub struct SearchIndexer {
    writer: IndexWriter,
    fields: IndexFields,
}

impl SearchIndexer {
    /// Create the canonical schema - single source of truth
    pub fn build_schema() -> (Schema, IndexFields) {
        let mut schema_builder = SchemaBuilder::default();

        // Primary key for deduplication
        let uuid_field = schema_builder.add_text_field("uuid", TEXT | STORED | FAST);
        let parent_uuid_field = schema_builder.add_text_field("parent_uuid", TEXT | STORED | FAST);

        let content_field = schema_builder.add_text_field("content", TEXT | STORED);
        let project_field = schema_builder.add_text_field("project", TEXT | STORED | FAST);
        let session_field = schema_builder.add_text_field("session_id", TEXT | STORED | FAST);
        let timestamp_field = schema_builder.add_date_field("timestamp", INDEXED | STORED | FAST);
        let message_type_field =
            schema_builder.add_text_field("message_type", TEXT | STORED | FAST);
        let model_field = schema_builder.add_text_field("model", TEXT | STORED | FAST);
        let technologies_field =
            schema_builder.add_text_field("technologies", TEXT | STORED | FAST);
        let code_languages_field =
            schema_builder.add_text_field("code_languages", TEXT | STORED | FAST);
        let tools_mentioned_field =
            schema_builder.add_text_field("tools_mentioned", TEXT | STORED | FAST);
        let has_code_field = schema_builder.add_bool_field("has_code", INDEXED | STORED | FAST);
        let has_error_field = schema_builder.add_bool_field("has_error", INDEXED | STORED | FAST);
        let cwd_field = schema_builder.add_text_field("cwd", TEXT | STORED | FAST);
        let sequence_num_field =
            schema_builder.add_u64_field("sequence_num", INDEXED | STORED | FAST);
        let is_sidechain_field =
            schema_builder.add_bool_field("is_sidechain", INDEXED | STORED | FAST);
        let agent_id_field = schema_builder.add_text_field("agent_id", TEXT | STORED | FAST);
        let source_field = schema_builder.add_text_field("source", STRING | STORED | FAST);
        let conversation_key_field =
            schema_builder.add_text_field("conversation_key", STRING | STORED | FAST);
        let document_key_field =
            schema_builder.add_text_field("document_key", STRING | STORED | FAST);
        let artifact_key_field =
            schema_builder.add_text_field("artifact_key", STRING | STORED | FAST);
        let source_artifact_field = schema_builder.add_text_field("source_artifact", STORED);

        let schema = schema_builder.build();
        let fields = IndexFields {
            uuid_field,
            parent_uuid_field,
            content_field,
            project_field,
            session_field,
            timestamp_field,
            message_type_field,
            model_field,
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
            artifact_key_field,
            source_artifact_field,
        };

        (schema, fields)
    }

    /// Validate that an existing index matches our expected schema
    pub fn validate_schema(index_path: &Path) -> Result<bool> {
        let index = Index::open_in_dir(index_path)?;
        let actual_schema = index.schema();

        let (expected_schema, _) = Self::build_schema();
        Ok(actual_schema == expected_schema)
    }

    pub fn new(index_path: &Path) -> Result<Self> {
        let (schema, fields) = Self::build_schema();

        std::fs::create_dir_all(index_path)?;
        let index = Index::create_in_dir(index_path, schema)?;
        let config = get_config();
        let writer = index.writer(config.get_writer_heap_size())?;

        Ok(Self { writer, fields })
    }

    pub fn open(index_path: &Path) -> Result<Self> {
        let index = Index::open_in_dir(index_path)?;
        let schema = index.schema();

        // Get fields from the existing schema
        let fields = IndexFields {
            uuid_field: schema.get_field("uuid")?,
            parent_uuid_field: schema.get_field("parent_uuid")?,
            content_field: schema.get_field("content")?,
            project_field: schema.get_field("project")?,
            session_field: schema.get_field("session_id")?,
            timestamp_field: schema.get_field("timestamp")?,
            message_type_field: schema.get_field("message_type")?,
            model_field: schema.get_field("model")?,
            technologies_field: schema.get_field("technologies")?,
            code_languages_field: schema.get_field("code_languages")?,
            tools_mentioned_field: schema.get_field("tools_mentioned")?,
            has_code_field: schema.get_field("has_code")?,
            has_error_field: schema.get_field("has_error")?,
            cwd_field: schema.get_field("cwd")?,
            sequence_num_field: schema.get_field("sequence_num")?,
            is_sidechain_field: schema.get_field("is_sidechain")?,
            agent_id_field: schema.get_field("agent_id")?,
            source_field: schema.get_field("source")?,
            conversation_key_field: schema.get_field("conversation_key")?,
            document_key_field: schema.get_field("document_key")?,
            artifact_key_field: schema.get_field("artifact_key")?,
            source_artifact_field: schema.get_field("source_artifact")?,
        };

        let config = get_config();
        let writer = index.writer(config.get_writer_heap_size())?;

        Ok(Self { writer, fields })
    }

    /// Delete one source-qualified conversation before re-indexing.
    pub fn delete_conversation(&mut self, source: Source, session_id: &str) -> Result<()> {
        let term = Term::from_field_text(
            self.fields
                .conversation_key_field,
            &source.conversation_key(session_id),
        );
        self.writer
            .delete_term(term);
        Ok(())
    }

    /// Delete every document originating from one source artifact.
    pub fn delete_artifact(&mut self, source: Source, artifact: &Path) -> Result<()> {
        let artifact = artifact.to_string_lossy();
        let term = Term::from_field_text(
            self.fields
                .artifact_key_field,
            &source.artifact_key(&artifact),
        );
        self.writer
            .delete_term(term);
        Ok(())
    }

    pub fn index_conversations(&mut self, entries: Vec<ConversationEntry>) -> Result<()> {
        for entry in entries {
            let conversation_key = entry
                .source
                .conversation_key(&entry.session_id);
            let document_key = entry
                .source
                .doc_key(&entry.session_id, &entry.uuid);
            let artifact = entry
                .source_artifact
                .to_string_lossy();
            let artifact_key = entry
                .source
                .artifact_key(&artifact);
            let doc = doc!(
                self.fields.uuid_field => entry.uuid,
                self.fields.parent_uuid_field => entry.parent_uuid.unwrap_or_default(),
                self.fields.content_field => entry.content,
                self.fields.project_field => entry.project_path,
                self.fields.session_field => entry.session_id,
                self.fields.timestamp_field => tantivy::DateTime::from_timestamp_millis(entry.timestamp.timestamp_millis()),
                self.fields.message_type_field => format!("{:?}", entry.message_type),
                self.fields.model_field => entry.model.unwrap_or_else(|| "unknown".to_string()),
                self.fields.technologies_field => entry.technologies.join(" "),
                self.fields.code_languages_field => entry.code_languages.join(" "),
                self.fields.tools_mentioned_field => entry.tools_mentioned.join(" "),
                self.fields.has_code_field => entry.has_code,
                self.fields.has_error_field => entry.has_error,
                self.fields.cwd_field => entry.cwd.unwrap_or_else(|| "unknown".to_string()),
                self.fields.sequence_num_field => entry.sequence_num as u64,
                self.fields.is_sidechain_field => entry.is_sidechain,
                self.fields.agent_id_field => entry.agent_id.unwrap_or_default(),
                self.fields.source_field => entry.source.as_str(),
                self.fields.conversation_key_field => conversation_key,
                self.fields.document_key_field => document_key,
                self.fields.artifact_key_field => artifact_key,
                self.fields.source_artifact_field => artifact.as_ref(),
            );

            self.writer
                .add_document(doc)?;
        }

        Ok(())
    }

    pub fn commit(&mut self) -> Result<()> {
        self.writer
            .commit()?;
        Ok(())
    }
}
