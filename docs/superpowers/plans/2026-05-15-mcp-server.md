# MCP Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the Python package to `agent_recall`, make indexing self-determining, and expose a three-tool MCP server (search / get_context / list_conversations) over stdio.

**Architecture:** One new file `src/agent_recall/mcp_server.py` using FastMCP wraps `ConversationSearch` and runs `index_new()` on startup. The indexer gains `get_last_indexed_at()` and renames `index_all` → `index_new` with smart days_back (all history on first run, date-windowed on subsequent runs). A `SessionStart` hook provides belt-and-suspenders indexing.

**Tech Stack:** Python ≥ 3.9, SQLite/FTS5 (existing), `mcp >= 1.0` (FastMCP), pytest

---

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Rename dir | `src/conversation_search/` → `src/agent_recall/` | Package root |
| Modify | `src/agent_recall/core/indexer.py` | Add `get_last_indexed_at()`, rename `index_all`→`index_new`, remove `scan_conversations()` |
| Modify | `src/agent_recall/core/summarization.py` | Update default `db_path` only |
| Modify | `src/agent_recall/core/search.py` | Update default `db_path` and imports |
| Modify | `src/agent_recall/adapters/claude.py` | Update imports |
| Modify | `src/agent_recall/adapters/gemini.py` | Update imports |
| Modify | `src/agent_recall/cli.py` | Update imports, db path, callers of `index_new`, migration notice |
| Modify | `src/agent_recall/__init__.py` | Update docstring |
| Create | `src/agent_recall/mcp_server.py` | FastMCP server with 3 tools + startup |
| Modify | `pyproject.toml` | Package path, schema path, add `mcp>=1.0`, add `agent-recall-mcp` entrypoint |
| Modify | `tests/*.py` + `tests/adapters/*.py` | Update imports |
| Create | `tests/test_index_new.py` | Tests for `get_last_indexed_at` and `index_new` days_back logic |
| Create | `tests/test_mcp_server.py` | Tests for MCP tool functions and error handling |
| Modify | `README.md` | Add SessionStart hook installation section |

---

## Task 1: Rename package `conversation_search` → `agent_recall`

**Files:**
- Create: `src/agent_recall/` (from `src/conversation_search/`)
- Modify: `pyproject.toml`
- Modify: all Python files in `src/agent_recall/` and `tests/`

**Important distinction:** `"conversation-search"` string literals inside `summarization.py` (used to detect when the old skill appears in conversation content) must NOT be changed — they match text in real conversations. Only Python import paths and file system paths get renamed.

- [ ] **Step 1: Write rename script**

Write to `/tmp/rename_pkg.py`:

```python
import os
import shutil
from pathlib import Path

repo = Path("/home/laurynas/github/agent-recall")
old_src = repo / "src" / "conversation_search"
new_src = repo / "src" / "agent_recall"

# Copy directory tree
shutil.copytree(old_src, new_src)

# Files to rewrite (Python source + tests only)
py_files = list(new_src.rglob("*.py")) + list((repo / "tests").rglob("*.py"))

for path in py_files:
    text = path.read_text()
    # Replace Python import paths
    text = text.replace("from conversation_search.", "from agent_recall.")
    text = text.replace("import conversation_search.", "import agent_recall.")
    text = text.replace("files('conversation_search.data')", "files('agent_recall.data')")
    # Replace filesystem paths
    text = text.replace("~/.conversation-search/", "~/.agent-recall/")
    # Update __init__ docstring
    text = text.replace(
        '"""conversation-search - Semantic search across Claude Code conversation history"""',
        '"""agent-recall - Search across Claude Code and Gemini CLI conversation history"""'
    )
    text = text.replace(
        '"""Core functionality for conversation-search"""',
        '"""Core functionality for agent-recall"""'
    )
    path.write_text(text)

print("Done. Review changes then delete src/conversation_search/")
```

- [ ] **Step 2: Run rename script**

```bash
python3 /tmp/rename_pkg.py
```

Expected: prints "Done. Review changes then delete src/conversation_search/"

- [ ] **Step 3: Update pyproject.toml**

