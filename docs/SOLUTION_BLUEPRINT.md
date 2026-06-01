# MemoryCore Solution Blueprint

## Objective

Build MemoryCore as a local-first control system for coding projects:

```text
memory + graph + daemon + MCP + plugins + skills + adapters + dashboard
```

The system should inspect a file, folder, algorithm, system, or memory case and
produce a graph, diagram, explanation, and impact view without leaving the
local machine.

## Phase 1: Core Storage

Status: implemented.

What exists now:

- `memorycore init`
- `.memorycore/` layout
- SQLite `index.db` in WAL mode
- `graph_nodes`, `graph_edges`, `event_log`
- sessions, snapshots, embeddings, plugins, skills, adapters tables
- file-backed session/snapshot/embedding directories

Verification used:

```bash
cargo test --workspace
memorycore init
memorycore status
```

## Phase 2: File And Graph MVP

Status: implemented.

What exists now:

- `memorycore graph file <path>`
- `memorycore graph folder <path>`
- `memorycore graph query <target>`
- `memorycore graph query <target> --depth 2`
- `memorycore graph query <target> --format mermaid`
- `memorycore graph query <target> --format mermaid --depth 2`
- `memorycore_graph_query` with optional target/depth
- `memorycore graph impact <target>`
- `memorycore graph impact <target> --depth 2`
- Mermaid export
- JSON export
- Rust symbol extraction with tree-sitter
- dashboard JSON payloads through the local API, a focus-depth control that
  persists in the URL/local state, and a Navigator tab that browses the current
  filtered node slice; the active tab persists across reloads; the left rail
  switches between Graph, Files, Plugins, Skills, Adapters, Memory, and Sessions surfaces

Verification used:

```bash
cargo test --workspace
memorycore graph file src/main.rs
memorycore graph folder crates/memorycore-core
memorycore graph query main.rs
memorycore graph query main.rs --depth 2
memorycore graph query main.rs --format mermaid
memorycore graph query main.rs --format mermaid --depth 2
memorycore graph export --format mermaid
memorycore graph export --format json
```

## Phase 3: Daemon Runtime

Status: implemented with native file wakeups plus a background polling lane.

What exists now:

- `memorycore daemon start`
- `memorycore daemon status`
- `memorycore daemon stop`
- `memorycore daemon logs`
- native filesystem wakeups for project file changes, including rename/move
  handling
- a background polling lane for git HEAD, session archives, plugins, and skills
- direct graph cleanup for deleted file, symbol, and import nodes
- rename handling that moves graph state to the new path
- changed-file rescans prune stale symbols, imports, module declarations, and
  outgoing call/import edges before writing the fresh graph
- `file_changed` and `file_deleted` events
- rescanning changed files into the graph

The next daemon step is broader dependency refresh for files that import or
call into changed Rust modules, plus richer session/git ingestion.

## Phase 4: MCP And API

Status: implemented in a minimal local form.

What exists now:

- `memorycore mcp serve`
- `memorycore api serve`
- `memorycore_search`
- `memorycore_snapshot`
- `memorycore_graph_query` with optional target/depth
- `memorycore_graph_render` for full or target-focused Mermaid with optional depth
- `memorycore_find_impact` with optional traversal depth
- `memorycore_adapters` for registered local agent adapters
- `memorycore_memory_cases` for pinned memory cases
- `memorycore_sessions` for imported sessions and messages
- `memorycore_embeddings` for local embedding metadata
- `memorycore_embedding_search` for local vector search over message embeddings
- `memorycore_analyze` for target reports and Mermaid diagrams that combine graph, search, and memory-case context

The API currently serves health, status, graph JSON, and node-specific graph
payloads with optional depth, plus impact text with optional depth, recent
event payloads, snapshot list/detail payloads, adapter list payloads, and
memory-case list payloads, session list/detail payloads, embedding metadata and
embedding search payloads, analysis payloads, and search hits from the SQLite store, including
snapshots. Search can be scoped by kind. `GET /status` includes snapshot counts alongside
graph and registry counts.
The `daemon` field in `/status` is structured so clients can tell live versus
stale daemon state apart without reading `.memorycore/daemon.json` directly.
The search API also returns per-surface counts for the dashboard rail.

## Phase 5: Plugins, Skills, And Adapters

Status: implemented in registry form.

What exists now:

- plugin manifest validation
- plugin install and list commands
- skill register and list commands
- adapter register and list commands
- plugin, skill, adapter, and snapshot counts in `memorycore status`
- graph-visible `Plugin`, `Skill`, and `Adapter` nodes

## Phase 6: Dashboard

Status: implemented as a static local UI.

What exists now:

- local graph canvas
- pan and zoom
- search and filtering
- node inspector
- registered adapter feed backed by `/adapters`
- memory cases feed backed by `/memory-cases`
- sessions feed backed by `/sessions`
- selected session message details backed by `/session/<id>`
- selected node analysis backed by `/analyze`
- direct target analysis input backed by `/analyze`
- analysis Mermaid copy backed by `/analyze?format=mermaid`
- embeddings feed backed by `/embeddings`
- embedding vector search backed by `/embeddings/search`
- Mermaid copy/export support
- live graph fetch from the local API

## Remaining Work

The current codebase is no longer just scaffolding. The remaining work is to
deepen the current runtime rather than replace it:

1. richer graph resolution beyond Rust symbol definitions
2. deeper memory/session graph extraction
3. semantic indexing and embeddings pipeline
4. more complete dashboard expansion and impact exploration

That sequence keeps the system local-first and prevents the implementation from
splitting into disconnected subsystems too early.
