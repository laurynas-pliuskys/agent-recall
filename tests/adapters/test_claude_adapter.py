import json
import tempfile
from pathlib import Path
from conversation_search.adapters.claude import ClaudeAdapter


def _write_jsonl(path: Path, lines: list) -> None:
    with open(path, "w") as f:
        for line in lines:
            f.write(json.dumps(line) + "\n")


def test_parse_returns_meta_and_messages(tmp_path):
    conv_file = tmp_path / "session1.jsonl"
    _write_jsonl(conv_file, [
        {"type": "summary", "summary": "A test session", "leafUuid": "leaf-uuid-1"},
        {
            "uuid": "msg-1",
            "parentUuid": None,
            "isSidechain": False,
            "timestamp": "2024-01-15T10:00:00Z",
            "type": "user",
            "sessionId": "session-abc",
            "message": {"content": "Hello, how are you?"},
        },
        {
            "uuid": "msg-2",
            "parentUuid": "msg-1",
            "isSidechain": False,
            "timestamp": "2024-01-15T10:00:01Z",
            "type": "ai",
            "sessionId": "session-abc",
            "message": {"content": [{"type": "text", "text": "I am doing well!"}]},
        },
    ])
    adapter = ClaudeAdapter()
    meta, messages = adapter.parse(conv_file)

    assert meta.session_id == "session-abc"
    assert meta.source == "claude"
    assert meta.summary == "A test session"
    assert meta.leaf_uuid == "leaf-uuid-1"
    assert len(messages) == 2

    user_msg = messages[0]
    assert user_msg.uuid == "msg-1"
    assert user_msg.role == "user"
    assert user_msg.content == "Hello, how are you?"
    assert user_msg.source == "claude"
    assert user_msg.session_id == "session-abc"
    assert user_msg.is_sidechain is False

    ai_msg = messages[1]
    assert ai_msg.uuid == "msg-2"
    assert ai_msg.role == "ai"
    assert "I am doing well!" in ai_msg.content


def test_parse_flattens_tool_use_blocks(tmp_path):
    conv_file = tmp_path / "session2.jsonl"
    _write_jsonl(conv_file, [
        {"type": "summary", "summary": "Tool use session", "leafUuid": "leaf-2"},
        {
            "uuid": "msg-3",
            "parentUuid": None,
            "isSidechain": False,
            "timestamp": "2024-01-15T11:00:00Z",
            "type": "ai",
            "sessionId": "session-def",
            "message": {
                "content": [
                    {"type": "text", "text": "Let me check that."},
                    {"type": "tool_use", "name": "Bash", "input": {"command": "ls -la"}},
                ]
            },
        },
    ])
    adapter = ClaudeAdapter()
    meta, messages = adapter.parse(conv_file)

    assert len(messages) == 1
    content = messages[0].content
    assert "Let me check that." in content
    assert "[Tool: Bash]" in content
    assert "ls -la" in content


def test_parse_skips_non_user_ai_types(tmp_path):
    conv_file = tmp_path / "session3.jsonl"
    _write_jsonl(conv_file, [
        {"type": "summary", "summary": "Mixed types", "leafUuid": "leaf-3"},
        {
            "uuid": "msg-4",
            "parentUuid": None,
            "isSidechain": False,
            "timestamp": "2024-01-15T12:00:00Z",
            "type": "user",
            "sessionId": "session-ghi",
            "message": {"content": "A real user message"},
        },
        {
            "uuid": "msg-5",
            "parentUuid": "msg-4",
            "isSidechain": False,
            "timestamp": "2024-01-15T12:00:01Z",
            "type": "tool",
            "sessionId": "session-ghi",
            "message": {"content": "some tool output"},
        },
    ])
    adapter = ClaudeAdapter()
    meta, messages = adapter.parse(conv_file)
    assert len(messages) == 1
    assert messages[0].uuid == "msg-4"


def test_parse_handles_missing_summary_line(tmp_path):
    conv_file = tmp_path / "session4.jsonl"
    _write_jsonl(conv_file, [
        {
            "uuid": "msg-6",
            "parentUuid": None,
            "isSidechain": False,
            "timestamp": "2024-01-15T13:00:00Z",
            "type": "user",
            "sessionId": "session-jkl",
            "message": {"content": "No summary line here"},
        },
    ])
    adapter = ClaudeAdapter()
    meta, messages = adapter.parse(conv_file)

    assert meta.summary is None
    assert meta.leaf_uuid is None
    assert len(messages) == 1


def test_scan_respects_days_back(tmp_path, monkeypatch):
    import os
    from datetime import datetime, timedelta

    projects_dir = tmp_path / ".claude" / "projects" / "myproject"
    projects_dir.mkdir(parents=True)

    recent = projects_dir / "recent.jsonl"
    recent.write_text('{"type": "summary"}\n')
    old = projects_dir / "old.jsonl"
    old.write_text('{"type": "summary"}\n')

    old_time = (datetime.now() - timedelta(days=10)).timestamp()
    os.utime(old, (old_time, old_time))

    monkeypatch.setattr(Path, "home", lambda: tmp_path)

    adapter = ClaudeAdapter()
    paths = adapter.scan(days_back=7)

    assert recent in paths
    assert old not in paths