Edit `pyproject.toml` — change three places:

```toml
[project.scripts]
agent-recall = "agent_recall.cli:main"

[tool.hatch.build.targets.wheel]
packages = ["src/agent_recall"]

[tool.hatch.build.targets.wheel.force-include]
"src/agent_recall/data/schema.sql" = "agent_recall/data/schema.sql"
```

- [ ] **Step 4: Remove old package directory**

```bash
rm -rf /home/laurynas/github/agent-recall/src/conversation_search
```

- [ ] **Step 5: Reinstall package**

```bash
pip install -e /home/laurynas/github/agent-recall
```

Expected: installs without errors, `agent-recall` command still works.

- [ ] **Step 6: Run all existing tests**

```bash
pytest /home/laurynas/github/agent-recall/tests/ -v
```

Expected: all tests pass (same count as before rename).

- [ ] **Step 7: Add migration notice to `cli.py`**

At the top of `main()` in `src/agent_recall/cli.py`, before `args = parser.parse_args()`, add:

```python
    # One-time migration notice for users with old DB location
    old_db = Path.home() / ".conversation-search" / "index.db"
    new_db = Path.home() / ".agent-recall" / "index.db"
    if old_db.exists() and not new_db.exists():
        print(
            "Note: database found at old path ~/.conversation-search/index.db\n"
            "Move it:        mv ~/.conversation-search/index.db ~/.agent-recall/index.db\n"
            "Or re-init:     agent-recall init\n"
        )
```

- [ ] **Step 8: Commit**

```bash
git -C /home/laurynas/github/agent-recall add -A
```

Write commit message to `/tmp/commit_msg_1.txt`:
```
refactor: rename package conversation_search → agent_recall

Updates all Python imports, default DB path (~/.agent-recall/index.db),
pyproject.toml package declaration, and adds migration notice for users
with old DB location. Detection strings for the old skill name in
summarization.py are preserved as-is (they match literal conversation text).
```

```bash
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" \
  git -C /home/laurynas/github/agent-recall commit \
  --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_msg_1.txt
```

---

## Task 2: Add `get_last_indexed_at()` to `ConversationIndexer`

**Files:**
- Modify: `src/agent_recall/core/indexer.py`
- Create: `tests/test_index_new.py`

- [ ] **Step 1: Write failing tests**

Create `tests/test_index_new.py`:

```python
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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest /home/laurynas/github/agent-recall/tests/test_index_new.py -v
```

Expected: FAIL with `AttributeError: 'ConversationIndexer' object has no attribute 'get_last_indexed_at'`

- [ ] **Step 3: Implement `get_last_indexed_at()`**

Add this method to `ConversationIndexer` in `src/agent_recall/core/indexer.py`, after `_init_db`:

```python
def get_last_indexed_at(self) -> Optional["date"]:
    """Return the date of the most recently indexed conversation, or None if DB is empty."""
    from datetime import date as date_type
    cursor = self.conn.cursor()
    cursor.execute("SELECT MAX(indexed_at) FROM conversations")
    row = cursor.fetchone()
    if row[0] is None:
        return None
    return datetime.fromisoformat(row[0]).date()
```

Also add `from datetime import date` to the imports at the top of `indexer.py` (it already imports `datetime` and `timedelta`, just add `date`):

```python
from datetime import date, datetime, timedelta
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
pytest /home/laurynas/github/agent-recall/tests/test_index_new.py -v
```

Expected: all 3 tests PASS

- [ ] **Step 5: Commit**

Write `/tmp/commit_msg_2.txt`:
```
feat: add get_last_indexed_at() to ConversationIndexer

Returns the date of the most recently indexed conversation from
MAX(indexed_at), or None if the DB is empty.
```

```bash
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" \
  git -C /home/laurynas/github/agent-recall commit \
  --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_msg_2.txt \
  src/agent_recall/core/indexer.py tests/test_index_new.py
```

---

## Task 3: Rename `index_all` → `index_new` with smart `days_back`

**Files:**
- Modify: `src/agent_recall/core/indexer.py`
- Modify: `src/agent_recall/cli.py`
- Modify: `tests/test_index_new.py`

