# V1 Adapter Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the monolithic Claude-only indexer into a pluggable adapter pattern and add Codex + Gemini adapters, while keeping Claude indexing working throughout.

**Architecture:** Extract parsing into `BaseAdapter` + per-CLI adapters (`ClaudeAdapter`, `CodexAdapter`, `GeminiAdapter`); refactor `ConversationIndexer` to drive the adapter list; add a `source` column to the schema with a one-shot migration for existing DBs.

**Tech Stack:** Python 3.9+, SQLite FTS5, `dataclasses`, `abc`, `pathlib`, `json`

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/conversation_search/adapters/__init__.py` | Package marker |
| Create | `src/conversation_search/adapters/base.py` | `ParsedMessage`, `ConversationMeta`, `BaseAdapter` ABC |
| Create | `src/conversation_search/adapters/claude.py` | `ClaudeAdapter` — wraps existing JSONL parse logic |
| Create | `src/conversation_search/adapters/codex.py` | `CodexAdapter` — parses `~/.codex/sessions/` rollouts |
| Create | `src/conversation_search/adapters/gemini.py` | `GeminiAdapter` — parses `~/.gemini/tmp/` chat JSON |
| Modify | `src/conversation_search/core/indexer.py` | Drive adapter list; accept `(path, adapter)` pairs |
| Modify | `src/conversation_search/data/schema.sql` | Add `source` column to `messages` + `conversations` |
| Modify | `src/conversation_search/core/search.py` | Accept optional `source` filter in `search_conversations` and `list_recent_conversations` |
| Create | `tests/adapters/__init__.py` | Package marker |
| Create | `tests/adapters/test_claude_adapter.py` | Unit tests for `ClaudeAdapter.parse` |
| Create | `tests/adapters/test_codex_adapter.py` | Unit tests for `CodexAdapter.parse` |
| Create | `tests/adapters/test_gemini_adapter.py` | Unit tests for `GeminiAdapter.parse` |
| Create | `tests/test_schema_migration.py` | Migration adds `source` column to existing DB |

---

## Task 1: Define `ParsedMessage`, `ConversationMeta`, and `BaseAdapter`

**Files:**
- Create: `src/conversation_search/adapters/__init__.py`
- Create: `src/conversation_search/adapters/base.py`

- [ ] **Step 1: Write the failing test** (import the types)

```python
# tests/adapters/__init__.py  (empty)
```

```python
# tests/adapters/test_base_types.py
from conversation_search.adapters.base import ParsedMessage, ConversationMeta, BaseAdapter

def test_parsed_message_defaults():
    msg = ParsedMessage(
        uuid="abc123",
        parent_uuid=None,
        session_id="sess1",
        timestamp="2024-01-01T00:00:00Z",
        role="user",
        content="hello",
        source="claude",
    )
    assert msg.is_sidechain is False
    assert msg.project_path == ""

def test_conversation_meta():
    meta = ConversationMeta(
        session_id="sess1",
        source="claude",
        project_path="myproject",
        conversation_file="/path/to/file.jsonl",
        summary="A test conversation",
        leaf_uuid="leaf1",
    )
    assert meta.session_id == "sess1"
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/adapters/test_base_types.py -v
```
Expected: `ModuleNotFoundError: No module named 'conversation_search.adapters'`

- [ ] **Step 3: Create the package and base module**

```python
# src/conversation_search/adapters/__init__.py
```
(empty file)

```python
# src/conversation_search/adapters/base.py
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Literal, Optional, Tuple


@dataclass
class ParsedMessage:
    uuid: str
    parent_uuid: Optional[str]
    session_id: str
    timestamp: str
    role: Literal["user", "assistant"]
    content: str
    source: str
    is_sidechain: bool = False
    project_path: str = ""
    conversation_file: str = ""


@dataclass
class ConversationMeta:
    session_id: str
    source: str
    project_path: str
    conversation_file: str
    summary: Optional[str]
    leaf_uuid: Optional[str]


class BaseAdapter(ABC):
    source: str  # subclasses set this as a class attribute

    @abstractmethod
    def scan(self, days_back: Optional[int]) -> List[Path]:
        """Return paths to transcript files to index."""

    @abstractmethod
    def parse(self, file_path: Path) -> Tuple[ConversationMeta, List[ParsedMessage]]:
        """Parse a transcript file into (meta, messages)."""
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/adapters/test_base_types.py -v
```
Expected: `2 passed`

- [ ] **Step 5: Commit**

```bash
cd /home/laurynas/github/agent-recall && git add tests/adapters/ src/conversation_search/adapters/
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" git commit --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_t1.txt
```
(Write commit message to `/tmp/commit_t1.txt` first: `feat: add adapter base types (ParsedMessage, ConversationMeta, BaseAdapter)`)

---

## Task 2: Implement `ClaudeAdapter`

**Files:**
- Create: `src/conversation_search/adapters/claude.py`
- Create: `tests/adapters/test_claude_adapter.py`

The `ClaudeAdapter` extracts the scan + parse logic currently embedded in `ConversationIndexer` (`scan_conversations`, `parse_conversation_file`). The content extraction per message (tool blocks, text blocks) moves here too.

- [ ] **Step 1: Write the failing tests**

```python
# tests/adapters/test_claude_adapter.py
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
            "type": "assistant",
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

    assistant_msg = messages[1]
    assert assistant_msg.uuid == "msg-2"
    assert assistant_msg.role == "assistant"
    assert "I am doing well!" in assistant_msg.content


