from unittest.mock import patch
import pytest


def test_project_path_to_fs_hashed():
    from agent_recall.mcp_server import _project_path_to_fs
    assert _project_path_to_fs("home-user-myproject") == "/home/user/myproject"


def test_project_path_to_fs_already_absolute():
    from agent_recall.mcp_server import _project_path_to_fs
    assert _project_path_to_fs("/already/absolute") == "/already/absolute"


def test_resume_hint_claude():
    from agent_recall.mcp_server import _resume_hint
    assert _resume_hint("claude", "abc123") == "claude --resume abc123"


def test_resume_hint_gemini():
    from agent_recall.mcp_server import _resume_hint
    assert _resume_hint("gemini", "abc123") == "gemini --resume abc123"


def test_resume_hint_unknown_source():
    from agent_recall.mcp_server import _resume_hint
    assert _resume_hint("codex", "abc123") is None


def test_search_db_not_found(tmp_path):
    with patch("agent_recall.mcp_server.DB_PATH", str(tmp_path / "nonexistent.db")):
        from agent_recall.mcp_server import search
        result = search("query")
    assert isinstance(result, str)
    assert "agent-recall init" in result


def test_get_context_db_not_found(tmp_path):
    with patch("agent_recall.mcp_server.DB_PATH", str(tmp_path / "nonexistent.db")):
        from agent_recall.mcp_server import get_context
        result = get_context("some-uuid")
    assert isinstance(result, str)
    assert "agent-recall init" in result


def test_list_conversations_db_not_found(tmp_path):
    with patch("agent_recall.mcp_server.DB_PATH", str(tmp_path / "nonexistent.db")):
        from agent_recall.mcp_server import list_conversations
        result = list_conversations()
    assert isinstance(result, str)
    assert "agent-recall init" in result


from agent_recall.core.indexer import ConversationIndexer


@pytest.fixture
def test_db(tmp_path):
    db_path = str(tmp_path / "test.db")
    indexer = ConversationIndexer(db_path=db_path, quiet=True)
    indexer.conn.execute("""
        INSERT INTO conversations
            (session_id, project_path, source, conversation_summary,
             last_message_at, first_message_at, message_count)
        VALUES ('sess1', 'home-user-project', 'claude', 'Test conversation',
                '2026-05-14T10:00:00Z', '2026-05-14T09:00:00Z', 1)
    """)
    indexer.conn.execute("""
        INSERT INTO messages
            (message_uuid, session_id, timestamp, message_type,
             project_path, full_content, source)
        VALUES ('uuid-1', 'sess1', '2026-05-14T10:00:00Z', 'user',
                'home-user-project', 'Test message about authentication bug', 'claude')
    """)
    indexer.conn.commit()
    indexer.close()
    return db_path


def test_search_returns_fragment_shape(test_db):
    with patch("agent_recall.mcp_server.DB_PATH", test_db):
        from agent_recall.mcp_server import search
        results = search("authentication")
    assert len(results) == 1
    r = results[0]
    assert r["source"] == "claude"
    assert r["session_id"] == "sess1"
    assert r["project_path"] == "/home/user/project"
    assert r["role"] == "user"
    assert "message_uuid" in r
    assert "snippet" in r
    assert "ts" in r


def test_search_returns_empty_for_no_match(test_db):
    with patch("agent_recall.mcp_server.DB_PATH", test_db):
        from agent_recall.mcp_server import search
        results = search("xyzzy_no_match")
    assert results == []


def test_list_conversations_returns_resume_hint(test_db):
    with patch("agent_recall.mcp_server.DB_PATH", test_db):
        from agent_recall.mcp_server import list_conversations
        results = list_conversations()
    assert len(results) == 1
    assert results[0]["resume_hint"] == "claude --resume sess1"
    assert results[0]["conversation_summary"] == "Test conversation"


def test_get_context_returns_message(test_db):
    with patch("agent_recall.mcp_server.DB_PATH", test_db):
        from agent_recall.mcp_server import get_context
        result = get_context("uuid-1")
    assert "message" in result
    assert result["message"]["message_uuid"] == "uuid-1"
