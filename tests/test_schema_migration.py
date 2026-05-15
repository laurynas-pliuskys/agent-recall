import sqlite3
import tempfile
from pathlib import Path
from conversation_search.core.indexer import ConversationIndexer


def test_migration_adds_source_to_fresh_db(tmp_path):
    db_path = tmp_path / "index.db"
    indexer = ConversationIndexer(db_path=str(db_path), quiet=True)
    indexer.close()

    conn = sqlite3.connect(str(db_path))
    cursor = conn.cursor()
    cursor.execute("PRAGMA table_info(messages)")
    cols = {row[1] for row in cursor.fetchall()}
    conn.close()

    assert "source" in cols


def test_migration_adds_source_to_existing_db(tmp_path):
    """Simulate a pre-migration DB without the source column."""
    db_path = tmp_path / "index.db"

    conn = sqlite3.connect(str(db_path))
    conn.executescript("""
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
            indexed_at TEXT DEFAULT CURRENT_TIMESTAMP
        );
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
            indexed_at TEXT DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE index_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_path TEXT UNIQUE NOT NULL,
            discovered_at TEXT DEFAULT CURRENT_TIMESTAMP,
            processed_at TEXT,
            status TEXT DEFAULT 'pending',
            error_message TEXT
        );
    """)
    conn.commit()
    conn.close()

    # Opening the indexer should run the migration
    indexer = ConversationIndexer(db_path=str(db_path), quiet=True)
    indexer.close()

    conn = sqlite3.connect(str(db_path))
    cursor = conn.cursor()
    cursor.execute("PRAGMA table_info(messages)")
    cols = {row[1] for row in cursor.fetchall()}
    cursor.execute("PRAGMA table_info(conversations)")
    conv_cols = {row[1] for row in cursor.fetchall()}
    conn.close()

    assert "source" in cols
    assert "source" in conv_cols


def test_existing_rows_default_to_claude(tmp_path):
    """Rows inserted before migration should get source='claude' after migration."""
    db_path = tmp_path / "index.db"

    conn = sqlite3.connect(str(db_path))
    conn.executescript("""
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
            indexed_at TEXT DEFAULT CURRENT_TIMESTAMP
        );
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
            indexed_at TEXT DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE index_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_path TEXT UNIQUE NOT NULL,
            discovered_at TEXT DEFAULT CURRENT_TIMESTAMP,
            processed_at TEXT,
            status TEXT DEFAULT 'pending',
            error_message TEXT
        );
        INSERT INTO messages (message_uuid, session_id, timestamp, message_type, full_content)
            VALUES ('uuid-old', 'sess-old', '2024-01-01T00:00:00Z', 'user', 'hello');
    """)
    conn.commit()
    conn.close()

    indexer = ConversationIndexer(db_path=str(db_path), quiet=True)
    indexer.close()

    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    cursor.execute("SELECT source FROM messages WHERE message_uuid = 'uuid-old'")
    row = cursor.fetchone()
    conn.close()

    assert row["source"] == "claude"