def test_parse_flattens_tool_use_blocks(tmp_path):
    conv_file = tmp_path / "session2.jsonl"
    _write_jsonl(conv_file, [
        {"type": "summary", "summary": "Tool use session", "leafUuid": "leaf-2"},
        {
            "uuid": "msg-3",
            "parentUuid": None,
            "isSidechain": False,
            "timestamp": "2024-01-15T11:00:00Z",
            "type": "assistant",
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


def test_parse_skips_non_user_assistant_types(tmp_path):
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
            "type": "tool",   # not user/assistant
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

    # Create two files: one recent, one old
    recent = projects_dir / "recent.jsonl"
    recent.write_text('{"type": "summary"}\n')
    old = projects_dir / "old.jsonl"
    old.write_text('{"type": "summary"}\n')

    # Set old file mtime to 10 days ago
    old_time = (datetime.now() - timedelta(days=10)).timestamp()
    os.utime(old, (old_time, old_time))

    monkeypatch.setattr(Path, "home", lambda: tmp_path)

    adapter = ClaudeAdapter()
    paths = adapter.scan(days_back=7)

    assert recent in paths
    assert old not in paths
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/adapters/test_claude_adapter.py -v
```
Expected: `ImportError` or `ModuleNotFoundError`

- [ ] **Step 3: Implement `ClaudeAdapter`**

```python
# src/conversation_search/adapters/claude.py
import json
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Optional, Tuple

from conversation_search.adapters.base import BaseAdapter, ConversationMeta, ParsedMessage


class ClaudeAdapter(BaseAdapter):
    source = "claude"

    def scan(self, days_back: Optional[int]) -> List[Path]:
        projects_dir = Path.home() / ".claude" / "projects"
        if not projects_dir.exists():
            return []

        cutoff = None
        if days_back is not None:
            cutoff = datetime.now() - timedelta(days=days_back)

        paths = []
        for project_dir in projects_dir.iterdir():
            if not project_dir.is_dir():
                continue
            for conv_file in project_dir.glob("*.jsonl"):
                if conv_file.stem.startswith("agent-"):
                    continue
                if cutoff:
                    mtime = datetime.fromtimestamp(conv_file.stat().st_mtime)
                    if mtime < cutoff:
                        continue
                paths.append(conv_file)

        return sorted(paths, key=lambda p: p.stat().st_mtime, reverse=True)

    def parse(self, file_path: Path) -> Tuple[ConversationMeta, List[ParsedMessage]]:
        messages: List[ParsedMessage] = []
        summary_line = None

        with open(file_path, "r") as f:
            for line_num, line in enumerate(f, 1):
                line = line.strip()
                if not line:
                    continue
                try:
                    data = json.loads(line)
                except json.JSONDecodeError:
                    continue

                if line_num == 1 and data.get("type") == "summary":
                    summary_line = data
                    continue

                if "uuid" not in data or "message" not in data:
                    continue

                role = data.get("type")
                if role not in ("user", "assistant"):
                    continue

                content = self._extract_content(data["message"].get("content", ""))
                session_id = data.get("sessionId", "")
                project_path = file_path.parent.name.replace("-", "/")

                messages.append(ParsedMessage(
                    uuid=data["uuid"],
                    parent_uuid=data.get("parentUuid"),
                    session_id=session_id,
                    timestamp=data.get("timestamp", ""),
                    role=role,
                    content=content,
                    source=self.source,
                    is_sidechain=data.get("isSidechain", False),
                    project_path=project_path,
                    conversation_file=str(file_path),
                ))

        session_id = messages[0].session_id if messages else file_path.stem
        project_path = file_path.parent.name.replace("-", "/")

        meta = ConversationMeta(
            session_id=session_id,
            source=self.source,
            project_path=project_path,
            conversation_file=str(file_path),
            summary=summary_line.get("summary") if summary_line else None,
            leaf_uuid=summary_line.get("leafUuid") if summary_line else None,
        )
        return meta, messages

    def _extract_content(self, content) -> str:
        if isinstance(content, str):
            return content
        if not isinstance(content, list):
            return str(content)

        parts = []
        for block in content:
            if not isinstance(block, dict):
                continue
            block_type = block.get("type")
            if block_type == "text":
                parts.append(block.get("text", ""))
            elif block_type == "thinking":
                pass  # skip thinking blocks from content; they're indexed separately if desired
            elif block_type == "tool_use":
                tool_name = block.get("name", "unknown")
                parts.append(f"[Tool: {tool_name}]")
                tool_input = block.get("input", {})
                if isinstance(tool_input, dict) and "command" in tool_input:
                    parts.append(tool_input["command"])
            elif block_type == "tool_result":
                parts.append("[Tool result]")
        return "\n".join(parts)
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/adapters/test_claude_adapter.py -v
```
Expected: `5 passed`

- [ ] **Step 5: Commit**

Write to `/tmp/commit_t2.txt`: `feat: add ClaudeAdapter with scan/parse extracted from indexer`
```bash
cd /home/laurynas/github/agent-recall && git add src/conversation_search/adapters/claude.py tests/adapters/test_claude_adapter.py
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" git commit --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_t2.txt
```

---

## Task 3: Implement `CodexAdapter`

**Files:**
- Create: `src/conversation_search/adapters/codex.py`
- Create: `tests/adapters/test_codex_adapter.py`

Codex CLI saves rollouts to `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`. Each line is a `RolloutLine` record. Based on the OpenAI Responses API format Codex uses, lines have shape:
```json
{"id": "...", "type": "message", "role": "user"|"assistant", "content": [{"type": "input_text"|"output_text", "text": "..."}], "created_at": 1234567890}
```
The adapter is lenient: it skips unknown record types and logs (doesn't crash) on malformed lines.

- [ ] **Step 1: Write the failing tests**

```python
# tests/adapters/test_codex_adapter.py
import json
import tempfile
from datetime import datetime
from pathlib import Path
from conversation_search.adapters.codex import CodexAdapter


def _write_jsonl(path: Path, lines: list) -> None:
    with open(path, "w") as f:
        for line in lines:
            f.write(json.dumps(line) + "\n")


def test_parse_basic_conversation(tmp_path):
    rollout = tmp_path / "rollout-abc123.jsonl"
    ts = int(datetime(2024, 2, 1, 10, 0, 0).timestamp())
    _write_jsonl(rollout, [
        {
            "id": "msg-1",
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "What is 2+2?"}],
            "created_at": ts,
        },
        {
            "id": "msg-2",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "It is 4."}],
            "created_at": ts + 1,
        },
    ])
    adapter = CodexAdapter()
    meta, messages = adapter.parse(rollout)

    assert meta.source == "codex"
    assert meta.session_id == "rollout-abc123"
    assert len(messages) == 2

    assert messages[0].role == "user"
    assert messages[0].content == "What is 2+2?"
    assert messages[0].source == "codex"
    assert messages[0].uuid == "msg-1"

    assert messages[1].role == "assistant"
    assert "4" in messages[1].content


