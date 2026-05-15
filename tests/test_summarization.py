import pytest
from agent_recall.core.indexer import ConversationIndexer
from agent_recall.core.summarization import MessageSummarizer


@pytest.fixture
def db_with_message(tmp_path):
    db_path = str(tmp_path / "test.db")
    indexer = ConversationIndexer(db_path=db_path, quiet=True)
    indexer.conn.execute("""
        INSERT INTO messages
            (message_uuid, session_id, timestamp, message_type, full_content, source)
        VALUES ('uuid-1', 'sess-1', '2026-01-01T00:00:00Z', 'user', 'hello world', 'claude')
    """)
    indexer.conn.commit()
    indexer.close()
    return db_path


def test_update_database_default_method_is_in_schema_allowed_set(db_with_message):
    """Default method stored by update_database must be one of the documented values.
    'smart_extraction' is not listed in the schema CHECK constraint."""
    import sqlite3

    summarizer = MessageSummarizer(db_path=db_with_message)
    count = summarizer.update_database([{"uuid": "uuid-1", "summary": "extracted text"}])
    assert count == 1

    conn = sqlite3.connect(db_with_message)
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    cursor.execute("SELECT summary_method FROM messages WHERE message_uuid = 'uuid-1'")
    row = cursor.fetchone()
    conn.close()

    assert row["summary_method"] != "smart_extraction"


def test_is_tool_noise_detects_short_ai_acknowledgment():
    """Short acknowledgments from the AI role should be detected as tool noise."""
    summarizer = MessageSummarizer()
    msg = {"message_type": "ai", "content": "Let me check that for you."}
    assert summarizer.is_tool_noise(msg) is True


def test_is_tool_noise_detects_ai_looking_at():
    """'Looking at' phrasing from the AI role should be tool noise."""
    summarizer = MessageSummarizer()
    msg = {"message_type": "ai", "content": "Looking at the file now to understand the structure."}
    assert summarizer.is_tool_noise(msg) is True


def test_is_tool_noise_does_not_flag_user_messages():
    """User messages that happen to contain acknowledgment phrases should not be filtered."""
    summarizer = MessageSummarizer()
    msg = {"message_type": "user", "content": "Let me check that for you."}
    assert summarizer.is_tool_noise(msg) is False


def test_extract_batch_is_callable():
    """extract_batch must exist; it is referenced by search.py --summarize path."""
    summarizer = MessageSummarizer()
    messages = [{"uuid": "x", "message_type": "user", "content": "hello " * 20}]
    result = summarizer.extract_batch(messages)
    assert isinstance(result, list)
    assert len(result) == 1
