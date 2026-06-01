# MemoryCore

MemoryCore is a local-first coding memory system for one project at a time.
It keeps a SQLite index, a code graph, a memory graph, a daemon, an MCP
interface, plugin/skill/adapter registries, and a local dashboard in one
project-scoped workspace.

## What It Does Now

- Initializes a project-local `.memorycore/` directory.
- Stores the primary index in SQLite with WAL mode and FTS5.
- Stores session logs as JSONL.zst under `.memorycore/sessions/`.
- Stores embeddings as a local mmap-read binary vector file with a small
  multi-layer HNSW-like neighbor graph under `.memorycore/embeddings/`.
- Scans files and folders into `graph_nodes` and `graph_edges`.
- Extracts Rust symbol definitions and call edges with tree-sitter, including
  project-level resolution during folder scans.
- Records Rust import nodes and `imports` edges for project dependencies.
- Resolves local Rust imports into `resolves_import` edges when the target file
  exists in the project index.
- Resolves local Rust imports into `resolves_import_symbol` edges when the
  imported symbol exists in the resolved file.
- Resolves Rust `mod foo;` declarations into `declares_module` file edges after
  folder scans populate the project index.
- Pins named memory cases into the graph.
- Exports Mermaid or JSON graph views.
- Runs a local daemon that uses native filesystem wakeups plus poll-based
  refreshes to keep the graph current.
- Exposes a local HTTP API and an MCP stdio server.
- Registers plugins, skills, and agent adapters in SQLite.

## Current Command Surface

```bash
memorycore init
memorycore status
memorycore daemon start
memorycore daemon status
memorycore daemon stop
memorycore daemon logs
memorycore graph file <path>
memorycore graph folder <path>
memorycore graph query <target>
memorycore graph query <target> --depth 2
memorycore graph query <target> --format mermaid
memorycore graph query <target> --format mermaid --depth 2
memorycore graph impact <target>
memorycore graph impact <target> --depth 2
memorycore graph export --format mermaid
memorycore graph export --format json
memorycore search <query> [--kind <kinds>]
memorycore analyze <target> [--depth <n>] [--format text|json|mermaid]
memorycore snapshots create --message <text>
memorycore snapshots list
memorycore snapshots show <hash>
memorycore events
memorycore events --node <graph-node-id>
memorycore events --follow
memorycore sessions import --agent <agent> --id <session-id> <path>
memorycore sessions list
memorycore sessions show <session-id>
memorycore memory pin <name>
memorycore memory list
memorycore mcp serve
memorycore api serve
memorycore plugins install <manifest.json>
memorycore plugins list
memorycore skills register <skill-dir-or-SKILL.md>
memorycore skills list
memorycore adapters register --agent <agent> [--name <name>] [--session-dir <path>] [--command <cmd>]
memorycore adapters list
memorycore embeddings build
memorycore embeddings list
memorycore embeddings search <query>
```

## Project Layout

The active store lives under `.memorycore/`:

```text
.memorycore/
├── index.db
├── sessions/
├── snapshots/
│   ├── objects/
│   └── refs/
├── embeddings/
│   └── chunks.bin
├── plugins/
├── skills/
├── events/
├── logs/
└── config.toml
```

## Graph Views

```bash
memorycore graph file src/auth.rs
memorycore graph folder crates/memorycore-core
memorycore graph query main.rs
memorycore graph query main.rs --format mermaid
memorycore graph export --format mermaid
memorycore graph export --format json
memorycore graph impact main.rs --depth 2
memorycore events --limit 5 --status pending
memorycore snapshots create --message "manual checkpoint"
memorycore snapshots list --limit 5
memorycore snapshots show <hash>
memorycore events --node file:src/main.rs
memorycore events --follow
```

