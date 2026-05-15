#!/usr/bin/env python3
"""Unified CLI for agent-recall"""

import argparse
import json
import os
import shlex
import sys
from datetime import datetime
from importlib.metadata import version, PackageNotFoundError
from importlib.resources import files as _pkg_files
from pathlib import Path
from typing import Any, Dict, List, Union

from agent_recall.core.indexer import ConversationIndexer
from agent_recall.core.search import ConversationSearch, format_timestamp

try:
    __version__ = version("agent-recall")
except PackageNotFoundError:
    __version__ = "dev"

# Configurable Claude command (default: 'claude')
# Set AGENT_RECALL_CMD env var to override (e.g., 'clauded' for alias)
CLAUDE_CMD = os.environ.get('AGENT_RECALL_CMD', 'claude')


def localize_timestamps(data: Any) -> Any:
    """Recursively convert UTC ISO timestamps to local timezone"""
    if isinstance(data, list):
        return [localize_timestamps(item) for item in data]
    elif isinstance(data, dict):
        result = {}
        for key, value in data.items():
            # Convert timestamp fields from UTC to local
            if key in ('timestamp', 'first_message_at', 'last_message_at', 'indexed_at'):
                if isinstance(value, str) and value.endswith('Z'):
                    dt_utc = datetime.fromisoformat(value.replace('Z', '+00:00'))
                    dt_local = dt_utc.astimezone()
                    result[key] = dt_local.isoformat()
                else:
                    result[key] = value
            else:
                result[key] = localize_timestamps(value) if isinstance(value, (dict, list)) else value
        return result
    else:
        return data


def cmd_init(args):
    """Initialize the database and run initial indexing"""
    quiet = args.quiet

    if not quiet:
        print("Agent Recall - Initializing")
        print("=" * 50)

    db_path = Path.home() / ".agent-recall" / "index.db"

    if db_path.exists() and not args.force:
        if not quiet:
            print(f"✓ Database already exists: {db_path}")
            print("  Use --force to reinitialize")
        return

    if not quiet:
        print(f"Creating database: {db_path}")
    indexer = ConversationIndexer(db_path=str(db_path), quiet=quiet)

    days = args.days
    if not quiet:
        print(f"\nIndexing conversations from last {days} days...")
    
    indexer.index_new()

    if not quiet:
        print(f"\n✓ Initialization complete!")
        print(f"  Database: {db_path}")
        print(f"\nNext steps:")
        print(f"  • Search conversations: agent-recall search '<query>'")
        print(f"  • List recent: agent-recall list")
        print(f"  • Re-index: agent-recall index")

    indexer.close()


def cmd_index(args):
    """Index conversations (JIT - fast without AI calls)"""
    quiet = args.quiet
    indexer = ConversationIndexer(quiet=quiet)

    indexer.index_new(days_back=args.days if not args.all else None)

    indexer.close()


def cmd_search(args):
    """Search conversations"""
    # Auto-index before searching to ensure fresh data
    if not getattr(args, 'no_index', False):
        indexer = ConversationIndexer(quiet=True)
        indexer.index_new()
        indexer.close()

    search = ConversationSearch()

    try:
        results = search.search_conversations(
            query=args.query,
            days_back=args.days,
            since=getattr(args, 'since', None),
            until=getattr(args, 'until', None),
            date=getattr(args, 'date', None),
            limit=args.limit,
            project_path=args.project,
            source=args.source
        )
    except Exception as e:
        print(f"Error: {e}")
        raise

    if args.json:
        print(json.dumps(localize_timestamps([dict(r) for r in results]), indent=2))
        return

    if not results:
        print(f"No results found for: {args.query}")
        return

    print(f"🔍 Found {len(results)} matches for '{args.query}':\n")

    for result in results:
        icon = "👤" if result['message_type'] == 'user' else "🤖"
        timestamp = format_timestamp(result['timestamp'])

        project_path = result['project_path']
        project_dir = project_path if project_path.startswith('/') else f"/{project_path}"

        print(f"{icon}  {result['conversation_summary']}")
        print(f"   Session: {result['session_id']}")
        print(f"   Project: {project_dir}")
        print(f"   Time: {timestamp}")
        print(f"   Message: {result['message_uuid']}")

        if args.content:
            content = search.get_full_message_content(result['message_uuid'])
            if content:
                print(f"\n   {content[:300]}...")
        else:
            print(f"\n   {result['context_snippet']}")

        print(f"\n   Resume:")
        print(f"     cd {project_dir}")
        print(f"     {CLAUDE_CMD} --resume {result['session_id']}")
        print()


