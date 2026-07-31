# Agent Recall: ticpu Fork and Replacement Plan

Status: Phase 8 in progress — local Claude and Codex readiness verified; fresh client-session end-to-end validation pending
Date: 2026-07-31  
Codex integration merge: `37fd824`
Next release target: `2.1.0` (unreleased)
Legacy repository: `laurynas-pliuskys/agent-recall-python-legacy`  
Replacement repository: `laurynas-pliuskys/agent-recall`  
Upstream: `ticpu/claude-conversation-search-mcp`  
Active development branch: `main`

## Intended outcome

Replace the previous Python implementation with a Rust/Tantivy fork of ticpu's
repository, renamed to `agent-recall`. Preserve the existing repository as a
read-only legacy archive and selectively port only the source-neutral
architecture, safety decisions, and behavioral tests that still add value.

Do not delete the existing repository. Rename and archive it so its Git history,
issues, and prior decisions remain available.

## Decision summary

- Use ticpu's implementation as the new search and indexing engine.
- Preserve agent-recall's product-level multi-source architecture rather than
  its Python implementation.
- Add Codex as the first second source and proof that the new adapter boundary
  is real.
- Treat Claude Code and Codex as supported transcript sources in one shared
  Tantivy index, with source-qualified identities throughout.
- Keep Codex tool calls and results source-backed in the original rollout:
  global discovery indexes conversational turns only, then selected-session
  reference search retrieves technical evidence on demand.
- Keep Claude's inherited capped tool-content indexing unchanged until its
  migration is explicitly decided.
- Treat the local MCP registrations and direct wire validation as verified
  readiness, not proof of a fresh-client installation flow. Fresh Claude Code
  and Codex session validation remains a Phase 8 requirement.
- Use `agent-recall install --client all|claude|codex`, defaulting to `all`.
  Codex registration is global; Claude Code registration uses user scope by
  default or project scope with `--project`.
