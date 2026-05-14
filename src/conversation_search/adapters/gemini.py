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
        project_path = file_path.parent.parent.name
        messages: List[ParsedMessage] = []

        try:
            with open(file_path, "r") as f:
                data = json.load(f)
        except (json.JSONDecodeError, OSError) as e:
            logger.warning("Gemini: failed to parse %s: %s", file_path, e)
            return self._empty_meta(session_id, project_path, file_path), []

        if not isinstance(data, list):
            logger.warning("Gemini: expected list at top level in %s", file_path)
            return self._empty_meta(session_id, project_path, file_path), []

        msg_index = 0  # counts only messages we actually include
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

            messages.append(ParsedMessage(
                uuid=f"{session_id}-{msg_index}",
                parent_uuid=f"{session_id}-{msg_index - 1}" if msg_index > 0 else None,
                session_id=session_id,
                timestamp=record.get("timestamp", ""),
                role=role,
                content=content,
                source=self.source,
                is_sidechain=False,
                project_path=project_path,
                conversation_file=str(file_path),
            ))
            msg_index += 1

        meta = ConversationMeta(
            session_id=session_id,
            source=self.source,
            project_path=project_path,
            conversation_file=str(file_path),
            summary=None,
            leaf_uuid=messages[-1].uuid if messages else None,
        )
        return meta, messages

    def _empty_meta(self, session_id: str, project_path: str, file_path: Path) -> ConversationMeta:
        return ConversationMeta(
            session_id=session_id,
            source=self.source,
            project_path=project_path,
            conversation_file=str(file_path),
            summary=None,
            leaf_uuid=None,
        )

    def _extract_content(self, parts) -> str:
        if not isinstance(parts, list):
            return ""
        texts = [p.get("text", "") for p in parts if isinstance(p, dict) and p.get("text")]
        return "\n".join(texts)
