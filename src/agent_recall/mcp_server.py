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
                "score": None,
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
        return cs.get_conversation_context(message_uuid=message_uuid, depth=window, include_children=True)
    finally:
        cs.close()


@mcp.tool(name="list")
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
