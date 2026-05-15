#!/usr/bin/env python3
"""Unified CLI for agent-recall"""

import argparse
import json
import os
import sys
from datetime import datetime
from importlib.metadata import version, PackageNotFoundError
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
        print("Conversation Search - Initializing")
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
    
    indexer.index_all(days_back=days, summarize=not args.no_extract)

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

    indexer.index_all(
        days_back=args.days if not args.all else None,
        summarize=not args.no_extract
    )

    indexer.close()


def cmd_search(args):
    """Search conversations"""
    # Auto-index before searching to ensure fresh data
    if not getattr(args, 'no_index', False):
        indexer = ConversationIndexer(quiet=True)
        # Index at least as far back as search range, minimum 30 days
        days_to_index = max(args.days if args.days else 30, 30)
        indexer.index_all(days_back=days_to_index, summarize=True)
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

        # Convert project_path hash to actual path
        project_dir = result['project_path'].replace('-', '/')
        if not project_dir.startswith('/'):
            project_dir = f"/{project_dir}"

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
        indexer.index_all(days_back=30, summarize=True)
        indexer.close()

    search = ConversationSearch()

    result = search.get_conversation_context(
        message_uuid=args.uuid,
        depth=args.depth
    )

    if args.json:
        print(json.dumps(localize_timestamps(result), indent=2))
        return

    print(f"Context for message: {args.uuid}\n")

    if 'error' in result:
        print(f"Error: {result['error']}")
        return

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
        days_to_index = max(args.days if args.days else 30, 30)
        indexer.index_all(days_back=days_to_index, summarize=True)
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

    print(f"Recent conversations (last {args.days} days):\n")

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

    # Convert project_path hash back to actual path
    project_dir = project_path.replace('-', '/')
    if not project_dir.startswith('/'):
        project_dir = f"/{project_dir}"

    print(f"cd {project_dir}")
    print(f"{CLAUDE_CMD} --resume {session_id}")


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
    init_parser.add_argument('--no-extract', action='store_true', help='Skip smart extraction (store only raw content)')
    init_parser.add_argument('--force', action='store_true', help='Reinitialize existing database')
    init_parser.add_argument('--quiet', action='store_true', help='Minimal output')
    init_parser.set_defaults(func=cmd_init)

    # index command
    index_parser = subparsers.add_parser('index', help='Index conversations (JIT - runs before search)')
    index_parser.add_argument('--days', type=int, default=1, help='Days back to index (default: 1)')
    index_parser.add_argument('--all', action='store_true', help='Index all conversations')
    index_parser.add_argument('--no-extract', action='store_true', help='Skip smart extraction')
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
