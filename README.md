# Agent Recall

**CLI + MCP service for searching AI coding assistant conversation history.**

A single binary that works two ways:
- **CLI**: Search your conversations from the terminal (`agent-recall search "rust async"`)
- **MCP Server**: Lets Claude Code and Codex search conversation history during sessions

Other tools are *viewers* - you browse manually. This tool indexes everything with Tantivy/BM25 and gives both you AND AI agents direct search access.

![Screenshot](docs/screenshot.png)

## Perfect For Heavy AI Agent Users

If you work across **dozens of projects**, you know the pain:
- "I solved this exact problem last month... but which project?"
- "What was that regex pattern I used for parsing logs?"
- "How did I configure that Docker setup?"

This tool indexes **conversations across all projects** and lets your agent search them instantly. No more digging through folders or re-explaining context.

> **Warning**: Claude Code auto-deletes old conversations! Check `~/.claude/settings.json` for `cleanupPeriodDays` - this deletes conversations older than N days (0 = immediate deletion!). Set it to `999999999` to keep your history.

## Why This Tool?

| Feature | Agent Recall | Other Tools |
|---------|--------------|-------------|
| AI agents search history | ✓ MCP integration | ✗ Manual browsing only |
| Cross-project search | ✓ All projects indexed | ✗ Per-project only |
| Full-text search | ✓ Tantivy/BM25 | Some have regex |
| Jump to specific message | ✓ `center_on` + `-B/-A` context | ✗ |
| Smart content filtering | ✓ Skips tool_result noise | ✗ Index everything |
| Passive staleness detection | ✓ Warns when index outdated | ✗ |

## Overview

`agent-recall` indexes transcript histories with smart filtering (skips file dumps, keeps reasoning) and exposes search via MCP so agents can find relevant past conversations during your session.

## Features

### 🔍 **Powerful Search**
- **Full-text search** across all conversations with BM25 ranking
- **Smart filtering** by project name
- **Highlighted snippets** showing matched content in context
- **Relevance scoring** for best matches first

### ⚡ **High Performance**  
- **Lightning fast**: Sub-millisecond search queries
- **Efficient indexing**: Processes thousands of conversations in seconds
- **Memory efficient**: Uses memory-mapped indexes via Tantivy

### 🔧 **Unified Interface**
- **Single binary** with subcommands for both CLI and MCP server functionality
- **CLI mode**: Simple command-line interface for terminal usage (`agent-recall search ...`)
- **MCP server mode**: Integration with Claude Code and Codex via Model Context Protocol (`agent-recall mcp`)
- Configurable result limits and project-based filtering

### 🎯 **Smart Features**
- **Auto-discovery** of transcript directories
- **Smart content filtering**: Indexes text/thinking blocks, skips tool_result file dumps (noise reduction)
- **UUID-based deduplication**: Handles session resume and rollbacks gracefully
- **Passive health monitoring**: Warns when index is stale, offers reindex tool
- **Robust parsing** handles malformed JSONL gracefully

## Quick Start

### Installation

```bash
cargo build --release
cp target/release/agent-recall ~/.local/bin/
agent-recall mcp register
```

Verify: `claude mcp list` should show `agent-recall`.

### Basic Usage

```bash
# Index your conversations (run this first time)
agent-recall index

# Search for anything
agent-recall search "kubernetes"
agent-recall search "error handling" 
agent-recall search "rust async"

# Search with project filter
agent-recall search "rust" --project "vault-rs"

# Limit number of results
agent-recall search "function" --limit 20
```

## CLI Reference

### `agent-recall index`
Build or update the search index.

```bash
agent-recall index              # Build/update index
agent-recall index --rebuild    # Force full rebuild (recreates index)
```

**What it does:**
- Scans transcript directories for `*.jsonl` files
- Parses conversation entries with timestamps, content, and metadata  
- Builds full-text search index using Tantivy
- Index stored at `~/.cache/agent-recall/`

### `agent-recall search <query>`
Search through your indexed conversations.

```bash
agent-recall search "rust async functions"
agent-recall search "error" --project "my-project" --limit 5
```

**Options:**
- `--project <name>` - Filter by project directory name (e.g., "vault-rs")
- `--limit <n>` - Maximum results to show (default: 10)

**Query features:**
- **Simple text**: `agent-recall search "docker compose"`
- **Multiple terms**: `agent-recall search "rust error handling"`  
- **Phrase search**: `agent-recall search '"exact phrase"'` (wrap in quotes)
- **Boolean AND**: `agent-recall search "rust AND async"` (both terms must appear)

## MCP Integration

This tool provides an MCP (Model Context Protocol) server for seamless integration with Claude Code and Codex.

### Setup

1. **Build the binary**:
   ```bash
   cargo build --release
   ```

2. **Configure Claude Code / MCP CLI**:
   ```bash
   claude mcp add agent-recall ~/.local/bin/agent-recall mcp
   ```

3. **Use within sessions**:
   - "Search my previous conversations about Rust async"
   - "Find where we discussed error handling"

### MCP Tools Available
- **search_conversations**: Full-text search with `-C`/`-B`/`-A` context (grep-style).
- **get_session_messages**: Paginated session content.
- **get_messages**: Fetch full content of specific messages.
- **summarize_session**: Returns Task instructions for session summarization.
- **reindex**: Update index when results seem incomplete.
- **respawn_server**: Reload MCP server after rebuilding.

## Configuration

### Config File

`~/.config/agent-recall/config.yaml`:

```yaml
limits:
  per_file_chars: 150000        # Max chars indexed per JSONL file
  tool_result_max_chars: 2000   # Max chars kept from tool_result content
  tool_input_max_chars: 200     # Max chars kept from tool_use input

search:
  exclude_patterns: []          # Regex patterns to exclude from results

index:
  auto_index_on_startup: true
  writer_heap_mb: 50
```

Changing `tool_result_max_chars` or `tool_input_max_chars` requires a reindex (`agent-recall index --rebuild`).

### Cache Location

- **Linux**: `~/.cache/agent-recall/`
- **macOS**: `~/Library/Caches/agent-recall/`
- **Windows**: `%LOCALAPPDATA%\agent-recall\`

## Troubleshooting

### Getting Help

```bash
agent-recall --help          # General help
agent-recall search --help   # Search command help
agent-recall index --help    # Index command help
```

## License

GPL-3.0-only - see [LICENSE](LICENSE) for details.