- [ ] **Step 1: Add days_back tests to `tests/test_index_new.py`**

Append to `tests/test_index_new.py`:

```python
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
    # Put something in DB so auto would compute 6
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
```

- [ ] **Step 2: Run new tests to verify they fail**

```bash
pytest /home/laurynas/github/agent-recall/tests/test_index_new.py::test_index_new_empty_db_uses_none \
       /home/laurynas/github/agent-recall/tests/test_index_new.py::test_index_new_incremental_uses_date_window \
       /home/laurynas/github/agent-recall/tests/test_index_new.py::test_index_new_explicit_days_override -v
```

Expected: FAIL with `AttributeError: 'ConversationIndexer' object has no attribute 'index_new'`

- [ ] **Step 3: Rename `index_all` → `index_new` and add smart `days_back`**

In `src/agent_recall/core/indexer.py`, replace the `index_all` method signature and body opening:

Old:
```python
def index_all(self, days_back: Optional[int] = 1, summarize: bool = True):
    """Index all conversations from the last N days"""
    pairs = self.scan_all(days_back)
```

New:
```python
def index_new(self, days_back: Optional[int] = None, summarize: bool = True):
    """Index new/changed conversations since the last run.

    days_back=None triggers auto-detection:
    - Empty DB → scan all history
    - Has data → scan from the date of last indexed conversation (+1 day buffer)
    Pass days_back explicitly to override.
    """
    if days_back is None:
        last_date = self.get_last_indexed_at()
        if last_date is None:
            days_back = None  # index everything
        else:
            today = datetime.now().date()
            days_back = (today - last_date).days + 1

    pairs = self.scan_all(days_back)
```

Also remove the legacy `scan_conversations` method entirely (it was the old Claude-only scan, now superseded by `scan_all`). Search for `def scan_conversations` and delete that method and its docstring.

- [ ] **Step 4: Update callers in `cli.py`**

In `src/agent_recall/cli.py`, replace every call to `indexer.index_all(...)` with `indexer.index_new(...)`.

There are calls in `cmd_init`, `cmd_index`, `cmd_search`, `cmd_context`, `cmd_list`. For each, remove any hardcoded `days_back=` argument — let `index_new()` determine it automatically. Exception: `cmd_index` should still pass `days_back=args.days if not args.all else None` when the user explicitly passes `--days` or `--all`.

Specific changes in `cmd_init`:
```python
indexer.index_new(days_back=args.days if args.days != 7 else None, summarize=not args.no_extract)
```
Wait — for `init`, it's the first run so `days_back` should always be `None` (index all). Change to:
```python
indexer.index_new(summarize=not args.no_extract)
```

In `cmd_index`:
```python
indexer.index_new(
    days_back=args.days if not args.all else None,
    summarize=not args.no_extract
)
```

In `cmd_search`, `cmd_context`, `cmd_list` (auto-index before search):
```python
indexer.index_new(summarize=True)
```
(Remove the `days_to_index = max(...)` lines — no longer needed.)

Also update the `indexer.py` `main()` function at the bottom (legacy entrypoint):
```python
indexer.index_new(days_back=None if args.all else args.days)
```

- [ ] **Step 5: Run all tests**

```bash
pytest /home/laurynas/github/agent-recall/tests/ -v
```

Expected: all tests pass including the 3 new `test_index_new` tests.

- [ ] **Step 6: Commit**

Write `/tmp/commit_msg_3.txt`:
```
feat: rename index_all → index_new with smart days_back

Empty DB indexes all history (days_back=None). Subsequent runs compute
days_back from the date of last indexed conversation + 1-day buffer,
re-covering any messages appended to partially-indexed days.
Removes legacy scan_conversations() (superseded by scan_all).
```

```bash
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" \
  git -C /home/laurynas/github/agent-recall commit \
  --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_msg_3.txt \
  src/agent_recall/core/indexer.py src/agent_recall/cli.py tests/test_index_new.py
```

---

## Task 4: Add `mcp` dependency and entrypoint

**Files:**
- Modify: `pyproject.toml`

- [ ] **Step 1: Update `pyproject.toml`**

