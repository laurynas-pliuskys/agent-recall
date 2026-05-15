from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import List, Literal, Optional, Tuple


@dataclass
class ParsedMessage:
    uuid: str
    parent_uuid: Optional[str]
    session_id: str
    timestamp: str
    role: Literal["user", "ai"]
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
    """
    Base class for conversation adapters.
    REVIEW: This abstraction is well-defined and allows for easy extension to other chat formats.
    """
    source: str  # subclasses set this as a class attribute

    @abstractmethod
    def scan(self, days_back: Optional[int]) -> List[Path]:
        """Return paths to transcript files to index."""

    @abstractmethod
    def parse(self, file_path: Path) -> Tuple[ConversationMeta, List[ParsedMessage]]:
        """Parse a transcript file into (meta, messages)."""