def test_parse_skips_unknown_types(tmp_path):
    rollout = tmp_path / "rollout-def456.jsonl"
    ts = int(datetime(2024, 2, 1, 11, 0, 0).timestamp())
    _write_jsonl(rollout, [
        {
            "id": "msg-1",
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "Hello"}],
            "created_at": ts,
        },
        {
            "id": "func-1",
            "type": "function_call",
            "name": "bash",
            "arguments": '{"command": "ls"}',
            "created_at": ts + 1,
        },
        {
            "id": "func-out-1",
            "type": "function_call_output",
            "output": "file1.txt\nfile2.txt",
            "created_at": ts + 2,
        },
    ])
    adapter = CodexAdapter()
    meta, messages = adapter.parse(rollout)

    assert len(messages) == 1
    assert messages[0].uuid == "msg-1"


def test_parse_tolerates_malformed_lines(tmp_path):
    rollout = tmp_path / "rollout-ghi789.jsonl"
    with open(rollout, "w") as f:
        f.write('{"id": "msg-1", "type": "message", "role": "user", "content": [{"type": "input_text", "text": "OK"}], "created_at": 1706778000}\n')
        f.write("NOT JSON AT ALL\n")
        f.write('{"id": "msg-2", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Fine"}], "created_at": 1706778001}\n')
    adapter = CodexAdapter()
    meta, messages = adapter.parse(rollout)
    assert len(messages) == 2


def test_scan_finds_rollout_files(tmp_path, monkeypatch):
    import os
    sessions_dir = tmp_path / ".codex" / "sessions" / "2024" / "02" / "01"
    sessions_dir.mkdir(parents=True)
    rollout = sessions_dir / "rollout-abc.jsonl"
    rollout.write_text("{}\n")

    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    adapter = CodexAdapter()
    paths = adapter.scan(days_back=None)
    assert rollout in paths
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/adapters/test_codex_adapter.py -v
```
Expected: `ImportError`

- [ ] **Step 3: Implement `CodexAdapter`**

```python
# src/conversation_search/adapters/codex.py
import json
import logging
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import List, Optional, Tuple

from conversation_search.adapters.base import BaseAdapter, ConversationMeta, ParsedMessage

logger = logging.getLogger(__name__)


