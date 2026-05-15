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
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    data = json.loads(line)
                except json.JSONDecodeError:
                    continue

                if summary_line is None and not messages and data.get("type") == "summary":
                    summary_line = data
                    continue

                if "uuid" not in data or "message" not in data:
                    continue

                role = data.get("type")
                if role not in ("user", "ai"):
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
                pass  # skip
            elif block_type == "tool_use":
                tool_name = block.get("name", "unknown")
                parts.append(f"[Tool: {tool_name}]")
                tool_input = block.get("input", {})
                if isinstance(tool_input, dict) and "command" in tool_input:
                    parts.append(tool_input["command"])
            elif block_type == "tool_result":
                parts.append("[Tool result]")
        return "\n".join(parts)
