# Agent Recall: ticpu Fork and Replacement Plan

Status: Phase 6 complete
Date: 2026-07-31  
Legacy repository: `laurynas-pliuskys/agent-recall-python-legacy`  
Replacement repository: `laurynas-pliuskys/agent-recall`  
Upstream: `ticpu/claude-conversation-search-mcp`  
Active development branch: `main`

## Intended outcome

Replace the current Python/SQLite implementation with a Rust/Tantivy fork of
ticpu's repository, renamed to `agent-recall`. Preserve the existing repository
as a read-only legacy archive and selectively port only the source-neutral
architecture, safety decisions, and behavioral tests that still add value.

Do not delete the existing repository. Rename and archive it so its Git history,
issues, and prior decisions remain available.

## Decision summary

- Use ticpu's implementation as the new search and indexing engine.
- Preserve agent-recall's product-level multi-source architecture rather than
  its Python implementation.
- Add Codex as the first second source and proof that the new adapter boundary
  is real.
- Do not revive legacy Gemini support unless a current, stable transcript source
  becomes available.
- Treat each proposed legacy concept as an independent decision and issue.

## Repository working convention

- `main` is the GitHub default and active development branch.
- Changes are committed and pushed directly to `main` until this convention is changed.
- `master` remains an untouched upstream-aligned reference branch.
- The living migration plan and unchanged-upstream baseline are tracked under
  `docs/plans/` in this repository.
- Commit `b5dbcdee8307d11d3edbea439dcf6ffc387b6719` is the fork's first
  post-upstream change and introduced both planning documents.
- The previous WSL home-directory copies were moved to the WSL trash after the
  pushed repository copies were verified through GitHub.
- The legacy repository's `LEGACY.md` links directly to these tracked documents.

## Migration plan

### Phase 0: Licensing basis — completed

- The fork will conservatively treat ticpu's source as GPLv3 unless the upstream
  copyright holder later resolves the conflicting GPLv3 `LICENSE` and MIT README
  declaration in favor of another license.
- GPLv3 is accepted for the intended personal, internal-work, and free public
  uses of agent-recall.
- The fork will retain upstream copyright and attribution notices.
- The conservative GPLv3 assumption will be stated explicitly in the fork's
  README and package metadata.
- Rust is accepted as the long-term implementation language.

### Phase 1: Legacy preservation — completed

- The final Python preservation commit is
  `46d96579a36cb7790868dca1f253debc184757b1`.
- The annotated `python-final` tag points to that commit and is published on
  GitHub.
- The repository is now
  `laurynas-pliuskys/agent-recall-python-legacy`; it remains connected to its
  original `akatz-ai/cc-conversation-search` fork network.
- This checkout's `origin` explicitly points to
  `https://github.com/laurynas-pliuskys/agent-recall-python-legacy.git`.
- `README.md` identifies the implementation as legacy and points to the future
  Rust/Tantivy successor.
- `LEGACY.md` records the migration rationale, conservative GPLv3 assumption,
  successor location, concepts under review, and issue inventory.
- Issues #8 and #9 remain open and accessible at their explicit legacy URLs.
- The local `.claude/settings.local.json` was preserved and `.claude/` is now
  ignored so it cannot be committed accidentally.
- The legacy repository remains unarchived until the replacement passes its
  first end-to-end Claude and Codex tests.
- Canonical legacy links use `agent-recall-python-legacy` because reusing the
  original `agent-recall` name will supersede GitHub's rename redirect.

### Phase 2: True fork and local cutover — completed

- `laurynas-pliuskys/agent-recall` is a true GitHub fork of
  `ticpu/claude-conversation-search-mcp`.
- The fork preserves upstream's `master` branch at commit
  `730624b9694c6568934f10085643143915e9eba7`; `origin/master` and
  `upstream/master` are identical. The default `main` branch began from the same
  baseline commit.
- The issue tracker is enabled.
- GitHub Actions is enabled for all actions, with the inherited CI and release
  workflows present.
- Dependabot vulnerability alerts and automated security fixes are enabled.
- `main` has minimal branch protection: force-pushes and deletion are blocked,
  while direct pushes remain available and no second-maintainer approval is
  required. The preserved `master` baseline remains protected as well.
