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


def test_index_new_empty_db_uses_none(tmp_path, monkeypatch):
    """Empty DB → days_back=None (index all history)."""
    indexer = make_indexer(tmp_path)
    captured = []

    def fake_scan_all(days_back):
        captured.append(days_back)
        return []

    monkeypatch.setattr(indexer, "scan_all", fake_scan_all)
    indexer.index_new()
    assert captured == [None]
    indexer.close()


def test_index_new_incremental_uses_date_window(tmp_path, monkeypatch):
    """Populated DB → days_back = (today - last_indexed_date).days + 1."""
    indexer = make_indexer(tmp_path)
    five_days_ago = (datetime.now() - timedelta(days=5)).strftime("%Y-%m-%d %H:%M:%S")
    indexer.conn.execute("""
        INSERT INTO conversations
            (session_id, source, indexed_at, last_message_at, first_message_at)
        VALUES ('sess1', 'claude', ?, ?, ?)
    """, (five_days_ago, five_days_ago, five_days_ago))
    indexer.conn.commit()

    captured = []

    def fake_scan_all(days_back):
        captured.append(days_back)
        return []

    monkeypatch.setattr(indexer, "scan_all", fake_scan_all)
    indexer.index_new()
    assert captured == [6]  # 5 days ago → (5).days + 1 = 6
    indexer.close()


def test_index_new_explicit_days_override(tmp_path, monkeypatch):
    """Explicit days_back argument overrides the auto-computed value."""
    indexer = make_indexer(tmp_path)
    five_days_ago = (datetime.now() - timedelta(days=5)).strftime("%Y-%m-%d %H:%M:%S")
    indexer.conn.execute("""
        INSERT INTO conversations
            (session_id, source, indexed_at, last_message_at, first_message_at)
        VALUES ('sess1', 'claude', ?, ?, ?)
    """, (five_days_ago, five_days_ago, five_days_ago))
    indexer.conn.commit()

    captured = []

    def fake_scan_all(days_back):
        captured.append(days_back)
        return []

    monkeypatch.setattr(indexer, "scan_all", fake_scan_all)
    indexer.index_new(days_back=30)
    assert captured == [30]
    indexer.close()