class CodexAdapter(BaseAdapter):
    source = "codex"

    def scan(self, days_back: Optional[int]) -> List[Path]:
        sessions_dir = Path.home() / ".codex" / "sessions"
        if not sessions_dir.exists():
            return []

        cutoff = None
        if days_back is not None:
            cutoff = datetime.now() - timedelta(days=days_back)

        paths = []
        for rollout in sessions_dir.rglob("rollout-*.jsonl"):
            if cutoff:
                mtime = datetime.fromtimestamp(rollout.stat().st_mtime)
                if mtime < cutoff:
                    continue
            paths.append(rollout)

        return sorted(paths, key=lambda p: p.stat().st_mtime, reverse=True)

    def parse(self, file_path: Path) -> Tuple[ConversationMeta, List[ParsedMessage]]:
        session_id = file_path.stem  # e.g. "rollout-abc123"
        messages: List[ParsedMessage] = []

        with open(file_path, "r") as f:
            for line_num, line in enumerate(f, 1):
                line = line.strip()
                if not line:
                    continue
                try:
                    data = json.loads(line)
                except json.JSONDecodeError:
                    logger.warning("Codex: malformed JSON on line %d of %s", line_num, file_path)
                    continue

                if data.get("type") != "message":
                    continue

                role = data.get("role")
                if role not in ("user", "assistant"):
                    continue

                content = self._extract_content(data.get("content", ""))
                msg_id = data.get("id", f"{session_id}-{line_num}")
                created_at = data.get("created_at")
                timestamp = (
                    datetime.fromtimestamp(created_at, tz=timezone.utc).isoformat()
                    if isinstance(created_at, (int, float))
                    else str(created_at or "")
                )

                messages.append(ParsedMessage(
                    uuid=msg_id,
                    parent_uuid=messages[-1].uuid if messages else None,
                    session_id=session_id,
                    timestamp=timestamp,
                    role=role,
                    content=content,
                    source=self.source,
                    is_sidechain=False,
                    project_path=str(file_path.parent),
                    conversation_file=str(file_path),
                ))

        meta = ConversationMeta(
            session_id=session_id,
            source=self.source,
            project_path=str(file_path.parent),
            conversation_file=str(file_path),
            summary=None,
            leaf_uuid=messages[-1].uuid if messages else None,
        )
        return meta, messages

    def _extract_content(self, content) -> str:
        if isinstance(content, str):
            return content
        if not isinstance(content, list):
            return str(content)
        parts = []
        for block in content:
            if isinstance(block, dict):
                text = block.get("text", "")
                if text:
                    parts.append(text)
        return "\n".join(parts)
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/adapters/test_codex_adapter.py -v
```
Expected: `4 passed`

- [ ] **Step 5: Commit**

Write to `/tmp/commit_t3.txt`: `feat: add CodexAdapter for ~/.codex/sessions/ rollout files`
```bash
cd /home/laurynas/github/agent-recall && git add src/conversation_search/adapters/codex.py tests/adapters/test_codex_adapter.py
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" git commit --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_t3.txt
```

---

## Task 4: Implement `GeminiAdapter`

**Files:**
- Create: `src/conversation_search/adapters/gemini.py`
- Create: `tests/adapters/test_gemini_adapter.py`

Gemini CLI (v0.20.0+) saves chat sessions to `~/.gemini/tmp/<project_hash>/chats/`. Each file is a JSON array. Records have shape:
```json
[
  {"role": "user", "parts": [{"text": "..."}], "timestamp": "2024-01-01T00:00:00Z"},
  {"role": "model", "parts": [{"text": "..."}], "timestamp": "2024-01-01T00:00:01Z"}
]
```
The adapter normalises `"model"` role to `"assistant"`. Format stability is lower than Claude/Codex — be lenient (missing fields → skip, unknown fields → ignore).

- [ ] **Step 1: Write the failing tests**

```python
# tests/adapters/test_gemini_adapter.py
import json
import tempfile
from pathlib import Path
from conversation_search.adapters.gemini import GeminiAdapter


def test_parse_basic_conversation(tmp_path):
    chat_file = tmp_path / "chat-20240201-100000.json"
    chat_file.write_text(json.dumps([
        {
            "role": "user",
            "parts": [{"text": "What is the capital of France?"}],
            "timestamp": "2024-02-01T10:00:00Z",
        },
        {
            "role": "model",
            "parts": [{"text": "Paris."}],
            "timestamp": "2024-02-01T10:00:01Z",
        },
    ]))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)

    assert meta.source == "gemini"
    assert len(messages) == 2

    assert messages[0].role == "user"
    assert messages[0].content == "What is the capital of France?"
    assert messages[0].source == "gemini"

    assert messages[1].role == "assistant"  # normalised from "model"
    assert "Paris" in messages[1].content


def test_parse_concatenates_multi_part(tmp_path):
    chat_file = tmp_path / "chat-multi.json"
    chat_file.write_text(json.dumps([
        {
            "role": "user",
            "parts": [{"text": "Part one."}, {"text": "Part two."}],
            "timestamp": "2024-02-01T11:00:00Z",
        },
    ]))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)
    assert "Part one." in messages[0].content
    assert "Part two." in messages[0].content


def test_parse_tolerates_missing_timestamp(tmp_path):
    chat_file = tmp_path / "chat-no-ts.json"
    chat_file.write_text(json.dumps([
        {"role": "user", "parts": [{"text": "Hello"}]},
    ]))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)
    assert len(messages) == 1
    assert messages[0].timestamp == ""


def test_parse_skips_unknown_roles(tmp_path):
    chat_file = tmp_path / "chat-roles.json"
    chat_file.write_text(json.dumps([
        {"role": "user", "parts": [{"text": "Hi"}], "timestamp": "2024-02-01T12:00:00Z"},
        {"role": "system", "parts": [{"text": "System prompt"}], "timestamp": "2024-02-01T12:00:00Z"},
        {"role": "model", "parts": [{"text": "Hello!"}], "timestamp": "2024-02-01T12:00:01Z"},
    ]))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)
    assert len(messages) == 2
    assert messages[0].role == "user"
    assert messages[1].role == "assistant"