Add `mcp>=1.0` to dependencies and add the new entrypoint:

```toml
[project]
dependencies = ["mcp>=1.0"]

[project.scripts]
agent-recall     = "agent_recall.cli:main"
agent-recall-mcp = "agent_recall.mcp_server:main"
```

- [ ] **Step 2: Reinstall to pick up the new dependency and entrypoint**

```bash
pip install -e /home/laurynas/github/agent-recall
```

Expected: installs `mcp` package, no errors. Verify: `python3 -c "from mcp.server.fastmcp import FastMCP; print('ok')"` prints `ok`.

- [ ] **Step 3: Commit**

Write `/tmp/commit_msg_4.txt`:
```
chore: add mcp>=1.0 dependency and agent-recall-mcp entrypoint
```

```bash
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" \
  git -C /home/laurynas/github/agent-recall commit \
  --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_msg_4.txt \
  pyproject.toml
```

---

## Task 5: MCP server skeleton — startup, helpers, DB-not-found

**Files:**
- Create: `src/agent_recall/mcp_server.py`
- Create: `tests/test_mcp_server.py`

- [ ] **Step 1: Write failing tests for helpers and DB-not-found**

Create `tests/test_mcp_server.py`:

```python
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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest /home/laurynas/github/agent-recall/tests/test_mcp_server.py -v
```

Expected: FAIL with `ModuleNotFoundError: No module named 'agent_recall.mcp_server'`

- [ ] **Step 3: Create `src/agent_recall/mcp_server.py`**

```python
import sys
from typing import Optional

from mcp.server.fastmcp import FastMCP

from agent_recall.core.indexer import ConversationIndexer
from agent_recall.core.search import ConversationSearch

DB_PATH = "~/.agent-recall/index.db"

mcp = FastMCP("agent-recall")


def _get_search() -> Optional[ConversationSearch]:
    try:
        return ConversationSearch(db_path=DB_PATH)
    except FileNotFoundError:
        return None


def _project_path_to_fs(stored_path: str) -> str:
    path = stored_path.replace("-", "/")
    return path if path.startswith("/") else f"/{path}"


def _resume_hint(source: str, session_id: str) -> Optional[str]:
    if source == "claude":
        return f"claude --resume {session_id}"
    if source == "gemini":
        return f"gemini --resume {session_id}"
    return None


@mcp.tool()
def search(
    query: str,
    source: Optional[str] = None,
    since: Optional[str] = None,
    until: Optional[str] = None,
    k: int = 5,
):
    """Search past conversations. Returns ranked fragments matching the query."""
    cs = _get_search()
    if cs is None:
        return "Database not found. Run: agent-recall init"
    try:
        results = cs.search_conversations(
            query=query, source=source, since=since, until=until, limit=k
        )
        return [
            {
                "source": r["source"],
                "session_id": r["session_id"],
                "project_path": _project_path_to_fs(r.get("project_path") or ""),
                "ts": r["timestamp"],
                "role": r["message_type"],
                "snippet": r["context_snippet"],
                "message_uuid": r["message_uuid"],
            }
            for r in results
        ]
    finally:
        cs.close()


@mcp.tool()
def get_context(message_uuid: str, window: int = 5):
    """Get surrounding messages for a search hit via tree traversal."""
    cs = _get_search()
    if cs is None:
        return "Database not found. Run: agent-recall init"
    try:
        return cs.get_conversation_context(message_uuid=message_uuid, depth=window)
    finally:
        cs.close()


@mcp.tool()
def list_conversations(
    source: Optional[str] = None,
    since: Optional[str] = None,
    until: Optional[str] = None,
    limit: int = 20,
):
    """List recent conversations with resume hints."""
    cs = _get_search()
    if cs is None:
        return "Database not found. Run: agent-recall init"
    try:
        convs = cs.list_recent_conversations(
            source=source, since=since, until=until, limit=limit
        )
        results = []
        for conv in convs:
            entry = dict(conv)
            entry["resume_hint"] = _resume_hint(
                conv.get("source", "claude"), conv["session_id"]
            )
            results.append(entry)
        return results
    finally:
        cs.close()


def main():
    try:
        indexer = ConversationIndexer(db_path=DB_PATH, quiet=True)
        indexer.index_new()
        indexer.close()
    except Exception as e:
        print(f"Warning: startup indexing failed: {e}", file=sys.stderr)

    mcp.run()


if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Run tests**

```bash
pytest /home/laurynas/github/agent-recall/tests/test_mcp_server.py -v
```

Expected: all 8 tests PASS.

- [ ] **Step 5: Commit**

Write `/tmp/commit_msg_5.txt`:
```
feat: add MCP server skeleton with 3 tools and startup indexing

