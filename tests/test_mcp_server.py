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