def test_parse_tolerates_invalid_json(tmp_path):
    chat_file = tmp_path / "chat-bad.json"
    chat_file.write_text("this is not json")
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)
    assert messages == []


def test_scan_finds_chat_files(tmp_path, monkeypatch):
    chats_dir = tmp_path / ".gemini" / "tmp" / "proj123" / "chats"
    chats_dir.mkdir(parents=True)
    chat = chats_dir / "chat-20240201-100000.json"
    chat.write_text("[]")

    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    adapter = GeminiAdapter()
    paths = adapter.scan(days_back=None)
    assert chat in paths
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/adapters/test_gemini_adapter.py -v
```
Expected: `ImportError`

- [ ] **Step 3: Implement `GeminiAdapter`**

```python
# src/conversation_search/adapters/gemini.py
import json
import logging
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Optional, Tuple

from conversation_search.adapters.base import BaseAdapter, ConversationMeta, ParsedMessage

logger = logging.getLogger(__name__)


class GeminiAdapter(BaseAdapter):
    source = "gemini"

    def scan(self, days_back: Optional[int]) -> List[Path]:
        gemini_dir = Path.home() / ".gemini" / "tmp"
        if not gemini_dir.exists():
            return []

        cutoff = None
        if days_back is not None:
            cutoff = datetime.now() - timedelta(days=days_back)

        paths = []
        for chat_file in gemini_dir.rglob("*.json"):
            if "chats" not in chat_file.parts:
                continue
            if cutoff:
                mtime = datetime.fromtimestamp(chat_file.stat().st_mtime)
                if mtime < cutoff:
                    continue
            paths.append(chat_file)

        return sorted(paths, key=lambda p: p.stat().st_mtime, reverse=True)

    def parse(self, file_path: Path) -> Tuple[ConversationMeta, List[ParsedMessage]]:
        session_id = file_path.stem
        project_path = file_path.parent.parent.name  # project hash directory
        messages: List[ParsedMessage] = []

        try:
            with open(file_path, "r") as f:
                data = json.load(f)
        except (json.JSONDecodeError, OSError) as e:
            logger.warning("Gemini: failed to parse %s: %s", file_path, e)
            meta = ConversationMeta(
                session_id=session_id,
                source=self.source,
                project_path=project_path,
                conversation_file=str(file_path),
                summary=None,
                leaf_uuid=None,
            )
            return meta, []

        if not isinstance(data, list):
            logger.warning("Gemini: expected list at top level in %s", file_path)
            data = []

        for i, record in enumerate(data):
            if not isinstance(record, dict):
                continue

            role = record.get("role")
            if role == "model":
                role = "assistant"
            if role not in ("user", "assistant"):
                continue

            content = self._extract_content(record.get("parts", []))
            if not content:
                continue

            timestamp = record.get("timestamp", "")

            messages.append(ParsedMessage(
                uuid=f"{session_id}-{i}",
                parent_uuid=f"{session_id}-{i-1}" if i > 0 else None,
                session_id=session_id,
                timestamp=timestamp,
                role=role,
                content=content,
                source=self.source,
                is_sidechain=False,
                project_path=project_path,
                conversation_file=str(file_path),
            ))

        meta = ConversationMeta(
            session_id=session_id,
            source=self.source,
            project_path=project_path,
            conversation_file=str(file_path),
            summary=None,
            leaf_uuid=messages[-1].uuid if messages else None,
        )
        return meta, messages

    def _extract_content(self, parts) -> str:
        if not isinstance(parts, list):
            return ""
        texts = [p.get("text", "") for p in parts if isinstance(p, dict) and p.get("text")]
        return "\n".join(texts)
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/adapters/test_gemini_adapter.py -v
```
Expected: `6 passed`

- [ ] **Step 5: Commit**

Write to `/tmp/commit_t4.txt`: `feat: add GeminiAdapter for ~/.gemini/tmp/ chat files`
```bash
cd /home/laurynas/github/agent-recall && git add src/conversation_search/adapters/gemini.py tests/adapters/test_gemini_adapter.py
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" git commit --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_t4.txt
```

---

## Task 5: Add `source` column to schema and migration

**Files:**
- Modify: `src/conversation_search/data/schema.sql`
- Create: `tests/test_schema_migration.py`

The `source` column uses `DEFAULT 'claude'` so existing rows (all Claude) are correct after migration.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_schema_migration.py
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
    """Simulate an existing DB without the source column."""
    db_path = tmp_path / "index.db"

    # Build a minimal DB without source column (as if it's a pre-migration DB)
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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/test_schema_migration.py -v
```
Expected: `FAILED` — `source` column missing.

- [ ] **Step 3: Update `schema.sql` to add `source` column**

In `src/conversation_search/data/schema.sql`, add `source TEXT NOT NULL DEFAULT 'claude'` to both `messages` and `conversations` tables:

```sql
-- In the messages table, after the `indexed_at` line and before the closing );
-- Replace the closing section of the messages table:
    -- Indexing
    indexed_at TEXT DEFAULT CURRENT_TIMESTAMP,

    -- Source CLI
    source TEXT NOT NULL DEFAULT 'claude',

    FOREIGN KEY (parent_uuid) REFERENCES messages(message_uuid)
);
```