- The Python checkout now lives at
  `/home/laurynas/github/agent-recall-python-legacy` with its clean working tree,
  explicit legacy `origin`, and local `.claude/settings.local.json` intact.
- The Rust fork is cloned at `/home/laurynas/github/agent-recall`.
- The Rust checkout has `origin` pointing to
  `laurynas-pliuskys/agent-recall` and `upstream` pointing to
  `ticpu/claude-conversation-search-mcp`.
- Rustup installed the stable WSL toolchain: `rustc 1.97.1`, `cargo 1.97.1`,
  rustfmt, and Clippy.
- `cargo build --release --locked` completed successfully without changing
  tracked source files.
- The unchanged baseline binary reports `claude-conversation-search 1.5.0`;
  rebranding has not started.

### Phase 3: Unchanged-upstream baseline — completed

- All checks ran against upstream commit
  `730624b9694c6568934f10085643143915e9eba7` with a clean tracked checkout.
- `cargo fmt -- --check` passed.
- Clippy passed for all targets and features with warnings denied.
- The locked release build passed.
- All 20 release tests passed with no failures.
- Behavioral checks used only synthetic Claude transcripts and an isolated
  configuration/index under `/tmp/agent-recall-phase3`; no real conversation
  history or existing user cache was accessed.
- Initial indexing processed 2 files and 8 entries; an immediate repeat skipped
  all unchanged files.
- Ordinary search, identifier tokenization, tool-input retrieval, tool-result
  retrieval, opt-in thinking retrieval, and asymmetric centered context all
  passed.
- Passive MCP health reporting detected new files, and incremental reindex made
  both new and modified transcript content searchable.
- MCP initialization, six-tool discovery, search, and reindex calls passed.
- Three upstream baseline defects were confirmed:
  1. `notifications/initialized` receives an erroneous `Unknown method` response;
  2. exactly one modified transcript does not trigger a passive stale warning;
  3. the cached global entry total inflates when a changed file is reindexed.
- Detailed commands, outcomes, and defect mechanics are stored in
  [`agent-recall-ticpu-baseline.md`](agent-recall-ticpu-baseline.md).

### Phase 4: Perform the mechanical rename — completed

Rename all user-visible and internal identities:

- Cargo package renamed to `agent-recall` and version set to `2.0.0`;
- License explicitly declared as `GPL-3.0-only` in `Cargo.toml` and `README.md`;
- Release binary and CLI command updated to `agent-recall`;
- MCP server identifier updated to `agent-recall`;
- Configuration directory updated to `~/.config/agent-recall`;
- Cache directory updated to `~/.cache/agent-recall`;
- Shell completions, `CLAUDE.md`, `README.md`, and CI/release workflows updated to `agent-recall`.

PR #4 (`refactor: rename binary and package to agent-recall v2.0.0`) submitted from branch `rename-to-agent-recall`.

### Phase 5: Introduce a real multi-source core — completed

The shared engine now uses a source boundary resembling:

