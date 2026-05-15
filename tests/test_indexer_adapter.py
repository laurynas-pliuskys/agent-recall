import json
import sqlite3
from pathlib import Path
from agent_recall.adapters.claude import ClaudeAdapter
from agent_recall.core.indexer import ConversationIndexer


def _write_claude_jsonl(path: Path, session_id: str) -> None:
    with open(path, "w") as f:
        f.write(json.dumps({"type": "summary", "summary": "Test session", "leafUuid": "leaf1"}) + "\n")
        f.write(json.dumps({
            "uuid": "msg-u1",
            "parentUuid": None,
            "isSidechain": False,
            "timestamp": "2024-03-01T10:00:00Z",
            "type": "user",
            "sessionId": session_id,
            "message": {"content": "Hello from test"},
        }) + "\n")
        f.write(json.dumps({
            "uuid": "msg-a1",
            "parentUuid": "msg-u1",
            "isSidechain": False,
            "timestamp": "2024-03-01T10:00:01Z",
            "type": "ai",
            "sessionId": session_id,
            "message": {"content": [{"type": "text", "text": "Hello back!"}]},
        }) + "\n")


def test_indexer_uses_adapter_parse(tmp_path, monkeypatch):
    db_path = tmp_path / "index.db"

    projects_dir = tmp_path / ".claude" / "projects" / "myproject"
    projects_dir.mkdir(parents=True)
    conv_file = projects_dir / "session-test.jsonl"
    _write_claude_jsonl(conv_file, "session-test-id")

    monkeypatch.setattr(Path, "home", lambda: tmp_path)

    indexer = ConversationIndexer(
        db_path=str(db_path),
        quiet=True,
        adapters=[ClaudeAdapter()],
    )
    indexer.index_new(days_back=None)
    indexer.close()

    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    cursor.execute("SELECT * FROM messages WHERE session_id = 'session-test-id'")
    rows = cursor.fetchall()
    conn.close()

    assert len(rows) == 2
    assert all(row["source"] == "claude" for row in rows)


def test_indexer_stores_source_in_conversations(tmp_path, monkeypatch):
    db_path = tmp_path / "index.db"

    projects_dir = tmp_path / ".claude" / "projects" / "proj2"
    projects_dir.mkdir(parents=True)
    conv_file = projects_dir / "session-two.jsonl"
    _write_claude_jsonl(conv_file, "session-two-id")

    monkeypatch.setattr(Path, "home", lambda: tmp_path)

    indexer = ConversationIndexer(
        db_path=str(db_path),
        quiet=True,
        adapters=[ClaudeAdapter()],
    )
    indexer.index_new(days_back=None)
    indexer.close()

    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    cursor.execute("SELECT source FROM conversations WHERE session_id = 'session-two-id'")
    row = cursor.fetchone()
    conn.close()

    assert row["source"] == "claude"


def test_indexer_default_adapters_include_claude_and_gemini(tmp_path):
    db_path = tmp_path / "index.db"
    indexer = ConversationIndexer(db_path=str(db_path), quiet=True)
    source_names = [a.source for a in indexer.adapters]
    indexer.close()
    assert "claude" in source_names
    assert "gemini" in source_names