And in the `conversations` table:
```sql
    message_count INTEGER DEFAULT 0,
    indexed_at TEXT DEFAULT CURRENT_TIMESTAMP,
    source TEXT NOT NULL DEFAULT 'claude',

    FOREIGN KEY (root_message_uuid) REFERENCES messages(message_uuid)
);
```

Also add indexes:
```sql
CREATE INDEX IF NOT EXISTS idx_source ON messages(source);
CREATE INDEX IF NOT EXISTS idx_conv_source ON conversations(source);
```

- [ ] **Step 4: Add migration to `ConversationIndexer._init_db`**

In `src/conversation_search/core/indexer.py`, after the existing `is_meta_conversation` migration block, add:

```python
        # Migration: Add source column if missing (for existing databases)
        for table in ("messages", "conversations"):
            try:
                self.conn.execute(
                    f"ALTER TABLE {table} ADD COLUMN source TEXT NOT NULL DEFAULT 'claude'"
                )
                if not self.quiet:
                    print(f"  Migrated {table}: added source column")
            except sqlite3.OperationalError:
                pass  # Column already exists
        self.conn.commit()
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/test_schema_migration.py -v
```
Expected: `2 passed`

- [ ] **Step 6: Commit**

Write to `/tmp/commit_t5.txt`: `feat: add source column to schema with migration for existing DBs`
```bash
cd /home/laurynas/github/agent-recall && git add src/conversation_search/data/schema.sql src/conversation_search/core/indexer.py tests/test_schema_migration.py
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" git commit --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_t5.txt
```

---

## Task 6: Refactor `ConversationIndexer` to use the adapter list

**Files:**
- Modify: `src/conversation_search/core/indexer.py`

The indexer currently has hard-coded Claude-specific logic in `scan_conversations` and `parse_conversation_file`. Refactor to:
1. Accept a list of adapters at init time.
2. Delegate scan + parse to the adapters.
3. Store `source` from the adapter on each message/conversation row.
4. Keep all other logic (meta-conversation detection, tree depth, incremental update) unchanged.

- [ ] **Step 1: Write integration test**

```python
# tests/test_indexer_adapter.py
import json
import sqlite3
import tempfile
from pathlib import Path
from conversation_search.adapters.claude import ClaudeAdapter
from conversation_search.core.indexer import ConversationIndexer


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
            "type": "assistant",
            "sessionId": session_id,
            "message": {"content": [{"type": "text", "text": "Hello back!"}]},
        }) + "\n")


def test_indexer_uses_adapter_parse(tmp_path, monkeypatch):
    db_path = tmp_path / "index.db"

    # Set up a fake ~/.claude/projects/ structure
    projects_dir = tmp_path / ".claude" / "projects" / "myproject"
    projects_dir.mkdir(parents=True)
    conv_file = projects_dir / "session-test.jsonl"
    _write_claude_jsonl(conv_file, "session-test-id")

    monkeypatch.setattr(Path, "home", lambda: tmp_path)

    adapter = ClaudeAdapter()
    indexer = ConversationIndexer(
        db_path=str(db_path),
        quiet=True,
        adapters=[adapter],
    )
    indexer.index_all(days_back=None)
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
    indexer.index_all(days_back=None)
    indexer.close()

    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    cursor.execute("SELECT source FROM conversations WHERE session_id = 'session-two-id'")
    row = cursor.fetchone()
    conn.close()

    assert row["source"] == "claude"
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/test_indexer_adapter.py -v
```
Expected: `TypeError` (unexpected `adapters` kwarg) or `FAILED`

- [ ] **Step 3: Refactor `ConversationIndexer`**

In `src/conversation_search/core/indexer.py`:

**3a.** Add adapter imports at the top (after existing imports):
```python
from typing import Dict, List, Optional, Tuple, TYPE_CHECKING
if TYPE_CHECKING:
    from conversation_search.adapters.base import BaseAdapter
```

Note: use lazy import inside methods to avoid circular issues, or import at top if no circularity.

Actually, the adapters don't import from `indexer.py`, so a direct import is fine:
```python
from conversation_search.adapters.base import BaseAdapter, ConversationMeta, ParsedMessage
from conversation_search.adapters.claude import ClaudeAdapter
from conversation_search.adapters.codex import CodexAdapter
from conversation_search.adapters.gemini import GeminiAdapter
```

**3b.** Update `__init__`:
```python
def __init__(
    self,
    db_path: str = "~/.conversation-search/index.db",
    quiet: bool = False,
    adapters: Optional[List["BaseAdapter"]] = None,
):
    ...
    if adapters is None:
        adapters = [ClaudeAdapter(), CodexAdapter(), GeminiAdapter()]
    self.adapters = adapters
```

**3c.** Replace `scan_conversations` with an adapter-aware version:
```python
def scan_all(self, days_back: Optional[int] = 1):
    """Return (file_path, adapter) pairs from all registered adapters."""
    pairs = []
    for adapter in self.adapters:
        for path in adapter.scan(days_back):
            pairs.append((path, adapter))
    return pairs
```

