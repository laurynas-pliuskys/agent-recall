#!/usr/bin/env python3
"""
Agent Recall Indexer
Scans ~/.claude/projects and indexes conversations with batch AI summarization
"""

import json
import os
import sqlite3
from pathlib import Path
from datetime import date, datetime, timedelta
from typing import Dict, List, Optional, Tuple

from importlib.resources import files
from agent_recall.adapters.base import BaseAdapter, ConversationMeta, ParsedMessage
from agent_recall.adapters.claude import ClaudeAdapter
from agent_recall.adapters.gemini import GeminiAdapter
from agent_recall.core.summarization import (
    MessageSummarizer,
    is_summarizer_conversation,
    message_uses_conversation_search
)


class ConversationIndexer:
    def __init__(
        self,
        db_path: str = "~/.agent-recall/index.db",
        quiet: bool = False,
        adapters=None,
    ):
        self.db_path = Path(db_path).expanduser()
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self.conn = sqlite3.connect(str(self.db_path), timeout=30.0)
        self.quiet = quiet
        if adapters is None:
            adapters = [ClaudeAdapter(), GeminiAdapter()]
        self.adapters = adapters

        # Enable WAL mode for concurrent access
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.execute("PRAGMA synchronous=NORMAL")
        self.conn.execute("PRAGMA busy_timeout=30000")  # 30 second busy timeout

        self.conn.row_factory = sqlite3.Row
        self._init_db()
        self.summarizer = MessageSummarizer(db_path=str(self.db_path))
        self._summarizer_project_hash = None

    def _init_db(self):
        """Initialize database with schema and run migrations"""
        # Run column migrations before executing the full schema so that
        # CREATE INDEX statements in schema.sql can reference new columns even
        # on pre-existing databases that don't have those columns yet.

        # Migration: Add is_meta_conversation if missing (for existing databases)
        try:
            self.conn.execute("""
                ALTER TABLE messages ADD COLUMN is_meta_conversation BOOLEAN DEFAULT FALSE
            """)
            if not self.quiet:
                print("  Migrated database: added is_meta_conversation column")
        except sqlite3.OperationalError:
            pass  # Column already exists

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

        schema_sql = files('agent_recall.data').joinpath('schema.sql').read_text()
        self.conn.executescript(schema_sql)

    def get_last_indexed_at(self) -> Optional[date]:
        """Return the date of the most recently indexed conversation, or None if DB is empty."""
        cursor = self.conn.cursor()
        cursor.execute("SELECT MAX(indexed_at) FROM conversations")
        row = cursor.fetchone()
        if row[0] is None:
            return None
        
        from datetime import timezone
        dt = datetime.fromisoformat(row[0])
        dt_utc = dt.replace(tzinfo=timezone.utc)
        return dt_utc.astimezone().date()

    def _get_summarizer_project_hash(self) -> Optional[str]:
        """Get the project hash for summarizer workspace by detection"""
        if self._summarizer_project_hash:
            return self._summarizer_project_hash

        projects_dir = Path.home() / ".claude" / "projects"
        if not projects_dir.exists():
            return None

        # Look for directories with summarizer conversations
        for project_dir in projects_dir.iterdir():
            if not project_dir.is_dir():
                continue

            for conv_file in list(project_dir.glob("*.jsonl"))[:5]:  # Check first 5
                try:
                    _, messages = self.parse_conversation_file(conv_file)
                    if is_summarizer_conversation(conv_file, messages):
                        self._summarizer_project_hash = project_dir.name
                        if not self.quiet:
                            print(f"  Detected summarizer project hash: {project_dir.name}")
                        return project_dir.name
                except:
                    continue

        return None

    def scan_all(self, days_back):
        """Return (file_path, adapter) pairs from all registered adapters."""
        pairs = []
        for adapter in self.adapters:
            for path in adapter.scan(days_back):
                pairs.append((path, adapter))
        return pairs

    def _to_msg_dict(self, msg: ParsedMessage) -> dict:
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

    def parse_conversation_file(self, file_path: Path) -> Tuple[Dict, List[Dict]]:
        """
        Parse a conversation JSONL file

        Returns:
            (conversation_metadata, messages_list)
        """
        messages = []
        conversation_meta = None

        with open(file_path, 'r') as f:
            for line_num, line in enumerate(f, 1):
                try:
                    data = json.loads(line.strip())

                    # First line is the summary
                    if line_num == 1 and data.get('type') == 'summary':
                        conversation_meta = data
                        continue

                    # Parse message entries
                    if 'uuid' in data and 'message' in data:
                        message_type = data.get('type', 'unknown')
                        if message_type not in ('user', 'ai'):
                            continue

                        # Extract content
                        msg_content = data['message'].get('content', '')
                        if isinstance(msg_content, list):
                            # Flatten content blocks
                            text_parts = []
                            for block in msg_content:
                                if isinstance(block, dict):
                                    if block.get('type') == 'text':
                                        text_parts.append(block.get('text', ''))
                                    elif block.get('type') == 'thinking':
                                        continue
                                    elif block.get('type') == 'tool_use':
                                        tool_name = block.get('name', 'unknown')
                                        text_parts.append(f"[Tool: {tool_name}]")
                                        # Include tool input for detection (especially for Bash commands)
                                        tool_input = block.get('input', {})
                                        if isinstance(tool_input, dict) and 'command' in tool_input:
                                            text_parts.append(tool_input['command'])
                                    elif block.get('type') == 'tool_result':
                                        text_parts.append("[Tool result]")
                            msg_content = '\n'.join(text_parts)

                        messages.append({
                            'uuid': data['uuid'],
                            'parent_uuid': data.get('parentUuid'),
                            'is_sidechain': data.get('isSidechain', False),
                            'timestamp': data.get('timestamp'),
                            'message_type': message_type,
                            'content': msg_content,
                            'session_id': data.get('sessionId'),
                        })

                except json.JSONDecodeError as e:
                    if not self.quiet:
                        print(f"Error parsing line {line_num} in {file_path}: {e}")
                    continue

        return conversation_meta, messages

    def calculate_depth(self, messages: List[Dict], parent_map: Dict[str, str]) -> Dict[str, int]:
        """Calculate depth of each message from root"""
        depths = {}

        # Find roots (messages with no parent)
        roots = [m['uuid'] for m in messages if not m['parent_uuid']]

        # BFS to calculate depths
        queue = [(root_uuid, 0) for root_uuid in roots]
        while queue:
            uuid, depth = queue.pop(0)
            depths[uuid] = depth

            # Find children
            children = [m['uuid'] for m in messages if m['parent_uuid'] == uuid]
            for child_uuid in children:
                queue.append((child_uuid, depth + 1))

        return depths

    def _mark_ancestor_chain_to_user(self, search_message_uuid: str, msg_map: Dict, meta_uuids: set) -> None:
        """
        Walk up the message tree from a search message to the originating user message.

        Marks all messages in the ancestor chain as meta-conversations, stopping when
        we reach a user message with actual content (not tool results or infrastructure).

        Args:
            search_message_uuid: UUID of the message that uses conversation-search
            msg_map: Dictionary mapping UUID -> message for O(1) lookups
            meta_uuids: Set of marked UUIDs (mutated in place)
        """
        current_uuid = search_message_uuid
        visited = set()  # Cycle detection

        while current_uuid:
            # Safety: detect cycles
            if current_uuid in visited:
                break
            visited.add(current_uuid)

            # Safety: handle orphaned messages
            current = msg_map.get(current_uuid)
            if not current:
                break

            # Mark this message
            meta_uuids.add(current_uuid)
            current['is_meta_conversation'] = True

            # Stop at first REAL user message (not tool results/infrastructure)
            if current.get('message_type') == 'user':
                content = current.get('content', '').strip()
                # Skip system-generated user messages
                if (not content.startswith('[Tool') and
                    not content.startswith('<command-message>') and
                    not content.startswith('Base directory')):
                    break  # Found real user message

            # Continue walking up
            current_uuid = current.get('parent_uuid')

    def _mark_descendant_chain(self, search_message_uuid: str, children_map: Dict, msg_map: Dict, meta_uuids: set) -> None:
        """
        Walk down from search message to mark search results and descendants.

        Marks descendants of the search message (tool results, processing, answer) until
        we hit a real user message (indicating follow-up work) or conversation ends.

        Args:
            search_message_uuid: UUID of the message that uses conversation-search
            children_map: Dictionary mapping parent_uuid -> list of child UUIDs
            msg_map: Dictionary mapping UUID -> message for O(1) lookups
            meta_uuids: Set of marked UUIDs (mutated in place)
        """
        current_uuid = search_message_uuid
        visited = set()
        max_depth = 20  # Safety limit for downward walk

        for _ in range(max_depth):
            # Already marked this one, get children
            children = children_map.get(current_uuid, [])

            # No children = end of conversation
            if not children:
                break

            # Take first child (main chain, ignore sidechains for now)
            child_uuid = children[0]

            # Safety: detect cycles
            if child_uuid in visited:
                break
            visited.add(child_uuid)

            child = msg_map.get(child_uuid)
            if not child:
                break

            # Check if this is a real user message (stop condition)
            if child.get('message_type') == 'user':
                content = child.get('content', '').strip()
                # If it's NOT a system message, this is real follow-up work
                if (not content.startswith('[Tool') and
                    not content.startswith('<command-message>') and
                    not content.startswith('Base directory')):
                    break  # Stop before real user message

            # Mark and continue
            meta_uuids.add(child_uuid)
            child['is_meta_conversation'] = True
            current_uuid = child_uuid

    def _mark_meta_conversations(self, messages: List[Dict]) -> set:
        """
        Find and mark conversation-search usage, ancestors, and descendants as meta.

        Walks up from search messages to find originating user requests, and walks
        down to mark search results. This filters entire meta-conversation transactions
        where users ask Claude to search for past conversations and receive results.

        Args:
            messages: List of message dicts with uuid, parent_uuid, message_type, content

        Returns:
            Set of message UUIDs that are meta-conversations
        """
        meta_uuids = set()
        msg_map = {m['uuid']: m for m in messages}

        # Build children map for downward traversal
        children_map = {}
        for message in messages:
            parent_uuid = message.get('parent_uuid')
            if parent_uuid:
                if parent_uuid not in children_map:
                    children_map[parent_uuid] = []
                children_map[parent_uuid].append(message['uuid'])

        # Find all messages that use conversation-search
        for message in messages:
            if not message_uses_conversation_search(message):
                continue

            # Mark search message
            meta_uuids.add(message['uuid'])
            message['is_meta_conversation'] = True

            # Walk up to originating user message
            self._mark_ancestor_chain_to_user(message['uuid'], msg_map, meta_uuids)

            # Walk down to mark search results
            self._mark_descendant_chain(message['uuid'], children_map, msg_map, meta_uuids)

        return meta_uuids

    def index_conversation(self, file_path: Path, adapter=None):
        """
        Index a single conversation file with batch summarization.
        REVIEW: Decoupling parsing into adapters significantly improves maintainability
        and simplifies the core indexing loop.
        """
        if not self.quiet:
            print(f"Indexing: {file_path}")

        # Use provided adapter or fall back to ClaudeAdapter for backward compat
        if adapter is None:
            adapter = ClaudeAdapter()

        # Parse file via adapter
        conv_meta_obj, parsed_messages = adapter.parse(file_path)

        # Convert ParsedMessage dataclasses to dicts for existing logic
        msg_dicts = [self._to_msg_dict(m) for m in parsed_messages]

        if not msg_dicts:
            if not self.quiet:
                print(f"  No messages found in {file_path}")
            return

        # Skip summarizer conversations
        if is_summarizer_conversation(file_path, msg_dicts):
            if not self.quiet:
                print(f"  ⏭️  Skipping automated summarizer conversation")
            return

        # Mark meta-conversations (search pairs)
        meta_uuids = self._mark_meta_conversations(msg_dicts)
        if meta_uuids and not self.quiet:
            pair_count = len(meta_uuids) // 2  # Approximate number of pairs
            print(f"  🏷️  Marking {len(meta_uuids)} meta-search messages (~{pair_count} pairs)")

        # Extract values from ConversationMeta
        project_path = conv_meta_obj.project_path
        session_id = conv_meta_obj.session_id
        source = conv_meta_obj.source

        if not session_id:
            if not self.quiet:
                print(f"  No session_id found in {file_path}")
            return

        # Calculate depths
        parent_map = {m['uuid']: m['parent_uuid'] for m in msg_dicts}
        depths = self.calculate_depth(msg_dicts, parent_map)

        # Index conversation metadata
        cursor = self.conn.cursor()

        # Check if already indexed
        cursor.execute(
            "SELECT indexed_at FROM conversations WHERE session_id = ?",
            (session_id,)
        )
        existing = cursor.fetchone()

        is_update = False
        if existing:
            if not self.quiet:
                print(f"  Already indexed at {existing['indexed_at']}, checking for new messages...")

            # Get existing message UUIDs
            cursor.execute(
                "SELECT message_uuid FROM messages WHERE session_id = ?",
                (session_id,)
            )
            existing_uuids = {row['message_uuid'] for row in cursor.fetchall()}

            # Find new messages only
            new_msg_dicts = [m for m in msg_dicts if m['uuid'] not in existing_uuids]

            if not new_msg_dicts:
                if not self.quiet:
                    print(f"  No new messages, skipping")
                return

            if not self.quiet:
                print(f"  Found {len(new_msg_dicts)} new messages (total: {len(msg_dicts)})")

            # Save reference to all messages for metadata update
            all_msg_dicts = msg_dicts
            msg_dicts = new_msg_dicts  # Only process new ones
            is_update = True

            # Update conversation metadata (use last message from ALL messages, not just new ones)
            cursor.execute("""
                UPDATE conversations
                SET last_message_at = ?,
                    message_count = ?,
                    leaf_message_uuid = ?,
                    indexed_at = CURRENT_TIMESTAMP
                WHERE session_id = ?
            """, (
                all_msg_dicts[-1]['timestamp'],
                len(existing_uuids) + len(new_msg_dicts),
                conv_meta_obj.leaf_uuid,
                session_id
            ))
        else:
            # New conversation - insert metadata
            cursor.execute("""
                INSERT INTO conversations (
                    session_id, project_path, conversation_file,
                    root_message_uuid, leaf_message_uuid, conversation_summary,
                    first_message_at, last_message_at, message_count, source
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """, (
                conv_meta_obj.session_id,
                conv_meta_obj.project_path,
                str(file_path),
                msg_dicts[0]['uuid'],
                conv_meta_obj.leaf_uuid,
                conv_meta_obj.summary or 'Untitled conversation',
                msg_dicts[0]['timestamp'],
                msg_dicts[-1]['timestamp'],
                len(msg_dicts),
                source,
            ))

        # Classify messages for tool noise filtering
        tool_noise_uuids = []
        for message in msg_dicts:
            if self.summarizer.is_tool_noise(message):
                tool_noise_uuids.append(message['uuid'])

        # Insert all messages in a single transaction
        try:
            for message in msg_dicts:
                cursor.execute("""
                    INSERT INTO messages (
                        message_uuid, session_id, parent_uuid, is_sidechain,
                        depth, timestamp, message_type, project_path,
                        conversation_file, full_content, is_meta_conversation,
                        is_tool_noise, source
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """, (
                    message['uuid'],
                    session_id,
                    message['parent_uuid'],
                    message['is_sidechain'],
                    depths.get(message['uuid'], 0),
                    message['timestamp'],
                    message['message_type'],
                    project_path,
                    str(file_path),
                    message['content'],
                    message.get('is_meta_conversation', False),
                    message['uuid'] in tool_noise_uuids,
                    source,
                ))

            # Commit once at the end
            self.conn.commit()

            if tool_noise_uuids and not self.quiet:
                print(f"  Marked {len(tool_noise_uuids)} messages as tool noise")

            if not self.quiet:
                if is_update:
                    print(f"  ✓ Added {len(msg_dicts)} new messages")
                else:
                    print(f"  ✓ Indexed {len(msg_dicts)} messages")

        except sqlite3.Error as e:
            self.conn.rollback()
            if not self.quiet:
                print(f"  Error during indexing, rolled back: {e}")
            raise

    def index_new(self, days_back: Optional[int] = None):
        """Index new/changed conversations since the last run.

        days_back=None triggers auto-detection:
        - Empty DB → scan all history
        - Has data → scan from the date of last indexed conversation (+1 day buffer)
        Pass days_back explicitly to override.
        """
        if days_back is None:
            last_date = self.get_last_indexed_at()
            if last_date is not None:
                today = datetime.now().date()
                days_back = max(1, (today - last_date).days + 1)

        pairs = self.scan_all(days_back)
        if not self.quiet:
            print(f"Found {len(pairs)} conversation files to index")

        for i, (file_path, adapter) in enumerate(pairs, 1):
            if not self.quiet:
                print(f"\n[{i}/{len(pairs)}]")
            try:
                self.index_conversation(file_path, adapter=adapter)
            except Exception as e:
                if not self.quiet:
                    print(f"  Error indexing {file_path}: {e}")
                    import traceback
                    traceback.print_exc()

        if not self.quiet:
            print(f"\n✓ Indexing complete!")

    def close(self):
        """Close database connection"""
        self.conn.close()


def main():
    import argparse

    parser = argparse.ArgumentParser(description='Index Claude Code conversations')
    parser.add_argument('--days', type=int, default=1,
                       help='Index conversations from last N days (default: 1)')
    parser.add_argument('--all', action='store_true',
                       help='Index all conversations regardless of age')
    parser.add_argument('--no-extract', action='store_true',
                       help='Skip smart extraction (store only raw content)')
    parser.add_argument('--db', default='~/.agent-recall/index.db',
                       help='Path to SQLite database')

    args = parser.parse_args()

    days_back = None if args.all else args.days
    extract = not args.no_extract

    indexer = ConversationIndexer(db_path=args.db)
    try:
        indexer.index_new(days_back=days_back, summarize=extract)
    finally:
        indexer.close()


if __name__ == '__main__':
    main()
