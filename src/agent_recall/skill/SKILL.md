---
name: agent-recall
description: Find and resume Claude Code conversations by searching topics or filtering by date. Returns session IDs and project paths for easy resumption via 'claude --resume'. Use when user asks "find that conversation about X", "what did we discuss", "what did we work on yesterday", "summarize today's work", "show this week's conversations", "recent projects we accomplished", or wants to locate past work by topic, date, or time period (yesterday, today, last week, specific dates).
allowed-tools: Bash, TodoWrite, mcp__agent_recall__search, mcp__agent_recall__list, mcp__agent_recall__get_context
---

# Conversation Search

Find past conversations and return matching fragments.

## MANDATORY FIRST STEP - CREATE TODO CHECKLIST

**Before doing ANYTHING else, you MUST use the TodoWrite tool to create this exact checklist:**

```
- Classify query type (temporal/topic/hybrid)
- Execute search via MCP (or CLI fallback)
- Present fragments to user
```

Mark each todo as `in_progress` when starting it, `completed` when done.

## Query Type Classification

**Second todo: Classify the user's query**

### Type 1: Temporal Queries
User asks about time periods WITHOUT specific topics:
- "What did we work on yesterday?"
- "Summarize this week"
- "Show today's conversations"

**Action:** Use `list` with date filters

### Type 2: Topic Queries
User asks about CONTENT/TOPICS:
- "Find that Redis conversation"
- "Where did we discuss authentication?"

**Action:** Use `search "topic"`

### Type 3: Hybrid Queries
User asks about TOPIC + TIME:
- "Show me yesterday's authentication work"
- "Find Redis discussions from last week"

**Action:** Use `search "topic"` with date filters

## Search Execution

**Preferred: MCP tools** (if the `agent-recall` MCP server is configured)

### Topic / Hybrid queries — call `mcp__agent_recall__search`:
- `query`: search terms
- `since` / `until`: ISO date strings, e.g. `"2025-11-13"` (optional)
- `k`: number of fragments to return (default 5)

### Temporal queries — call `mcp__agent_recall__list`:
- `since` / `until`: ISO date strings (optional)
- `limit`: max results (default 20)

### Getting more context — call `mcp__agent_recall__get_context`:
- `message_uuid`: UUID from a search result
- `window`: surrounding messages to include (default 5)

---

**Fallback: CLI** (if MCP server is not configured)

```bash
# Topic / Hybrid
agent-recall search "terms" --days 14 --json

# Temporal
agent-recall list --date yesterday --json

# Broader search if first attempt finds nothing
agent-recall search "terms" --json
```

**CRITICAL CONSTRAINTS:**
- DO NOT use grep, find, cat, or any manual file operations on .jsonl files
- ONLY use agent-recall commands or MCP tools for search operations

## Presenting Results

**Always synthesize first** — answer the user's actual question in 2–4 sentences based on what the fragments contain. Do not just list citations; the user wants to know what happened, not where to look.

Then show the supporting fragments — omit session/message UUIDs (not useful to the user):

```
**[2025-11-13 22:50]** claude · /home/user/projects/myproject
> "We need to fix the authentication bug in the login flow — the token expiry
>  check was missing from the refresh handler. Fixed by adding a 401 guard..."
```

If a fragment is too short to answer the user's question, call `get_context` on it before presenting, to pull the surrounding messages.

For temporal / list results, group by project and summarize topics covered.

**Do NOT include resume commands unless the user explicitly asks to resume a conversation.**

## Resume Hints (only when explicitly requested)

When the user says "resume", "go back to", "continue that session", etc., provide the appropriate command from the result's `resume_hint` field, or construct it manually:

**Claude sessions:**
```bash
cd /home/user/projects/myproject
claude --resume abc-123-session-id
```

**Gemini sessions:**
```bash
cd /home/user/projects/myproject
gemini --resume  # then pick the session from the browser
```

**Codex sessions:** No resume command available — share the session ID and project path only.

## If Nothing Found

1. Expand scope: remove date filter or increase `--days`
2. Try alternative keywords ("auth" vs "authentication", "db" vs "database")
3. Last resort: `agent-recall list --days 30 --json` to browse recent sessions manually
4. If still nothing: "No matching conversations found — consider running `agent-recall init --days 90` to reindex older history"

## Error Handling

**MCP tools unavailable:** Fall back to CLI workflow above.

**CLI not installed:**
```bash
uv tool install agent-recall   # recommended
# OR
pip install --user agent-recall
agent-recall init --days 7
```

**Database not found:** Run `agent-recall init`

**Empty results after all attempts:** Report clearly and suggest reindexing.