def cmd_context(args):
    """Get context around a message"""
    # Auto-index recent conversations to ensure fresh data
    if not getattr(args, 'no_index', False):
        indexer = ConversationIndexer(quiet=True)
        indexer.index_new()
        indexer.close()

    search = ConversationSearch()

    try:
        result = search.get_conversation_context(
            message_uuid=args.uuid,
            depth=args.depth
        )
    except ValueError as e:
        if args.json:
            print(json.dumps({"error": str(e)}))
        else:
            print(f"Error: {e}")
        return

    if args.json:
        print(json.dumps(localize_timestamps(result), indent=2))
        return

    print(f"Context for message: {args.uuid}\n")

    # Show parents
    if result.get('ancestors'):
        print("📜 Parent messages:")
        for msg in result['ancestors']:
            icon = "👤" if msg.get('message_type') == 'user' else "🤖"
            print(f"  {icon} {msg.get('summary', 'No summary')}")
        print()

    # Show target message
    if result.get('message'):
        print("🎯 Target message:")
        msg = result['message']
        icon = "👤" if msg.get('message_type') == 'user' else "🤖"
        if args.content and msg.get('full_content'):
            print(f"  {icon} {msg['full_content']}")
        else:
            print(f"  {icon} {msg.get('summary', 'No summary')}")
        print()

    # Show children
    if result.get('children'):
        print("💬 Responses:")
        for msg in result['children']:
            icon = "👤" if msg.get('message_type') == 'user' else "🤖"
            print(f"  {icon} {msg.get('summary', 'No summary')}")


def cmd_list(args):
    """List recent conversations"""
    # Auto-index before listing to ensure fresh data
    if not getattr(args, 'no_index', False):
        indexer = ConversationIndexer(quiet=True)
        indexer.index_new()
        indexer.close()

    search = ConversationSearch()

    convs = search.list_recent_conversations(
        days_back=args.days,
        since=getattr(args, 'since', None),
        until=getattr(args, 'until', None),
        date=getattr(args, 'date', None),
        limit=args.limit,
        source=args.source
    )

    if args.json:
        print(json.dumps(localize_timestamps([dict(c) for c in convs]), indent=2))
        return

    if not convs:
        print("No conversations found")
        return

    if args.days is not None:
        print(f"Recent conversations (last {args.days} days):\n")
    elif getattr(args, 'since', None) or getattr(args, 'until', None) or getattr(args, 'date', None):
        print("Recent conversations (filtered by date):\n")
    else:
        print("Recent conversations:\n")

    for conv in convs:
        timestamp = format_timestamp(conv['last_message_at'])
        print(f"[{timestamp}] {conv['conversation_summary']}")
        print(f"  {conv['message_count']} messages")
        print(f"  {conv['project_path']}")
        print(f"  Session: {conv['session_id']}")
        print()


def cmd_tree(args):
    """Show conversation tree"""
    search = ConversationSearch()

    tree = search.get_conversation_tree(args.session_id)

    if args.json:
        print(json.dumps(localize_timestamps(tree), indent=2))
        return

    print(f"Conversation tree: {args.session_id}\n")

    if 'error' in tree:
        print(f"Error: {tree['error']}")
        return

    # Simple tree visualization
    def print_tree(nodes, indent=0):
        for node in nodes:
            icon = "👤" if node['message_type'] == 'user' else "🤖"
            prefix = "  " * indent
            summary = node['summary'][:80]
            print(f"{prefix}{icon} {summary}")
            if node.get('children'):
                print_tree(node['children'], indent + 1)

    print_tree(tree['tree'])


def cmd_resume(args):
    """Get session resumption commands for a message UUID"""
    search = ConversationSearch()

    # Get message info
    cursor = search.conn.cursor()
    cursor.execute("""
        SELECT m.session_id, m.project_path, m.timestamp, m.summary
        FROM messages m
        WHERE m.message_uuid = ?
    """, (args.uuid,))

    result = cursor.fetchone()

    if not result:
        print(f"Message not found: {args.uuid}")
        sys.exit(1)

    session_id = result['session_id']
    project_path = result['project_path']

    project_dir = project_path if project_path.startswith('/') else f"/{project_path}"

    print(f"cd {shlex.quote(project_dir)}")
    print(f"{CLAUDE_CMD} --resume {session_id}")