**3d.** Update `index_all` to use `scan_all`:
```python
def index_all(self, days_back: Optional[int] = 1, summarize: bool = True):
    pairs = self.scan_all(days_back)
    if not self.quiet:
        print(f"Found {len(pairs)} conversation files to index")
    for i, (file_path, adapter) in enumerate(pairs, 1):
        if not self.quiet:
            print(f"\n[{i}/{len(pairs)}]")
        try:
            self.index_conversation(file_path, adapter=adapter, summarize=summarize)
        except Exception as e:
            if not self.quiet:
                print(f"  Error indexing {file_path}: {e}")
                import traceback
                traceback.print_exc()
    if not self.quiet:
        print(f"\n✓ Indexing complete!")
```

**3e.** Update `index_conversation` signature and internal logic:
```python
def index_conversation(self, file_path: Path, adapter: "BaseAdapter" = None, summarize: bool = True):
    if adapter is None:
        # fallback: use ClaudeAdapter for backwards compatibility
        adapter = ClaudeAdapter()
    
    if not self.quiet:
        print(f"Indexing: {file_path}")

    conv_meta, messages = adapter.parse(file_path)
    ...
```

Replace the manual `project_path = file_path.parent.name.replace('-', '/')` and the manual session_id extraction with values from `conv_meta` and `messages[0]`:
```python
    if not messages:
        if not self.quiet:
            print(f"  No messages found in {file_path}")
        return

    session_id = conv_meta.session_id
    project_path = conv_meta.project_path
    source = conv_meta.source
```

**3f.** Update the INSERT statements to include `source`:

For `INSERT INTO conversations`:
```python
cursor.execute("""
    INSERT INTO conversations (
        session_id, project_path, conversation_file,
        root_message_uuid, leaf_message_uuid, conversation_summary,
        first_message_at, last_message_at, message_count, source
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
""", (
    session_id,
    project_path,
    str(file_path),
    messages[0].uuid,
    conv_meta.leaf_uuid,
    conv_meta.summary or "Untitled conversation",
    messages[0].timestamp,
    messages[-1].timestamp,
    len(messages),
    source,
))
```

For `INSERT INTO messages`:
```python
cursor.execute("""
    INSERT INTO messages (
        message_uuid, session_id, parent_uuid, is_sidechain,
        depth, timestamp, message_type, project_path,
        conversation_file, full_content, is_meta_conversation,
        is_tool_noise, source
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
""", (
    message.uuid,
    session_id,
    message.parent_uuid,
    message.is_sidechain,
    depths.get(message.uuid, 0),
    message.timestamp,
    message.role,
    project_path,
    str(file_path),
    message.content,
    message.get("is_meta_conversation", False),  # see note below
    message.uuid in tool_noise_uuids,
    source,
))
```

Note: `ParsedMessage` is a dataclass — use `message.is_sidechain` not `message['is_sidechain']`. The meta-conversation detection (`_mark_meta_conversations`) currently operates on dicts. Update it to work with `ParsedMessage` objects, or convert messages to dicts before passing. The simplest approach: convert `ParsedMessage` list to dicts for the parts of the indexer that still use dict access, keeping the existing meta-conversation detection working.

**Conversion helper (add as a private method):**
```python
def _to_dict(self, msg: "ParsedMessage") -> Dict:
    return {
        "uuid": msg.uuid,
        "parent_uuid": msg.parent_uuid,
        "session_id": msg.session_id,
        "timestamp": msg.timestamp,
        "message_type": msg.role,
        "content": msg.content,
        "is_sidechain": msg.is_sidechain,
        "is_meta_conversation": False,
    }
```

Call `msg_dicts = [self._to_dict(m) for m in messages]` before passing to `_mark_meta_conversations`, then reconcile the `is_meta_conversation` flag back onto the original `ParsedMessage` objects via a UUID lookup.

