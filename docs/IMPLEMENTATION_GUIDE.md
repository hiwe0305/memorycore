# MemoryCore Implementation Guide

This guide reflects the **actual** architecture and code. See also
[ARCHITECTURE.md](ARCHITECTURE.md) for system-level design and
[IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) for phase status.

## Overview

MemoryCore is a local-first coding memory system. It runs on a single project,
stores everything in SQLite under `.memorycore/`, and provides:

- A **code graph** extracted via tree-sitter (Rust, JavaScript/JSX)
- A **daemon** with filesystem watcher for live updates
- **MCP tools** for AI agent integration
- A **local HTTP API** and browser-based **dashboard**
- **Plugin, skill, and adapter registries** for extensibility
- **Snapshots, sessions, embeddings, search, events**

All Rust, all synchronous, all local.

## Crate Layout

```
memorycore/
├── crates/
│   ├── memorycore-core/     # SQLite migration, project init, search, snapshots, analysis
│   ├── memorycore-daemon/   # Process lifecycle, file watcher, background refresh loop
│   ├── memorycore-graph/    # Tree-sitter parsing, graph model, scanner, query, render
│   ├── memorycore-cli/      # Clap CLI, MCP server, integration tests
│   ├── memorycore-api/      # HTTP API server (tiny_http)
│   ├── memorycore-plugin-host/  # Plugin/skill manifest validation + registry
│   ├── memorycore-embeddings/   # Message embedding builder
│   └── memorycore-adapters/     # Agent adapter registry
├── dashboard/               # Pure HTML/JS/CSS interactive graph dashboard
├── docs/                    # Architecture, design, plan documents
└── Cargo.toml               # Workspace root
```

All crates use stable Rust, edition 2021, no async runtime.

## Key Design Decisions

### SQLite as Primary Store

- Single `index.db` with WAL mode
- FTS5 for full-text search (`messages_fts`, `file_contents_fts`)
- Tables: `graph_nodes`, `graph_edges`, `event_log`, `file_contents`, `snapshots`,
  `sessions`, `messages`, `embeddings`, `plugins`, `skills`, `adapters`, `memory_cases`
- Foreign keys enforced via `PRAGMA foreign_keys=ON`
- No connection pool — single connection per CLI/daemon process

### Tree-sitter Code Graph

- `tree-sitter` crate v0.20 for AST parsing
- Language-specific parsers: `tree-sitter-rust` v0.20, `tree-sitter-javascript` v0.20
- Each parser module exports:
  - `parse_*_symbols` — extracts function, struct, class, method, variable nodes
  - `extract_*_imports` — extracts use/import statements
  - `extract_*_call_sites` — extracts call expressions for cross-file resolution
- Cross-file resolution: import paths → file nodes, call names → function symbols
- Two-phase scan: all nodes first, then edges (avoids FK constraint violations)

### Synchronous Daemon

- Process fork (Unix) or spawn for background lifecycle
- `notify` crate for native filesystem wakeups
- 10-second background poll for git HEAD, session archives, plugins, skills
- Event-driven: file change → rescanned → graph updated → snapshot created
- File deletions handled as graph node cleanup
- Renames handled as delete + create

### MCP Integration

- Stdio-based MCP server via `memorycore mcp serve`
- Content-Length framing per MCP spec
- 11 tools: search, snapshot, graph_query, graph_render, find_impact,
  adapters, memory_cases, sessions, embeddings, embedding_search, analyze

### HTTP API

- `tiny_http` server on `127.0.0.1:7330`
- JSON endpoints for graph, search, events, snapshots, sessions, analysis
- CORS enabled for dashboard access
- Daemon status endpoint with structured state

## Adding a New Language

1. Add the tree-sitter grammar crate to workspace `Cargo.toml`
2. Add it to `crates/memorycore-graph/Cargo.toml`
3. Create `crates/memorycore-graph/src/parser/<lang>.rs` with:
   - Symbol extraction (`ParsedSymbol` / `Parsed*Import` types)
   - Import extraction
   - Call site extraction
4. Export from `crates/memorycore-graph/src/parser/mod.rs`
5. Add extension check in `crates/memorycore-graph/src/scanner.rs`
6. Add tests

### Tree-sitter Version Compatibility

The codebase uses tree-sitter **0.20**. Grammar crates must use the matching
0.20.x API where `extern "C"` functions return `Language` directly:

- `tree-sitter-rust` 0.20.x — `tree_sitter_rust::language()` returns `Language`
- `tree-sitter-javascript` 0.20.x — `tree_sitter_javascript::language()` returns `Language`

Newer grammar crates (0.25+) use `LanguageFn` from `tree_sitter_language` and
are incompatible without extra conversion. Use the 0.20.x line.

## Phase Status

| Phase | Status | What |
|---|---|---|
| 0: Documentation Alignment | ✓ Complete | All docs reviewed and updated |
| 1: Local MVP | ✓ Complete | Init, SQLite, scanner, daemon, graph commands, Mermaid export |
| 2: MCP Tools | ✓ Complete | 11 MCP tools for AI agent integration |
| 3: Plugin/Skill/Adapter Host | ✓ Complete | Validation + SQLite registry for plugins, skills, adapters |
| 4: Dashboard | ✓ Complete | HTML/CSS/JS interactive graph, search, events, inspect panels |

## Verification

```bash
memorycore init
memorycore status
memorycore daemon start
memorycore daemon status
memorycore daemon stop
memorycore graph file <path>
memorycore graph folder <path>
memorycore graph export --format mermaid
memorycore graph query <target>
memorycore graph impact <target>
memorycore search <query>
memorycore snapshots create --message "checkpoint"
memorycore events --limit 5
memorycore analyze <target>
memorycore mcp serve
memorycore api serve
memorycore plugins list
memorycore skills list
memorycore adapters list
```