def cmd_install_skill(args):
    """Copy the bundled skill into the target CLI's skills directory."""
    target = args.target

    if target == "claude":
        dest = Path.home() / ".claude" / "skills" / "agent-recall"
    elif target == "gemini":
        print("Gemini CLI skill format is not yet implemented.")
        print("For Gemini, use:  agent-recall configure-mcp --target gemini")
        sys.exit(0)
    elif target == "codex":
        print("Codex skill format is not yet implemented.")
        sys.exit(0)
    else:
        print(f"Unknown target: {target}. Choose from: claude, gemini, codex")
        sys.exit(1)

    if dest.exists() and not args.force:
        print(f"Skill already installed at {dest}")
        print("Use --force to overwrite.")
        return

    dest.mkdir(parents=True, exist_ok=True)
    skill_pkg = _pkg_files("agent_recall") / "skill"
    for fname in ("SKILL.md", "REFERENCE.md"):
        content = (skill_pkg / fname).read_bytes()
        (dest / fname).write_bytes(content)
        print(f"  Installed {fname}")

    print(f"\nSkill installed at {dest}")
    if target == "claude":
        print("Restart Claude Code (or open a new session) to activate it.")


def cmd_configure_mcp(args):
    """Write the agent-recall MCP server entry into the target CLI's settings."""
    target = args.target

    if target == "claude":
        if args.project:
            settings_path = Path.cwd() / ".claude" / "settings.json"
        else:
            settings_path = Path.home() / ".claude" / "settings.json"
    elif target == "gemini":
        if args.project:
            settings_path = Path.cwd() / ".gemini" / "settings.json"
        else:
            settings_path = Path.home() / ".gemini" / "settings.json"
    else:
        print(f"Unknown target: {target}. Choose from: claude, gemini")
        sys.exit(1)

    settings_path.parent.mkdir(parents=True, exist_ok=True)

    if settings_path.exists():
        try:
            with open(settings_path) as f:
                content = f.read().strip()
            settings = json.loads(content) if content else {}
        except json.JSONDecodeError:
            print(f"Error: {settings_path} contains invalid JSON.")
            sys.exit(1)
    else:
        settings = {}

    settings.setdefault("mcpServers", {})
    if "agent-recall" in settings["mcpServers"] and not args.force:
        print(f"agent-recall MCP server already configured in {settings_path}")
        print("Use --force to overwrite.")
        return

    settings["mcpServers"]["agent-recall"] = {"command": "agent-recall-mcp"}

    with open(settings_path, "w") as f:
        json.dump(settings, f, indent=2)
        f.write("\n")

    print(f"MCP server configured in {settings_path}")
    print("Restart your CLI session for the change to take effect.")