- Target `2.1.0` for the first release containing the completed Codex
  integration and compatibility work. Phase 4's `2.0.0` rename remains the
  historical completed version bump.
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
|- is_primary_index_record()
`- is_reference_record()
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

The parser recognizes user-visible user/assistant messages plus canonical
textual tool calls and outputs. The Codex adapter puts only conversational turns
in the primary Tantivy index; tool records remain in the original rollout and
are searched on demand after a conversation is selected. It excludes
developer/system instructions, injected runtime/app/plugin context, encrypted
reasoning, and mirrored event records. Non-text image/audio payloads are not
exposed as textual references. Malformed non-final JSON records fail that
artifact visibly; a partially written final line is tolerated for an active
rollout.

Union search, source filters, overlapping-ID isolation, artifact-scoped
replacement, source-correct context, parser invalidation, and source-specific
resume hints have regression coverage. A release build was also validated with
an isolated cache against the current local Codex session corpus on 2026-07-31;
the active session and its textual tool calls/results were retrievable.

Retrieval is fragment-first and two-tiered for Codex: global search returns
bounded conversational previews, `search_session_references` scans the selected
rollout without creating a second persistent index, and centered expansion
retrieves an exact technical record. This avoids tool-driven BM25 ranking and
duplicate technical payload storage. The accepted tradeoff is that a term found
only in a Codex tool record cannot be used for global conversation discovery.
Claude's capped tool-input/result indexing remains unchanged and is deferred to
a separate migration decision.

### Phase 7: Selectively port approved legacy concepts — partially complete

The implemented and open candidate decisions are recorded in the legacy-concept
status summary below. Implemented behavior and test fixtures were ported without
merging the unrelated Python and Rust histories. Each remaining accepted concept
requires its own decision and issue before implementation.

### Phase 8: User migration and compatibility — in progress

Completed local readiness verification on 2026-07-31:

- The Codex integration was merged as `37fd824`.
- The current tree passed its checks: 38 unit tests and 2 integration tests.
- Direct MCP JSON-RPC wire validation passed.
- The release binary was installed at `~/.local/bin/agent-recall`.
- MCP registrations are present for both Claude Code and Codex.
- The installer now supports `--client all|claude|codex` (default `all`), with
  Codex global and Claude Code user/project scope handling. Its three focused
  installer tests pass.
- Basic installation documentation now covers installing from the checkout,
  initial indexing, client selection, sequential verification, and manual
  registration fallback for both clients.
- A full Tantivy rebuild processed 71 transcript files and indexed 4,492 primary
  conversation entries.
- A source-filtered Codex search succeeded against the rebuilt local index.

Remaining work, in priority order:

1. Verify a fresh installation and MCP registration from each client, without
   relying on the existing local registrations.
2. Start fresh Claude Code and Codex sessions and prove that each client can
   call the MCP server, search both sources, retrieve source-correct context,
   and reindex after a new transcript is written.
3. Document the current-tool upgrade procedure.

### Phase 9: Release and cutover

1. Complete the remaining Phase 8 fresh-client end-to-end checks and current
   installation and upgrade guidance.
2. Build a fresh `2.1.0` release artifact.
3. Publish that artifact as a `2.1.0` prerelease.
4. Verify fresh installation and upgrade from the prerelease artifact.
5. Recreate or transfer selected legacy issues.
6. Point the legacy README to the replacement.
7. Publish the `2.1.0` stable release.
8. Archive `agent-recall-python-legacy`.

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

### S13. Indexing trigger strategy and session-start hooks

Recommendation: Evaluate and define a clear triggering policy (session-start hooks vs JIT-on-search vs background file watcher).

Why it matters: Passive transcript indexing is only effective if newly written
conversation history becomes searchable when needed. Originally, the ticpu Rust
engine only auto-indexes at MCP server startup or when the agent explicitly calls
the `reindex` MCP tool (or prints a passive "index is stale" warning). Because MCP
server processes stay alive in the background across long client sessions,
transcripts created or updated during a session can remain stale until server
restart or explicit reindexing.

Legacy Python behavior & reasoning: The previous Python implementation supported
an optional `SessionStart` client hook in `.claude/settings.json` (running
`agent-recall index --quiet`) so that every newly initiated client session
automatically ran incremental indexing before queries were made. It also embraced
Just-In-Time (JIT) indexing prior to search execution to guarantee fresh results.

Questions to resolve:
1. Should `agent-recall install` automatically configure client session hooks
   (e.g., Claude Code `SessionStart` hook) to run `agent-recall index` at session launch?
2. Should `agent-recall` perform automatic JIT incremental reindexing transparently
   before executing a `search` tool call when stale transcripts are detected,
   rather than just returning a text notice?
3. Should a background filesystem watcher (e.g. using Rust's `notify` crate) be
   introduced in the MCP server process to auto-index active JSONL transcripts
   in real time as they are updated?
4. What are the performance and write-lock contention tradeoffs (e.g., Tantivy
   index write locks during active search or simultaneous transcript append operations)?

### S14. Evaluation of default retrieval context and truncation budgets

Recommendation: Create a dedicated issue titled *"Evaluate default retrieval context and truncation budgets"* to benchmark context window sizes against message truncation strategies.

Why it matters: The current engine defaults to returning wider message windows (e.g., ±10 messages) paired with per-message character caps (e.g., 500 characters). However, severe character truncation often clips key technical details like SQL queries, CLI commands, code snippets, or error tracebacks mid-line. Returning a narrower context window (e.g., ±2 messages) containing full, un-truncated message content may yield higher evidence quality and precision while maintaining similar or smaller token budgets.

Benchmark parameters to compare:
- Centered context windows: ±2, ±5, ±10 messages.
- Sequential paging windows: 5, 10, 20 messages.
- Truncation caps: 200, 300, 500 characters vs. full un-truncated message rendering.

Target recovery tasks:
- Recovering an architectural decision, CLI command, SQL query, exact tool output, and stack trace / error trace.

Benchmark evaluation metrics:
- Successful evidence recovery rate (zero detail loss).
- Total returned characters and token overhead.
- Required follow-up tool calls for context expansion.
- Response latency.

## Legacy concept status

- Implemented: S1 multi-source boundary; S2 union search and source-qualified
  identities; S3 Codex transcript research and adapter; S5 fragment-first
  retrieval; S6 Codex runtime-context filtering; S8 per-artifact parse-failure
  isolation; S9 source-specific resume hints; and S10 multi-source fixtures and
  regression tests.
- Open decisions: S4 structured, client-neutral MCP response objects; S7 shared
  redaction before durable indexing and true opt-in thinking; S11 SDK-backed
  MCP; S12 an optional retrieval skill; S13 indexing trigger strategy and session-start hooks; and S14 retrieval context and truncation budget evaluation.
- The completed local readiness verification does not close the fresh-client
  end-to-end acceptance work in Phase 8.

## Things to discard by default

- The Python indexing and search engine.
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

- [ ] Applicable upstream license is unambiguous.
- [ ] Legacy repository and issues remain accessible under the explicit archive
  URL.
- [x] New repository is a real fork connected to ticpu upstream.
- [x] Upstream baseline checks passed before branding, and the current tree's
  38 unit tests and 2 integration tests pass after the Codex work.
- [ ] Claude parity is preserved through a fresh client-session test.
- [ ] Codex and Claude union search works end to end from fresh client sessions.
- [x] The installer configures Claude Code and/or Codex with explicit client and
  scope behavior, and basic installation documentation covers both clients.
- [x] Source filters and source-qualified IDs work.
- [x] A malformed source artifact is isolated so other artifacts continue to
  index.
- [ ] Tool content is redacted before indexing.
- [ ] Thinking indexing is opt-in.
- [ ] MCP responses are structured and source-neutral.
- [ ] Fresh install and upgrade flows work from Claude Code and Codex.

## References

- Comparison issue: <https://github.com/laurynas-pliuskys/agent-recall/issues/8>
- Proposed upstream: <https://github.com/ticpu/claude-conversation-search-mcp>
- GitHub repository renaming:
  <https://docs.github.com/en/repositories/creating-and-managing-repositories/renaming-a-repository>
- GitHub fork behavior:
  <https://docs.github.com/en/pull-requests/reference/forks>
