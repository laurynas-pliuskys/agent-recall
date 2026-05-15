import sqlite3
from datetime import date, datetime, timedelta
from pathlib import Path
import pytest
from agent_recall.core.indexer import ConversationIndexer


def make_indexer(tmp_path) -> ConversationIndexer:
    return ConversationIndexer(db_path=str(tmp_path / "test.db"), quiet=True)


def test_get_last_indexed_at_empty_db(tmp_path):
    indexer = make_indexer(tmp_path)
    assert indexer.get_last_indexed_at() is None
    indexer.close()


def test_get_last_indexed_at_with_data(tmp_path):
    indexer = make_indexer(tmp_path)
    indexer.conn.execute("""
        INSERT INTO conversations
            (session_id, source, indexed_at, last_message_at, first_message_at)
        VALUES ('sess1', 'claude', '2026-05-14 10:30:00', '2026-05-14 10:30:00',
                '2026-05-14 10:30:00')
    """)
    indexer.conn.commit()
    result = indexer.get_last_indexed_at()
    assert result == date(2026, 5, 14)
    indexer.close()


def test_get_last_indexed_at_returns_latest(tmp_path):
    indexer = make_indexer(tmp_path)
    indexer.conn.executemany("""
        INSERT INTO conversations
            (session_id, source, indexed_at, last_message_at, first_message_at)
        VALUES (?, 'claude', ?, ?, ?)
    """, [
        ('s1', '2026-05-10 08:00:00', '2026-05-10 08:00:00', '2026-05-10 08:00:00'),
        ('s2', '2026-05-14 10:30:00', '2026-05-14 10:30:00', '2026-05-14 10:30:00'),
        ('s3', '2026-05-12 12:00:00', '2026-05-12 12:00:00', '2026-05-12 12:00:00'),
    ])
    indexer.conn.commit()
    result = indexer.get_last_indexed_at()
    assert result == date(2026, 5, 14)
    indexer.close()