```text
ConversationSource
|- discover()
|- parse()
|- source_name()
`- resume_hint()
```

The normalized record contains:

```text
source
session_id
message_id
parent_message_id
timestamp
role
content
project_path
conversation_file
sequence
tool/thinking metadata where applicable
```

Source-qualified identities are used throughout:

```text
(source, session_id)
(source, session_id, message_id)
```

`source` is propagated through discovery, parsing, per-artifact cache metadata,
Tantivy documents, search/context retrieval, MCP responses, filtering,
navigation, and health reporting. Exact `(source, session_id)`,
`(source, session_id, message_id)`, and `(source, artifact path)` keys prevent
cross-source replacement or context mixing. Parser versions invalidate only
the artifacts owned by the changed adapter, while schema changes trigger a full
index rebuild.

### Phase 6: Add Codex as the architectural proof — completed

The Codex adapter discovers active and archived rollout JSONL under each
detected Codex home, including the Windows-host home when running in WSL. It
uses the rollout metadata `id` as the native resumable thread ID and records the
exact source artifact for later full-fidelity reads.

The parser indexes user-visible user/assistant messages plus canonical textual
tool calls and outputs. It excludes developer/system instructions, injected
runtime/app/plugin context, encrypted reasoning, and mirrored event records.
Non-text image/audio payloads are not indexed. Malformed non-final JSON records
fail that artifact visibly; a partially written final line is tolerated for an
active rollout.

Union search, source filters, overlapping-ID isolation, artifact-scoped
replacement, source-correct context, parser invalidation, and source-specific
resume hints have regression coverage. A release build was also validated with
an isolated cache against the current local Codex session corpus on 2026-07-31;
the active session and its textual tool calls/results were searchable.

Retrieval is fragment-first: search returns bounded previews, technical records
do not consume the default conversational context window, a matched technical
record is always shown, centered expansion defaults to 500 characters per
message, and exact/full retrieval remains explicit.

### Phase 7: Selectively port approved legacy concepts

Review every candidate in the section below. Create a separate issue for every
accepted item. Port behavior and test fixtures rather than merging the unrelated
Python and Rust histories.

### Phase 8: User migration and compatibility

1. State clearly that the old SQLite index will not be reused.
2. Rebuild the new Tantivy index from original transcript files.
3. Document MCP configuration changes and the new binary path.
4. Decide whether to import old configuration values or simply explain their
   replacements.
5. Detect the old cache/config location and print a non-destructive migration
   notice rather than silently deleting it.
6. Test installation and use from both Claude Code and Codex.

### Phase 9: Release and cutover

1. Publish a prerelease.
2. Run end-to-end tests against real Claude and Codex sessions.
3. Verify fresh installation and upgrade documentation.
4. Recreate or transfer selected legacy issues.
5. Point the legacy README to the replacement.
6. Publish the stable release.
7. Archive `agent-recall-python-legacy`.

## Legacy concepts worth reviewing

Each item below is a candidate, not an instruction to port it automatically.

### S1. Multi-source adapter boundary

Recommendation: Strongly save the concept; rewrite it in Rust.

Why it matters: Source-specific discovery and parsing should be isolated from
indexing and search. ticpu currently understands Claude in multiple layers;
without this boundary, every new client requires changes across the whole
application.

User example and mechanics: A search for `TLS handshake timeout` can return the
best result regardless of whether it occurred in Claude or Codex. Each parser
produces the same normalized record, which enters one shared index.

### S2. Union search with source-qualified identities

Recommendation: Strongly save.

Why it matters: This is the real product differentiator over ticpu. Storing the
source on every session/message and including it in identity keys prevents data
from different clients from colliding.

User example and mechanics: Search everything by default, or ask specifically
for the Codex discussion about OAuth. `(source, session_id)` ensures the result
opens the correct transcript even if two clients produce similar identifiers.

### S3. Codex transcript-format research

Recommendation: Strongly save the research; there is no adapter code to port.

Why it matters: Codex is a current second source whose JSONL history fits the
passive indexing model. The existing notes reduce rediscovery work, but must be
validated against current real transcripts.

User example and mechanics: Work completed in Codex becomes searchable from a
later Claude session without manually recording a memory or summary.

### S4. Structured, client-neutral MCP responses

Recommendation: Save.

Why it matters: The current project returns fields such as source, session,
project, timestamp, role, snippet, and message ID. ticpu primarily formats output
for Claude; a stable structured contract is easier for any MCP client or future
interface to consume.

User example and mechanics: Codex can inspect the `source` field directly instead
of parsing presentation text or emojis. Formatting becomes a client concern,
while retrieval remains machine-readable.

### S5. Fragment-first retrieval defaults

Recommendation: Save the contract and conservative defaults.

Why it matters: ticpu already has strong context navigation, so its engine should
remain. The useful agent-recall principle is returning the match plus a small
context window before offering an entire session.

User example and mechanics: Answering a question about one earlier decision uses
hundreds of tokens rather than loading a 50,000-token transcript. The agent asks
for more context only if the first fragment is insufficient.

### S6. Meta-conversation pollution filtering

Recommendation: Save.

Why it matters: Searches, summarizations, and retrieved tool output can be
re-indexed and then outrank the original conversation. Because ticpu indexes more
tool content, explicit feedback-loop prevention becomes more important.

User example and mechanics: Searching for `OAuth migration` returns the original
engineering discussion rather than five later sessions where agents merely
searched for that discussion.

### S7. Redaction before indexing and opt-in thinking

Recommendation: Strongly save the design; it is not finished reusable code in
the current repository.

Why it matters: Tool inputs/results can contain API keys, environment variables,
customer data, and internal URLs. A shared redaction stage should run before
durable indexing, and thinking should remain disabled unless explicitly enabled.

User example and mechanics: An error remains searchable without making a nearby
credential searchable. Every adapter sends extracted content through the same
redaction policy before Tantivy receives it.

### S8. Per-source failure isolation

Recommendation: Save as a required behavior and implement it explicitly.

Why it matters: One changing transcript format must not make the entire memory
system unavailable. The current repository describes this goal, but the new
implementation must enforce it around discovery and parsing.

User example and mechanics: A malformed Codex rollout produces a warning while
Claude history continues indexing and searching normally.

### S9. Source-specific resume/open hints

Recommendation: Save.

Why it matters: Not every client supports `claude --resume`. Navigation belongs
behind the source interface so each result gets a valid action.

User example and mechanics: A Claude result produces a Claude resume command;
a Codex result produces the appropriate Codex action or transcript reference.

### S10. Multi-source fixtures and behavioral tests

Recommendation: Strongly save the behaviors and fixtures.

Why it matters: Parser, source-filter, date-filter, union-search, malformed-input,
and MCP-shape tests protect the new differentiators when upstream changes are
merged. Rewrite tests in Rust rather than mechanically translating every test.

User example and mechanics: CI catches an upstream merge that accidentally makes
Codex results invisible or drops `source` from MCP responses before release.

### S11. SDK-backed MCP implementation

Recommendation: Evaluate after initial parity.

Why it matters: The Python project uses FastMCP while ticpu implements more MCP
protocol behavior itself. A maintained Rust SDK can reduce protocol maintenance,
but adopting one during the first migration may create unnecessary instability.

User example and mechanics: New Claude or Codex MCP protocol behavior is handled
by the SDK's negotiation and serialization instead of requiring handwritten
server changes.

### S12. Retrieval skill and usage guidance

Recommendation: Optionally save the concise behavioral guidance, not the legacy
installation machinery.

Why it matters: Storage is only useful if the agent recognizes when and how to
retrieve. A short skill can teach fragment-first search and progressive context
expansion without tying the engine to one client.

User example and mechanics: Asking `What did we decide last week?` is more likely
to trigger a search automatically, without explicitly naming an MCP tool.

## Things to discard by default

- The Python indexing and search engine.
- SQLite/FTS5 and its migrations.
- The legacy Gemini adapter.
- The duplicated legacy Claude parser.
- The current AI summarization pipeline, at least initially.
- Python CLI and packaging machinery.
- Existing features already implemented better by ticpu, including cache health,
  incremental indexing, tokenization, tool-content extraction, configurable
  truncation, locking, and context navigation.
- Claims of multi-source support not backed by a maintained adapter and an
  end-to-end integration test.

## Final acceptance checklist

- Applicable upstream license is unambiguous.
- Legacy repository and issues remain accessible under the explicit archive URL.
- New repository is a real fork connected to ticpu upstream.
- Upstream baseline tests pass before and after branding changes.
- Claude parity is preserved.
- Codex and Claude union search works end to end.
- Source filters and source-qualified IDs work.
- A broken source cannot block healthy sources.
- Tool content is redacted before indexing.
- Thinking indexing is opt-in.
- MCP responses are structured and source-neutral.
- Old SQLite data is left intact and the migration path is documented.
- Fresh install and upgrade flows work from Claude Code and Codex.

## References

- Comparison issue: <https://github.com/laurynas-pliuskys/agent-recall/issues/8>
- Proposed upstream: <https://github.com/ticpu/claude-conversation-search-mcp>
- GitHub repository renaming:
  <https://docs.github.com/en/repositories/creating-and-managing-repositories/renaming-a-repository>
- GitHub fork behavior:
  <https://docs.github.com/en/pull-requests/reference/forks>
