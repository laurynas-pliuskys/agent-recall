import sqlite3
from pathlib import Path
from agent_recall.core.search import ConversationSearch


def _build_db(db_path: str):
    """Create a minimal test DB with messages from two sources."""
    conn = sqlite3.connect(db_path)
    conn.executescript("""
        PRAGMA journal_mode=WAL;
        CREATE TABLE messages (
            message_uuid TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            parent_uuid TEXT,
            is_sidechain BOOLEAN DEFAULT FALSE,
            depth INTEGER DEFAULT 0,
            timestamp TEXT NOT NULL,
            message_type TEXT NOT NULL,
            project_path TEXT,
            conversation_file TEXT,
            summary TEXT,
            full_content TEXT NOT NULL DEFAULT '',
            is_summarized BOOLEAN DEFAULT FALSE,
            is_tool_noise BOOLEAN DEFAULT FALSE,
            is_meta_conversation BOOLEAN DEFAULT FALSE,
            summary_method TEXT,
            indexed_at TEXT DEFAULT CURRENT_TIMESTAMP,
            source TEXT NOT NULL DEFAULT 'claude'
        );
        CREATE VIRTUAL TABLE message_content_fts USING fts5(
            message_uuid UNINDEXED,
            full_content,
            content='messages',
            content_rowid='rowid'
        );
        CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
            INSERT INTO message_content_fts(rowid, message_uuid, full_content)
            VALUES (new.rowid, new.message_uuid, new.full_content);
        END;
        CREATE TABLE conversations (
            session_id TEXT PRIMARY KEY,
            project_path TEXT,
            conversation_file TEXT,
            root_message_uuid TEXT,
            leaf_message_uuid TEXT,
            conversation_summary TEXT,
            first_message_at TEXT,
            last_message_at TEXT,
            message_count INTEGER DEFAULT 0,
            indexed_at TEXT DEFAULT CURRENT_TIMESTAMP,
            source TEXT NOT NULL DEFAULT 'claude'
        );
    """)
    conn.executemany(
        "INSERT INTO messages (message_uuid, session_id, timestamp, message_type, full_content, source) VALUES (?, ?, ?, ?, ?, ?)",
        [
            ("uuid-c1", "sess-claude", "2026-04-01T10:00:00Z", "user", "authentication bug in Claude session", "claude"),
            ("uuid-g1", "sess-gemini", "2026-04-01T12:00:00Z", "user", "authentication bug in Gemini session", "gemini"),
        ],
    )
    conn.executemany(
        "INSERT INTO conversations (session_id, last_message_at, source) VALUES (?, ?, ?)",
        [
            ("sess-claude", "2026-04-01T10:00:00Z", "claude"),
            ("sess-gemini", "2026-04-01T12:00:00Z", "gemini"),
        ],
    )
    conn.commit()
    conn.close()


def test_search_no_source_filter_returns_all(tmp_path):
    db_path = str(tmp_path / "index.db")
    _build_db(db_path)
    search = ConversationSearch(db_path=db_path)
    results = search.search_conversations("authentication bug")
    search.close()
    sources = {r["source"] for r in results}
    assert sources == {"claude", "gemini"}


def test_search_source_filter_claude(tmp_path):
    db_path = str(tmp_path / "index.db")
    _build_db(db_path)
    search = ConversationSearch(db_path=db_path)
    results = search.search_conversations("authentication bug", source="claude")
    search.close()
    assert all(r["source"] == "claude" for r in results)
    assert len(results) == 1


def test_search_source_filter_gemini(tmp_path):
    db_path = str(tmp_path / "index.db")
    _build_db(db_path)
    search = ConversationSearch(db_path=db_path)
    results = search.search_conversations("authentication bug", source="gemini")
    search.close()
    assert all(r["source"] == "gemini" for r in results)
    assert len(results) == 1


def test_list_no_source_filter_returns_all(tmp_path):
    db_path = str(tmp_path / "index.db")
    _build_db(db_path)
    search = ConversationSearch(db_path=db_path)
    results = search.list_recent_conversations(days_back=365)
    search.close()
    sources = {r["source"] for r in results}
    assert sources == {"claude", "gemini"}


def test_list_source_filter_claude(tmp_path):
    db_path = str(tmp_path / "index.db")
    _build_db(db_path)
    search = ConversationSearch(db_path=db_path)
    results = search.list_recent_conversations(days_back=365, source="claude")
    search.close()
    assert all(r["source"] == "claude" for r in results)
    assert len(results) == 1