def main():
    old_db = Path.home() / ".conversation-search" / "index.db"
    new_db = Path.home() / ".agent-recall" / "index.db"
    if old_db.exists() and not new_db.exists():
        print(
            "Note: database found at old path ~/.conversation-search/index.db\n"
            "Move it:        mv ~/.conversation-search/index.db ~/.agent-recall/index.db\n"
            "Or re-init:     agent-recall init\n"
        )

    parser = argparse.ArgumentParser(
        prog='agent-recall',
        description='Find and resume Claude Code conversations using semantic search'
    )
    parser.add_argument('--version', action='version', version=f'%(prog)s {__version__}')

    subparsers = parser.add_subparsers(dest='command', help='Command to run')

    # init command
    init_parser = subparsers.add_parser('init', help='Initialize database and index')
    init_parser.add_argument('--days', type=int, default=7, help='Days of history to index (default: 7)')
    init_parser.add_argument('--force', action='store_true', help='Reinitialize existing database')
    init_parser.add_argument('--quiet', action='store_true', help='Minimal output')
    init_parser.set_defaults(func=cmd_init)

    # index command
    index_parser = subparsers.add_parser('index', help='Index conversations (JIT - runs before search)')
    index_parser.add_argument('--days', type=int, default=1, help='Days back to index (default: 1)')
    index_parser.add_argument('--all', action='store_true', help='Index all conversations')
    index_parser.add_argument('--quiet', action='store_true', help='Minimal output')
    index_parser.set_defaults(func=cmd_index)

    # search command
    search_parser = subparsers.add_parser('search', help='Search conversations')
    search_parser.add_argument('query', help='Search query')
    search_parser.add_argument('--days', type=int, help='Limit to last N days')
    search_parser.add_argument('--since', help='Start date (YYYY-MM-DD, yesterday, today)')
    search_parser.add_argument('--until', help='End date (YYYY-MM-DD, yesterday, today)')
    search_parser.add_argument('--date', help='Specific date (YYYY-MM-DD, yesterday, today)')
    search_parser.add_argument('--project', help='Filter by project path')
    search_parser.add_argument('--source', help='Filter by source (claude, gemini)')
    search_parser.add_argument('--limit', type=int, default=20, help='Max results (default: 20)')
    search_parser.add_argument('--content', action='store_true', help='Show full content')
    search_parser.add_argument('--json', action='store_true', help='Output as JSON')
    search_parser.add_argument('--no-index', action='store_true', help='Skip auto-indexing (faster but may be stale)')
    search_parser.set_defaults(func=cmd_search)

    # context command
    context_parser = subparsers.add_parser('context', help='Get context around a message')
    context_parser.add_argument('uuid', help='Message UUID')
    context_parser.add_argument('--depth', type=int, default=3, help='Parent depth (default: 3)')
    context_parser.add_argument('--content', action='store_true', help='Show full content')
    context_parser.add_argument('--json', action='store_true', help='Output as JSON')
    context_parser.add_argument('--no-index', action='store_true', help='Skip auto-indexing (faster but may be stale)')
    context_parser.set_defaults(func=cmd_context)

    # list command
    list_parser = subparsers.add_parser('list', help='List recent conversations')
    list_parser.add_argument('--days', type=int, help='Days back (default: 7)')
    list_parser.add_argument('--since', help='Start date (YYYY-MM-DD, yesterday, today)')
    list_parser.add_argument('--until', help='End date (YYYY-MM-DD, yesterday, today)')
    list_parser.add_argument('--date', help='Specific date (YYYY-MM-DD, yesterday, today)')
    list_parser.add_argument('--source', help='Filter by source (claude, gemini)')
    list_parser.add_argument('--limit', type=int, default=20, help='Max results (default: 20)')
    list_parser.add_argument('--json', action='store_true', help='Output as JSON')
    list_parser.add_argument('--no-index', action='store_true', help='Skip auto-indexing (faster but may be stale)')
    list_parser.set_defaults(func=cmd_list)

    # tree command
    tree_parser = subparsers.add_parser('tree', help='Show conversation tree')
    tree_parser.add_argument('session_id', help='Session ID')
    tree_parser.add_argument('--json', action='store_true', help='Output as JSON')
    tree_parser.set_defaults(func=cmd_tree)

    # resume command
    resume_parser = subparsers.add_parser('resume', help='Get session resumption commands')
    resume_parser.add_argument('uuid', help='Message UUID')
    resume_parser.set_defaults(func=cmd_resume)

    # install-skill command
    install_skill_parser = subparsers.add_parser(
        'install-skill', help='Install the agent-recall skill into a CLI tools directory'
    )
    install_skill_parser.add_argument(
        '--target', default='claude', choices=['claude', 'gemini', 'codex'],
        help='Target CLI (default: claude)'
    )
    install_skill_parser.add_argument('--force', action='store_true', help='Overwrite existing skill')
    install_skill_parser.set_defaults(func=cmd_install_skill)

    # configure-mcp command
    configure_mcp_parser = subparsers.add_parser(
        'configure-mcp', help='Add agent-recall MCP server to a CLI settings file'
    )
    configure_mcp_parser.add_argument(
        '--target', default='claude', choices=['claude', 'gemini'],
        help='Target CLI (default: claude)'
    )
    configure_mcp_parser.add_argument(
        '--project', action='store_true',
        help='Write to project-level .claude/settings.json instead of global'
    )
    configure_mcp_parser.add_argument('--force', action='store_true', help='Overwrite existing entry')
    configure_mcp_parser.set_defaults(func=cmd_configure_mcp)

    args = parser.parse_args()

    if not args.command:
        parser.print_help()
        sys.exit(1)

    try:
        args.func(args)
    except FileNotFoundError as e:
        print(f"Error: {e}")
        print("\nThe agent-recall tool requires initialization.")
        print("Install: uv tool install agent-recall")
        print("Initialize: agent-recall init")
        sys.exit(1)
    except KeyboardInterrupt:
        print("\n\nInterrupted")
        sys.exit(0)
    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)


if __name__ == '__main__':
    main()
