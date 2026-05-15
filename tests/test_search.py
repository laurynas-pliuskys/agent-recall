import pytest
from agent_recall.core.indexer import ConversationIndexer
from agent_recall.core.search import ConversationSearch


@pytest.fixture
def db_orphaned_message(tmp_path):
    """DB with a message row but no matching conversations row."""
    db_path = str(tmp_path / "test.db")
    indexer = ConversationIndexer(db_path=db_path, quiet=True)
    indexer.conn.execute("""
        INSERT INTO messages
            (message_uuid, session_id, timestamp, message_type, full_content, source)
        VALUES ('orphan-uuid', 'orphan-sess', '2026-01-01T00:00:00Z',
                'user', 'orphan content', 'claude')
    """)
    indexer.conn.commit()
    indexer.close()
    return db_path


def test_get_conversation_context_orphaned_message(db_orphaned_message):
    """get_conversation_context must return an error dict, not crash, when the
    conversation metadata row is missing for an otherwise valid message UUID."""
    search = ConversationSearch(db_path=db_orphaned_message)
    try:
        result = search.get_conversation_context("orphan-uuid")
    finally:
        search.close()

    assert "error" in result
