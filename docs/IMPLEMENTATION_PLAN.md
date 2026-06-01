# MemoryCore Implementation Plan

## Phase 0: Documentation Alignment

Status: complete for the current scaffold.

- Reviewed README and architecture docs.
- Kept SQLite, graph, daemon, MCP, plugin, skill, and dashboard order aligned
  with the local-first blueprint.

Verification:

```bash
ls README.md docs/ARCHITECTURE.md docs/PROJECT_STRUCTURE.md docs/GRAPH_ARCHITECTURE.md docs/DAEMON_RUNTIME.md docs/EXTENSIBILITY.md docs/SOLUTION_BLUEPRINT.md
```

## Phase 1: Local MVP

Status: in progress.

Implemented scope:

- Rust workspace with the required core crates.
- `memorycore init`.
- `.memorycore/` directory structure.
- SQLite migration with WAL mode.
- `graph_nodes`, `graph_edges`, and `event_log` schema.
- File/folder scanner for Project, Folder, File, and `contains` graph edges.
- `memorycore daemon start/status/stop/logs` lifecycle with native filesystem
  wakeups for file changes and a background polling lane for the other runtime
  surfaces, plus graph cleanup for deleted file paths.
- `memorycore graph file`, `memorycore graph folder`, `memorycore graph query`, focused Mermaid render, `memorycore graph impact`, and full Mermaid export.

Verification:

```bash
cargo fmt --all
cargo check --workspace
cargo test --workspace
cargo run -p memorycore-cli -- init
cargo run -p memorycore-cli -- status
cargo run -p memorycore-cli -- graph file README.md
cargo run -p memorycore-cli -- graph folder docs
cargo run -p memorycore-cli -- graph export --format mermaid
cargo run -p memorycore-cli -- daemon start
cargo run -p memorycore-cli -- daemon status
cargo run -p memorycore-cli -- daemon logs
cargo run -p memorycore-cli -- daemon stop
```

Current verification status:

- `cargo fmt --all --check` passes.
- `cargo check --workspace` passes.
- `cargo test --workspace` passes.
- MVP CLI smoke tests pass against `/tmp/memorycore-verify`.
- Daemon lifecycle and native file watcher behavior were verified through the
  daemon package tests and process checks.

## Phase 2: MCP Tools

Status: first local stdio implementation complete.

- `memorycore_search`
- `memorycore_snapshot`
- `memorycore_graph_query`
- `memorycore_graph_render`
- `memorycore_find_impact`
- `memorycore_adapters`
- `memorycore_memory_cases`
- `memorycore_sessions`
- `memorycore_embeddings`
- `memorycore_embedding_search`
- `memorycore_analyze`

The current implementation lives behind:

```bash
memorycore mcp serve
```

It handles MCP `initialize`, `tools/list`, and `tools/call` over stdio
Content-Length framing. Tools currently call local SQLite directly; daemon/API
forwarding should replace that once the local API exists.

Verification:

```bash
memorycore mcp serve
memorycore graph export --format mermaid
```

Smoke-tested tool calls:

- `tools/list`
- `memorycore_search`
- `memorycore_snapshot`
- `memorycore_graph_query`
- `memorycore_graph_render`
- `memorycore_find_impact`

## Phase 3: Plugin Host, Skill Registry, And Adapter Registry

Status: first local registry implementation complete.

- Validate plugin manifests.
- Register hooks.
- Add project-local skill registry and cache.
- Keep plugin execution capability-scoped.

Implemented scope:

- SQLite `plugins` and `skills` registry tables.
- SQLite `adapters` registry table.
- Plugin manifest validation with capability and hook allowlists.
- `memorycore plugins install <plugin.json>`.
- `memorycore plugins list`.
- `memorycore skills register <path-or-SKILL.md>`.
- `memorycore skills list`.
- `memorycore adapters register --agent <agent>`.
- `memorycore adapters list`.
- Registry events: `plugin_installed`, `skill_registered`.
- Adapter event: `adapter_registered`.

Verification:

```bash
memorycore plugins install plugins/example/plugin.json
memorycore plugins list
memorycore skills register skills/generate-diagram
memorycore skills list
memorycore adapters register --agent codex --name "Codex CLI"
memorycore adapters list
cargo test -p memorycore-plugin-host
cargo test -p memorycore-adapters
```

## Phase 4: Dashboard Graph

Status: first local API + static interactive dashboard implementation complete.

Implemented scope:

- `memorycore api serve` on `127.0.0.1:7330`.
- `/health`, `/status`, `/graph.json`, and `/graph/<node-id>` endpoints.
- CORS enabled for browser dashboard access.
- Static dashboard in `dashboard/` with no npm dependency.
- Interactive SVG graph canvas with pan, zoom, search, filters, selected-node
  inspector, impact edge list, and Mermaid copy action.
- `memorycore graph export --format json` for dashboard data.
- Dashboard fetches `http://127.0.0.1:7330/graph.json` and falls back to sample
  data when the API is not running.

Verification:

```bash
memorycore api serve
memorycore graph export --format json
cd dashboard
python3 -m http.server 8765 --bind 127.0.0.1
```

Visual QA performed with headless Chrome at `1440x900` against the dashboard
connected to the live API. The temporary screenshot was written to
`/tmp/memorycore-dashboard-live.png`.