Exposes search, get_context, list_conversations via FastMCP stdio.
Runs index_new() on startup. Returns friendly error string when DB
is missing. Helper functions for path conversion and resume hints.
```

```bash
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" \
  git -C /home/laurynas/github/agent-recall commit \
  --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_msg_5.txt \
  src/agent_recall/mcp_server.py tests/test_mcp_server.py
```

---

## Task 6: Integration tests — search and list_conversations with real DB

**Files:**
- Modify: `tests/test_mcp_server.py`

These tests use a real SQLite DB (via `ConversationIndexer`) to verify the tool output shapes.

- [ ] **Step 1: Add the DB fixture and shape tests to `tests/test_mcp_server.py`**

Append to `tests/test_mcp_server.py`:

```python
from agent_recall.core.indexer import ConversationIndexer


@pytest.fixture
def test_db(tmp_path):
    db_path = str(tmp_path / "test.db")
    indexer = ConversationIndexer(db_path=db_path, quiet=True)
    # Insert conversation and message via SQL — triggers populate FTS automatically
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
```

- [ ] **Step 2: Run all tests**

```bash
pytest /home/laurynas/github/agent-recall/tests/ -v
```

Expected: all tests pass including the 4 new integration tests.

- [ ] **Step 3: Commit**

Write `/tmp/commit_msg_6.txt`:
```
test: add MCP server integration tests with real SQLite DB

Verifies search fragment shape, empty result, list resume_hint,
and get_context message structure against an in-memory test database.
```

```bash
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" \
  git -C /home/laurynas/github/agent-recall commit \
  --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_msg_6.txt \
  tests/test_mcp_server.py
```

---

## Task 7: README — SessionStart hook and MCP server setup

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add MCP server section to README**

In `README.md`, add a new section after `## Quick start` (or within it):

````markdown
### MCP server (for Codex, Cursor, and other MCP clients)

Start the MCP server via stdio:

```bash
agent-recall-mcp
```

Or configure it in your MCP client. For Claude Code, add to `.claude/settings.json`:

```json
{
  "mcpServers": {
    "agent-recall": {
      "command": "agent-recall-mcp"
    }
  }
}
```

The server exposes three tools: `search`, `get_context`, `list_conversations`.
It runs `agent-recall index` automatically on startup to ensure fresh data.

### SessionStart hook (optional, belt-and-suspenders)

To index at the start of every Claude Code conversation (before the MCP server
is ready), add a `SessionStart` hook to `.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "agent-recall index --quiet"
          }
        ]
      }
    ]
  }
}
```

Indexing is incremental — it only processes new content since the last run,
so this adds only a few seconds on each session start.
````

- [ ] **Step 2: Commit**

Write `/tmp/commit_msg_7.txt`:
```
docs: add MCP server setup and SessionStart hook instructions to README
```

```bash
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" \
  git -C /home/laurynas/github/agent-recall commit \
  --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_msg_7.txt \
  README.md
```

---

## Final verification

- [ ] **Run the full test suite one last time**

```bash
pytest /home/laurynas/github/agent-recall/tests/ -v
```

Expected: all tests pass.

- [ ] **Smoke-test the CLI still works**

```bash
agent-recall --version
agent-recall --help
```

Expected: prints version and help without errors.

- [ ] **Smoke-test the MCP entrypoint exists**

```bash
which agent-recall-mcp
python3 -c "from agent_recall.mcp_server import search, get_context, list_conversations; print('ok')"
```

Expected: path printed, then `ok`.
