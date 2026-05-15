# Agent Recall — Technical Reference

## CLI Command Reference

### agent-recall init

Initialize the database and perform initial indexing.

```bash
agent-recall init [--days DAYS] [--no-extract] [--force]
```

**Options:**
- `--days DAYS`: Index last N days of conversations (default: 7)
- `--no-extract`: Skip smart extraction, store only raw content
- `--force`: Reinitialize existing database

**What it does:**
1. Creates `~/.agent-recall/index.db` SQLite database
2. Scans all configured CLI conversation directories
3. Parses JSONL/JSON conversation formats
4. Extracts searchable content using smart hybrid extraction (instant, no AI)
5. Builds FTS5 search index

---

### agent-recall index

JIT index conversations (instant, no AI calls). Run before searches for fresh data.

```bash
agent-recall index [--days N] [--all] [--no-extract]
```

**Options:**
- `--days N`: Index last N days (default: 1)
- `--all`: Index all conversations
- `--no-extract`: Skip smart extraction

---

### agent-recall search

```bash
agent-recall search QUERY [--days N] [--project PATH] [--limit N] [--content] [--json]
```

**Arguments:**
- `QUERY`: Search query (supports FTS5 syntax)

**Options:**
- `--days N`: Limit to last N days
- `--date DATE`: Specific calendar day (`yesterday`, `today`, `YYYY-MM-DD`)
- `--since DATE` / `--until DATE`: Date range
- `--project PATH`: Filter by project path
- `--limit N`: Max results (default: 20)
- `--content`: Show full message content instead of summaries
- `--json`: Output as JSON

**Note:** `--days` cannot be combined with `--date`/`--since`/`--until`.

**Search syntax:**
- Simple: `authentication bug`
- Phrase: `"exact phrase"`
- Operators: `auth AND bug`, `react OR vue`

---

### agent-recall context

Get surrounding messages for a specific message UUID.

```bash
agent-recall context MESSAGE_UUID [--depth N] [--content] [--json]
```

---

### agent-recall list

List recent conversations.

```bash
agent-recall list [--days N] [--date DATE] [--since DATE] [--until DATE] [--limit N] [--json]
```

---

### agent-recall tree

Show conversation tree structure for a session.

```bash
agent-recall tree SESSION_ID [--json]
```

---

## MCP Server Reference

The MCP server (`agent-recall-mcp`) exposes three tools via stdio:

### `search(query, source?, since?, until?, k=5)`

Returns ranked fragments matching the query.

```json
[
  {
    "source": "claude",
    "session_id": "abc-123",
    "project_path": "/home/user/projects/myapp",
    "ts": "2025-11-13T10:30:00",
    "role": "user",
    "snippet": "We need to fix the authentication bug...",
    "message_uuid": "def-456"
  }
]
```

### `get_context(message_uuid, window=5)`

Returns surrounding messages for a search hit.

```json
{
  "message": { ... },
  "ancestors": [ ... ],
  "children": [ ... ]
}
```

### `list(source?, since?, until?, limit=20)`

Returns conversation-level summaries with resume hints.

```json
[
  {
    "session_id": "abc-123",
    "project_path": "/home/user/projects/myapp",
    "source": "claude",
    "conversation_summary": "Auth Bug Fix",
    "last_message_at": "2025-11-13T10:30:00",
    "message_count": 42,
    "resume_hint": "cd /home/user/projects/myapp && claude --resume abc-123"
  }
]
```

---

## Database Schema

**Location:** `~/.agent-recall/index.db`

**Tables:**
- `messages`: Individual messages with summaries, tree structure (`parent_uuid`), source
- `conversations`: Session metadata and summaries
- `message_summaries_fts`: FTS5 full-text search index

**Key fields:**
- `message_uuid`: Unique message identifier
- `parent_uuid`: Parent message (tree structure)
- `session_id`: Conversation session
- `source`: `"claude"`, `"gemini"`, or `"codex"`
- `summary`: Smart-extracted searchable content
- `full_content`: Original message content

---

## How Smart Extraction Works

1. **User messages**: Full content indexed (avg 3.5K chars, important info upfront)
2. **Assistant messages**: First 500 + last 200 chars + tool usage metadata
3. **Tool noise**: Pure tool markers filtered automatically
4. **Short messages**: Raw content used (< 50 chars)
5. **Instant**: No AI API calls, deterministic, ~1000+ messages/second

---

## Conversation sources

| Source | Files indexed |
|---|---|
| Claude Code | `~/.claude/projects/<proj>/<session>.jsonl` |
| Gemini CLI | `~/.gemini/tmp/<hash>/chats/*.json` |
| Codex CLI | *(adapter not yet built)* |

---

## Resume hints (per source)

Resume hints are included in `list` output and surfaced by the skill only when the user explicitly asks.

| Source | Command |
|---|---|
| Claude | `cd <project_path> && claude --resume <session_id>` |
| Gemini | `cd <project_path> && gemini --resume` *(then pick session from browser)* |
| Codex | No CLI resume command — share session ID and project path |

---

## Troubleshooting

**Database not found:**
```bash
agent-recall init
```

**No results after search:**
```bash
agent-recall index --days 30    # reindex wider window
agent-recall list --days 30 --json  # browse what's indexed
```

**mcp module not found (running tests):**

The `mcp` package is a runtime dependency installed automatically via pip. If running tests without a full install:
```bash
pip install -e .
```

**Skill not activating in Claude Code:**
- Check: `ls ~/.claude/skills/agent-recall/SKILL.md`
- Verify YAML frontmatter is intact
- Restart Claude Code, or trigger explicitly: *"Search my conversations for X"*