The graph currently includes project, folder, file, and Rust symbol nodes, plus
memory-case nodes, session nodes, message nodes, snapshot nodes, plugin nodes,
skill nodes, adapter nodes, plus `contains`, `defines`, and `explains` edges. Focused graph
queries can expand to a configurable neighborhood depth.
The JSON export powers the local dashboard and the API graph payload. Impact
queries can also expand traversal depth when you need a deeper dependency slice.
The dashboard supports node selection, focus expansion, periodic refresh from
the local API, a configurable focus depth, search results backed by the SQLite
index, a recent events feed backed by the SQLite event log, a recent snapshots
feed backed by the SQLite snapshot store, a registered adapters feed backed by
the adapter API, and a memory-cases feed backed by the memory API. Event rows
that point at a graph node can be used to jump back into the graph, snapshot
hits can open the snapshot detail panel, adapter, memory, and session rows can
focus their graph nodes, selected sessions show message details from `/session/<id>`,
selected nodes show analysis reports from `/analyze`,
the toolbar can analyze arbitrary targets without first selecting a node,
analysis reports can copy Mermaid diagrams,
embeddings metadata and vector search are shown from
the local vector store, and the selected node's related events are shown in the inspector. The
selected node and focus depth are reflected in the dashboard
URL and persisted locally so the view can be reopened in the same state. The
active tab is persisted too, so Inspector/Navigator state survives reloads.
The Navigator tab shows the current filtered node slice as a browseable list,
and when a node is selected it also shows the connected neighborhood at the
current depth, with quick inspect/focus actions on each node.
The left rail switches the navigator surface between Graph, Files, Plugins,
Skills, Adapters, Memory, and Sessions so those local node sets can be browsed directly.

## Runtime

```bash
memorycore daemon start
memorycore daemon status
memorycore daemon logs
memorycore daemon stop
```

The daemon keeps the project fresh with native filesystem wakeups for local
file changes plus a slower background lane for git HEAD, session archives, and
registered plugin/skill source files. File wakes are coalesced by path, carry
changed paths into the graph layer directly, and deleted files remove their
file, symbol, and import nodes from SQLite. The background lane refreshes the
other surfaces and acts as a fallback for file scanning. It records events in
SQLite, rescans or reindexes affected inputs, and creates new project
snapshots when watched surfaces change. You can also create and list snapshots
directly with `memorycore snapshots create/list`, and `memorycore status`
reports the current snapshot count. Session archive changes replace the old
session graph/messages and rebuild embeddings; session deletions remove the
session graph state and message rows from SQLite. Renamed files are treated as
delete-plus-create so the graph state moves to the new path instead of leaving
the old path behind.

## MCP

The local MCP server currently exposes:

- `memorycore_search`
- `memorycore_snapshot`
- `memorycore_graph_query` with optional target/depth
- `memorycore_graph_render` for full or target-focused Mermaid, with optional depth
- `memorycore_find_impact` with optional traversal depth
- `memorycore_adapters` for registered local agent adapters
- `memorycore_memory_cases` for pinned memory cases
- `memorycore_sessions` for imported sessions and messages
- `memorycore_embeddings` for local embedding metadata
- `memorycore_embedding_search` for local vector search over message embeddings
- `memorycore_analyze` for target reports that combine graph, search, and memory-case context

## Local API

The HTTP API is bound to `127.0.0.1:7330` and serves:

- `GET /health`
- `GET /status`
- `GET /search?q=<query>&limit=<n>&kind=<kinds>`
  - response includes `surfaces` counts for Graph/Files/Plugins/Skills/Adapters
  - response includes `daemon.alive`, `daemon.status`, and `daemon.error`
- `GET /graph.json`
- `GET /graph/<node-id>`
- `GET /graph/<node-id>?depth=<n>`
- `GET /graph/<node-id>?format=mermaid`
- `GET /graph/<node-id>?format=mermaid&depth=<n>`
- `GET /impact?target=<target>&depth=<n>&limit=<n>`
- `GET /analyze?target=<target>&depth=<n>&limit=<n>`
- `GET /analyze?target=<target>&format=mermaid&depth=<n>`
- `GET /events?limit=<n>&status=<status>&node_id=<graph-node-id>`
- `GET /snapshots?limit=<n>`
- `GET /snapshot/<hash>`
- `GET /adapters?agent=<filter>&limit=<n>`
- `GET /memory-cases?target=<filter>&limit=<n>`
- `GET /sessions?agent=<filter>&limit=<n>`
- `GET /session/<session-id>?limit=<n>`
- `GET /embeddings?chunk_type=<filter>&limit=<n>`
- `GET /embeddings/search?q=<query>&limit=<n>`

## Roadmap

The repository already has the core workspace, storage, daemon, graph, MCP,
plugin, and skill pieces in place. The remaining work is to deepen the watcher,
expand graph resolution beyond Rust symbol definitions, and make the dashboard a
richer live control surface.
