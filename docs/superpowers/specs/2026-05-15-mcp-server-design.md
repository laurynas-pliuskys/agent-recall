# MCP Server + Package Rename — Design Spec

**Date:** 2026-05-15  
**Scope:** v1 task 4 — wrap ConversationSearch in an MCP server (stdio transport, Python `mcp` SDK)  
**Also includes:** package rename (`conversation_search` → `agent_recall`) and indexer improvements

---

## 1. Package Rename

Rename the Python package from `conversation_search` to `agent_recall` throughout the codebase.

### Changes

| Before | After |
|---|---|
| `src/conversation_search/` | `src/agent_recall/` |
| `from conversation_search.X import Y` | `from agent_recall.X import Y` |
| `packages = ["src/conversation_search"]` in pyproject.toml | `packages = ["src/agent_recall"]` |
| `~/.conversation-search/index.db` | `~/.agent-recall/index.db` |

### Migration notice

If `~/.conversation-search/index.db` exists and `~/.agent-recall/index.db` does not, print a one-time notice on any command:

```
Database found at old path ~/.conversation-search/index.db.
Move it: mv ~/.conversation-search/index.db ~/.agent-recall/index.db
Or re-initialize: agent-recall init
```

Do not auto-migrate silently — user must act explicitly.

---

## 2. Indexer Improvement — Smart `index_new()`

### Problem

`index_all()` is a misleading name (sounds like full reindex) and callers hardcode `days_back` values (1, 7, 30) throughout the CLI and MCP server.

### Solution

Rename `index_all()` → `index_new()` and make it self-determining:

1. `get_last_indexed_at()` — queries `SELECT MAX(indexed_at) FROM conversations`
2. **Empty DB (None returned)** → `days_back=None` — index all history
3. **Has data** → `days_back = (today - last_indexed_date).days + 1`
   - Uses the *date* of last index (not exact timestamp) — re-covers the partial day, safer against clock skew and late-appended messages
   - Example: last indexed 2026-05-14 → scans files modified on May 14 or later

4. Explicit `--days N` flag still works as an override when the user wants it

### Rename impact

All internal callers (`cli.py`, future `mcp_server.py`) call `index_new()` with no arguments. The `--days` override is surfaced in the CLI only.

---

## 3. MCP Server

### Dependency

Add `mcp>=1.0` to `pyproject.toml` dependencies.

### File

`src/agent_recall/mcp_server.py` — uses `FastMCP` (handles JSON schema generation automatically from type annotations).

### Entrypoint

```toml
# pyproject.toml
[project.scripts]
agent-recall     = "agent_recall.cli:main"
agent-recall-mcp = "agent_recall.mcp_server:main"
```

### Startup sequence

1. Run `ConversationIndexer().index_new()` (quiet) — ensures DB is fresh before serving any tool calls
2. Open `ConversationSearch()`
3. Start FastMCP stdio server

If `~/.agent-recall/index.db` does not exist at startup, skip indexing and have all tool calls return:
```
Database not found. Run: agent-recall init
```

### Tools

#### `search`

```python
def search(
    query: str,
    source: Optional[str] = None,   # "claude" | "gemini" | None (all)
    since: Optional[str] = None,    # "YYYY-MM-DD" | "yesterday" | "today"
    until: Optional[str] = None,
    k: int = 5,
) -> list[dict]
```

Calls `ConversationSearch.search_conversations(query, source=source, since=since, until=until, limit=k)`.

Returns ranked fragments (FTS5 relevance order):
```json
[{
  "source": "claude",
  "session_id": "abc123",
  "project_path": "/home/user/myproject",
  "ts": "2026-05-14T10:32:00+02:00",
  "role": "user",
  "snippet": "...matched **text** here...",
  "message_uuid": "uuid-here"
}]
```

No explicit score field — results are already in relevance order.

`project_path` is returned as a real filesystem path (slashes restored from the hashed storage format), matching what the CLI displays.

#### `get_context`

```python
def get_context(
    message_uuid: str,
    window: int = 5,
) -> dict
```

Calls `ConversationSearch.get_conversation_context(message_uuid, depth=window)`.

Returns the target message + ancestors (up to `window` levels) + direct children, each with full content. Uses tree traversal via `parent_uuid` — correct for branching conversations (retried/edited turns).

```json
{
  "message": {...},
  "ancestors": [...],
  "children": [...],
  "conversation": {"session_id": "...", "conversation_summary": "...", "project_path": "..."},
  "context_depth": 3
}
```

#### `list_conversations`

The Python function is named `list_conversations` (avoids shadowing Python's built-in `list`). MCP clients see it as `list_conversations`.

```python
def list_conversations(
    source: Optional[str] = None,
    since: Optional[str] = None,
    until: Optional[str] = None,
    limit: int = 20,
) -> list[dict]
```

Calls `ConversationSearch.list_recent_conversations(source=source, since=since, until=until, limit=limit)`.

Returns conversation-level summaries. Each entry includes a `resume_hint` field:
- Claude: `"claude --resume <session_id>"`
- Gemini: `"gemini --resume <session_id>"` (placeholder — Gemini resume command TBD)
- Unknown source: omitted

```json
[{
  "session_id": "abc123",
  "source": "claude",
  "project_path": "/home/user/myproject",
  "conversation_summary": "Debugging auth middleware",
  "last_message_at": "2026-05-14T10:32:00+02:00",
  "message_count": 42,
  "resume_hint": "claude --resume abc123"
}]
```

### Error handling

All three tools catch exceptions and return a plain string error message rather than raising — MCP protocol expects tool results, not exceptions. DB-not-found is the primary case to handle; other errors are logged to stderr.

---

## 4. SessionStart Hook

The MCP server runs `index_new()` on startup, which covers the common case. Additionally, users can configure a `SessionStart` hook for indexing *before* the MCP server is ready (belt-and-suspenders).

### Delivery

Document in README as opt-in installation step. Optionally provide `agent-recall install-hook` subcommand that writes the hook config automatically. **Do not silently modify user's Claude settings.**

Hook command:
```bash
agent-recall index --quiet
```

---

## 5. Tests

| Test | What it covers |
|---|---|
| `test_get_last_indexed_at_empty` | Empty DB → returns None |
| `test_get_last_indexed_at_populated` | Populated DB → returns correct date |
| `test_index_new_days_back_empty` | Empty DB → `days_back=None` passed to scan |
| `test_index_new_days_back_incremental` | Populated DB → correct `days_back` computed |
| `test_mcp_search` | `search()` returns correct fragment shape |
| `test_mcp_get_context` | `get_context()` returns message + ancestors |
| `test_mcp_list` | `list_conversations()` returns summaries with `resume_hint` |
| `test_mcp_db_not_found` | All tools return friendly error string when DB missing |

MCP server entrypoint wiring (`main()`) is not unit-tested — it's pure glue.

---

## Cleanup (do during rename PR)

- Remove or deprecate legacy `scan_conversations()` in `indexer.py` — superseded by `scan_all()` after the adapter refactor

---

## Out of scope

- Codex adapter (separate task)
- Semantic/embedding search (v2)
- `agent-recall install-hook` subcommand (nice-to-have, can be added later)
- Gemini resume command format (unknown — placeholder string is fine for now)