- [ ] **Step 4: Run all tests**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/ -v
```
Expected: all existing tests still pass + new ones pass. Pay attention to any test that directly calls `scan_conversations` or `parse_conversation_file` on the indexer — update those calls to use the adapter API.

- [ ] **Step 5: Commit**

Write to `/tmp/commit_t6.txt`: `refactor: ConversationIndexer drives adapter list; stores source column`
```bash
cd /home/laurynas/github/agent-recall && git add src/conversation_search/core/indexer.py tests/test_indexer_adapter.py
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" git commit --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_t6.txt
```

---

## Task 7: Add `source` filter to `ConversationSearch`

**Files:**
- Modify: `src/conversation_search/core/search.py`

- [ ] **Step 1: Write the failing test**

```python
# tests/test_source_filter.py
import json
import sqlite3
import tempfile
from pathlib import Path
from conversation_search.core.search import ConversationSearch


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
            ("uuid-c1", "sess-claude", "2024-04-01T10:00:00Z", "user", "authentication bug in Claude session", "claude"),
            ("uuid-x1", "sess-codex",  "2024-04-01T11:00:00Z", "user", "authentication bug in Codex session",  "codex"),
            ("uuid-g1", "sess-gemini", "2024-04-01T12:00:00Z", "user", "authentication bug in Gemini session", "gemini"),
        ],
    )
    conn.executemany(
        "INSERT INTO conversations (session_id, last_message_at, source) VALUES (?, ?, ?)",
        [
            ("sess-claude", "2024-04-01T10:00:00Z", "claude"),
            ("sess-codex",  "2024-04-01T11:00:00Z", "codex"),
            ("sess-gemini", "2024-04-01T12:00:00Z", "gemini"),
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
    assert sources == {"claude", "codex", "gemini"}


def test_search_source_filter_claude(tmp_path):
    db_path = str(tmp_path / "index.db")
    _build_db(db_path)
    search = ConversationSearch(db_path=db_path)
    results = search.search_conversations("authentication bug", source="claude")
    search.close()
    assert all(r["source"] == "claude" for r in results)
    assert len(results) == 1


def test_list_source_filter(tmp_path):
    db_path = str(tmp_path / "index.db")
    _build_db(db_path)
    search = ConversationSearch(db_path=db_path)
    results = search.list_recent_conversations(source="codex")
    search.close()
    assert all(r["source"] == "codex" for r in results)
    assert len(results) == 1
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/test_source_filter.py -v
```
Expected: `TypeError: search_conversations() got an unexpected keyword argument 'source'`

- [ ] **Step 3: Add `source` parameter to `search_conversations` and `list_recent_conversations`**

In `src/conversation_search/core/search.py`:

In `search_conversations`, add `source: Optional[str] = None` to the signature and add this block just before the `ORDER BY` line:
```python
        if source:
            sql += " AND m.source = ?"
            params.append(source)
```

In `list_recent_conversations`, add `source: Optional[str] = None` to the signature and add:
```python
        if source:
            sql += " AND source = ?"
            params.append(source)
```

Also ensure both SELECT statements include `m.source` in the column list. For `search_conversations`:
```python
                    m.source,
```
(add after `c.conversation_file` in the SELECT).

- [ ] **Step 4: Run all tests**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/ -v
```
Expected: all tests pass.

- [ ] **Step 5: Commit**

Write to `/tmp/commit_t7.txt`: `feat: add source filter to search_conversations and list_recent_conversations`
```bash
cd /home/laurynas/github/agent-recall && git add src/conversation_search/core/search.py tests/test_source_filter.py
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" git commit --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_t7.txt
```

---

## Task 8: Smoke test — run Claude indexing end-to-end

This task verifies the whole stack works on real data before calling the branch done.

- [ ] **Step 1: Run the full test suite one more time**

```bash
cd /home/laurynas/github/agent-recall && pytest tests/ -v
```
Expected: all pass.

- [ ] **Step 2: Install the package in development mode**

```bash
cd /home/laurynas/github/agent-recall && pip install -e .
```

- [ ] **Step 3: Run a real index + search cycle**

```bash
cc-conversation-search index --days 3
cc-conversation-search search "agent-recall" --days 3
```
Expected:
- `index` prints "Found N conversation files to index" and "✓ Indexing complete!" without errors.
- `search` returns results (or "Found 0 matches" — either is fine as long as no exception).

- [ ] **Step 4: Verify `source` column is populated**

```bash
python3 -c "
import sqlite3
conn = sqlite3.connect(str(__import__('pathlib').Path.home() / '.conversation-search' / 'index.db'))
conn.row_factory = sqlite3.Row
c = conn.cursor()
c.execute('SELECT source, COUNT(*) FROM messages GROUP BY source')
for row in c.fetchall():
    print(dict(row))
conn.close()
"
```
Expected: `{'source': 'claude', 'COUNT(*)': <N>}` — all rows have `source = 'claude'`.

- [ ] **Step 5: Final commit if any cleanup was needed**

If Step 3-4 revealed any bugs, fix them, then:
```bash
GIT_COMMITTER_NAME="Claude Code" GIT_COMMITTER_EMAIL="noreply@anthropic.com" git commit --author="Claude Code <noreply@anthropic.com>" -F /tmp/commit_smoke.txt
```
(Message: `fix: resolve issues found during end-to-end smoke test`)

---

## Self-Review

**Spec coverage check:**
- ✅ Pluggable adapter pattern (`ParsedMessage`, `BaseAdapter`) — Task 1
- ✅ `ClaudeAdapter` (move existing logic) — Task 2
- ✅ `CodexAdapter` (~80 LoC) — Task 3
- ✅ `GeminiAdapter` (~80 LoC) — Task 4
- ✅ `source` column + schema rebuild + one-shot migration — Task 5
- ✅ Indexer drives adapter list — Task 6
- ✅ Source filter at query time — Task 7
- ✅ Claude indexing works at the end — Task 8

**Type consistency check:**
- `ParsedMessage.role` is `Literal["user", "assistant"]` — all adapters normalise `"model"` → `"assistant"` before constructing `ParsedMessage`. ✅
- `ConversationMeta.leaf_uuid` and `summary` are `Optional[str]` — all adapters use `None` fallback. ✅
- `ConversationIndexer._to_dict` maps `msg.role` → `message_type` for meta-conversation detection. ✅
- `INSERT INTO messages` uses `message.role` (via `_to_dict` round-trip or direct access after refactor). Must be consistent — verify in Task 6 Step 4. ✅

**Placeholder scan:** No TBD/TODO items in the task steps. ✅
