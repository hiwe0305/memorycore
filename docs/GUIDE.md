# MemoryCore Guide

> A local-first coding memory system — 24/7 daemon, code graph, MCP tools, SQLite storage.

---

## Table of Contents

1. [What is MemoryCore](#1-what-is-memorycore)
2. [Core Concepts](#2-core-concepts)
3. [Installation](#3-installation)
4. [Quick Start](#4-quick-start)
5. [The Daemon](#5-the-daemon)
6. [The Code Graph](#6-the-code-graph)
7. [Search](#7-search)
8. [Analysis](#8-analysis)
9. [Impact Tracing](#9-impact-tracing)
10. [Memory Cases](#10-memory-cases)
11. [Snapshots](#11-snapshots)
12. [Events](#12-events)
13. [Sessions & Embeddings](#13-sessions--embeddings)
14. [MCP Server](#14-mcp-server)
15. [HTTP API](#15-http-api)
16. [Dashboard](#16-dashboard)
17. [Plugins, Skills & Adapters](#17-plugins-skills--adapters)
18. [Architecture](#18-architecture)
19. [Database Schema](#19-database-schema)
20. [Project Status](#20-project-status)

---

## 1. What is MemoryCore

MemoryCore is a persistent, local-first memory layer for coding projects.
It indexes your source code into a queryable code graph, watches for changes
24/7, and exposes everything through a CLI, HTTP API, and MCP server — so both
you and AI agents share the same structural understanding of the codebase.

### What it does

- Parses Rust (`.rs`), JavaScript (`.js`/`.jsx`), and TypeScript (`.ts`/`.tsx`)
  source files into a graph of nodes and edges using tree-sitter
- Runs a background daemon that keeps the index fresh as files change
- Provides full-text search across files, symbols, events, and snapshots
- Traces impact chains — "what calls this function?" — before refactoring
- Pins named memory cases that AI agents discover through MCP tools
- Takes content-addressed snapshots before risky changes
- Exports diagrams as Mermaid flowcharts or JSON
- Serves an MCP protocol server for AI agent integration
- Runs a local HTTP API and browser dashboard

### What it is not

- Not a cloud service — everything stays on your machine
- Not a CI/CD tool — no pipeline integration
- Not a replacement for `grep` — it enriches search with structure

---

## 2. Core Concepts

### Graph

The graph is a tree-sitter AST stored as nodes and edges in SQLite.

- **Nodes** represent files, folders, functions, structs, enums, traits,
  interfaces, classes, methods, variables, imports
- **Edges** capture relationships: `defines`, `calls`, `imports`, `extends`,
  `contains`, `declares_module`
- Cross-file resolution connects imports to their target files and symbols

### Daemon

A background process that:
- Starts with `memorycore daemon start` and runs independently of the terminal
- Watches the filesystem for changes using `notify` (inotify on Linux)
- Polls for git commits, session updates, and plugin/skill changes
- Creates automatic snapshots when changes are detected
- Logs all activity to `.memorycore/logs/daemon.log`

### Memory

Named references to parts of the codebase. You create them with
`memorycore memory pin` and AI agents discover them through MCP tools.
They're how you tell an agent "this module matters."

### Snapshot

A content-addressed (SHA256) point-in-time capture of the indexed state:
all node and edge data plus file content hashes. Take one before
refactoring and compare later.

### Event

Everything that happens flows through an append-only event log: file
scans, graph mutations, snapshots, session imports, plugin changes.
Useful for audit, debugging, and live monitoring (`--follow`).

---

## 3. Installation

```bash
git clone git@github.com:<you>/memorycore.git
cd memorycore
cargo build --release
cp target/release/memorycore ~/.local/bin/
```

Requires Rust 1.96+. The binary is a single ELF file (~8 MB) with no
runtime dependencies.

---

## 4. Quick Start

```bash
cd /path/to/your-project

# Initialize MemoryCore (creates .memorycore/)
memorycore init

# Start the 24/7 daemon
memorycore daemon start

# Index your code
memorycore graph folder src/
memorycore graph folder lib/

# Check what you've got
memorycore status
```

Example output:
```
MemoryCore project: /path/to/your-project
Graph nodes: 23896
Graph edges: 27942
Daemon: running pid=24272
```

---

## 5. The Daemon

### Lifecycle

```bash
memorycore daemon start          # fork into background
memorycore daemon status         # check pid and uptime
memorycore daemon stop           # graceful shutdown
memorycore daemon logs           # view full log
```

### How it works

When started, the daemon:
1. Forks into the background (detaches from terminal)
2. Re-scans the project to build an initial cache
3. Starts a filesystem watcher (inotify on Linux)
4. Polls every 5 seconds for git, session, plugin, and skill changes
5. Records all activity to `.memorycore/logs/daemon.log`

The daemon persists across terminal sessions. You start it once and
forget about it.

### What the watcher ignores

The daemon automatically skips these directories at any level:
`.git`, `.memorycore`, `target`, `node_modules`, `dist`, `build`,
`.next`, `.venv`, `__pycache__`, `.pytest_cache`, `.yarn`,
`.codegraph`, `.codex`, `.agents`

---

## 6. The Code Graph

### Supported languages

| Language | File extensions | Parser |
|---|---|---|
| Rust | `.rs` | tree-sitter-rust 0.20 |
| JavaScript | `.js` `.jsx` | tree-sitter-javascript 0.20 |
| TypeScript | `.ts` `.tsx` | tree-sitter-typescript 0.20 |

### Node types

| Kind | Description |
|---|---|
| `File` | A source file |
| `Folder` | A directory |
| `Function` | Function declaration |
| `Method` | Method inside a class/object |
| `Class` | Class declaration |
| `Interface` | TypeScript interface |
| `Enum` | Enum declaration |
| `TypeAlias` | TypeScript type alias |
| `Struct` | Rust struct |
| `Trait` | Rust trait |
| `Variable` | const / let / var / static |
| `Import` | Import statement |
| `Project` | Root project node |

### Edge types

| Kind | Meaning |
|---|---|
| `defines` | A file defines a symbol |
| `calls` | A function/method calls another |
| `imports` | A file imports a module |
| `contains` | A folder/file contains another |
| `extends` | A class extends another |
| `declares_module` | A file declares a submodule (Rust) |
| `explains` | A memory case targets a symbol |

### How indexing works

The scanner operates in two phases:

1. **Node phase**: Scan all files, create nodes for files, folders, and
   parsed symbols. Write them to SQLite.
2. **Edge phase**: Resolve imports (e.g., `use crate::module;` in Rust)
   and call sites against the existing nodes. Write edges.

This two-phase approach avoids foreign key constraint issues when a
symbol references another file that hasn't been scanned yet.

### Commands

```bash
# Index a single file
memorycore graph file src/main.rs

# Index a folder recursively
memorycore graph folder src/

# Export full graph
memorycore graph export --format mermaid
memorycore graph export --format json > graph.json

# Query a subgraph
memorycore graph query "auth" --depth 2 --format mermaid
```

---

## 7. Search

Full-text search across files, symbols, events, snapshots, and memory
cases — all in one query.

```bash
memorycore search "auth middleware"
memorycore search "login" --kind Function
memorycore search "database" --limit 20
```

The search command queries the FTS5 index, which is kept in sync with
the file contents and graph data. Results include the kind (File,
Function, Import, etc.), path, and a content snippet.

---

## 8. Analysis

Point at any file, folder, function, or symbol. MemoryCore resolves it
and returns a combined report of graph context, search hits, file
content, and related memory cases.

```bash
memorycore analyze src/auth.rs
memorycore analyze src/auth.rs --format json
memorycore analyze src/auth.rs --format mermaid
```

### Example output

```
# MemoryCore Analysis: src/auth.rs

Resolved node: file:src/auth.rs [File] src/auth.rs

Graph context: 10 nodes, 12 edges
- file:src/auth.rs -defines-> symbol:src/auth.rs#login
- symbol:src/auth.rs#login -calls-> symbol:src/db.rs#connect
- folder:src/ -contains-> file:src/auth.rs

Search hits:
- [Function] login src/auth.rs
- [Struct] User src/auth.rs
- [Enum] Role src/auth.rs
```

Three formats: `text` (human-readable), `json` (machine-parseable),
`mermaid` (diagram for embedding in Markdown).

---

## 9. Impact Tracing

Before refactoring a function, find everything that depends on it.

```bash
# Who calls this file?
memorycore graph impact src/auth.rs --depth 2

# Focused subgraph around a symbol
memorycore graph query "login" --depth 2 --format mermaid
```

Impact output shows incoming and outgoing edges for the target:

```
impact for file:src/auth.rs:
- folder:src/ -contains-> file:src/auth.rs
- file:src/lib.rs -declares_module-> file:src/auth.rs
- file:src/auth.rs -defines-> symbol:src/auth.rs#login
- file:src/auth.rs -defines-> symbol:src/auth.rs#User
- symbol:src/auth.rs#login -calls-> symbol:src/db.rs#connect
```

---

## 10. Memory Cases

Memory cases are named references to parts of the codebase. They act as
context markers for AI agents.

```bash
# Pin key modules
memorycore memory pin "auth-flow" --target src/auth.rs
memorycore memory pin "db-schema" --target src/db/
memorycore memory pin "payment-api" --target src/api/payment.rs

# List all pinned cases
memorycore memory list
```

Output:
```
memory:auth-flow-1712345678 auth-flow src/auth.rs
memory:db-schema-1712345678  db-schema  src/db/
```

Agents discover these through the `memorycore_memory_cases` MCP tool.

---

## 11. Snapshots

Content-addressed snapshots capture the indexed state before a risky
change. Each snapshot stores SHA256 hashes of all indexed files plus
the graph state.

```bash
# Take a snapshot
memorycore snapshots create --message "before-refactor-auth"

# List all snapshots
memorycore snapshots list

# Show details
memorycore snapshots show <hash>
```

---

## 12. Events

The event log records every mutation: file scans, graph changes,
snapshots, session imports, plugin activity.

```bash
memorycore events --limit 10      # show recent
memorycore events --follow        # live tail
memorycore daemon logs            # full daemon log
```

Events are stored in SQLite and include timestamp, source, type, data,
status, and optional error information.

---

## 13. Sessions & Embeddings

### Sessions

Import agent conversation logs as JSONL archives:

```bash
memorycore sessions import --agent codex --id my-chat chat.jsonl
memorycore sessions list
memorycore sessions show my-chat
```

Session data is stored compressed (zstd) in `.memorycore/sessions/` and
indexed in SQLite.

### Embeddings

Build vector embeddings from session messages for semantic search:

```bash
memorycore embeddings build
memorycore embeddings list
memorycore embeddings search "how does auth work" --limit 5
```

The embedding store uses a binary mmap file with HNSW approximate
nearest-neighbor search. Metadata is stored in SQLite.

---

## 14. MCP Server

MemoryCore exposes 11 tools through the Model Context Protocol (MCP
2024-11-05) over stdio. This lets any MCP-compatible AI agent (Claude
Code, Cursor, etc.) query your codebase natively.

```bash
memorycore mcp serve
```

### Available tools

| Tool | Description |
|---|---|
| `memorycore_search` | Full-text search across files, nodes, events, snapshots |
| `memorycore_snapshot` | Create / list / show snapshots |
| `memorycore_graph_query` | Focused subgraph with configurable depth |
| `memorycore_graph_render` | Full or target Mermaid diagram |
| `memorycore_find_impact` | Impact chain traversal |
| `memorycore_analyze` | Combined graph + search + memory report |
| `memorycore_adapters` | List registered agent adapters |
| `memorycore_memory_cases` | List pinned memory cases |
| `memorycore_sessions` | List / show imported sessions |
| `memorycore_embeddings` | Embedding metadata |
| `memorycore_embedding_search` | Vector search over session messages |

### Agent integration example

Configure your MCP client to run `memorycore mcp serve` from the
project root. An agent can then call:

```json
{
  "method": "tools/call",
  "params": {
    "name": "memorycore_analyze",
    "arguments": { "target": "src/auth.rs", "depth": 2 }
  }
}
```

---

## 15. HTTP API

Start the REST server:

```bash
memorycore api serve
# Listening on 127.0.0.1:7330
```

### Endpoints

| Method | Path | Description |
|---|---|---|
| GET | `/health` | Health check |
| GET | `/status` | Node/edge/plugin/snapshot counts |
| GET | `/search?q=<query>` | Full-text search |
| GET | `/graph.json` | Full graph as JSON |
| GET | `/graph/<node-id>?depth=N&format=mermaid` | Focused subgraph |
| GET | `/impact?target=<path>&depth=N` | Impact chain |
| GET | `/analyze?target=<path>` | Combined analysis |
| GET | `/events?limit=N` | Event log |
| GET | `/snapshots` | List snapshots |
| GET | `/adapters` | List adapters |
| GET | `/sessions` | List sessions |
| GET | `/embeddings` | Embedding metadata |

CORS is enabled. The dashboard connects to this API.

---

## 16. Dashboard

Open `dashboard/index.html` in a browser while the API server is
running to browse the graph interactively.

The dashboard is a pure HTML/JS/CSS application with no build step.
It connects to the HTTP API at `http://127.0.0.1:7330`.

---

## 17. Plugins, Skills & Adapters

### Plugins

Extensions that add runtime capabilities to MemoryCore. Each plugin
has a manifest (`plugin.json`) with an id, version, entry point,
capabilities, and hooks.

```bash
memorycore plugins install ./path/to/plugin.json
memorycore plugins list
```

### Skills

Reusable workflow instructions for AI agents. Each skill is a
directory with a `SKILL.md` file.

```bash
memorycore skills register ./path/to/skill-dir
memorycore skills list
```

### Adapters

Metadata records that tell MemoryCore about configured AI agent
integrations (session directories, platform commands).

```bash
memorycore adapters register --agent codex --name my-agent
memorycore adapters list
```

---

## 18. Architecture

### Crate layout

```
memorycore/
├── Cargo.toml               workspace root (8 crates)
├── crates/
│   ├── core/                SQLite, init, search, snapshots, analysis
│   ├── daemon/              Process lifecycle, file watcher, background poll
│   ├── graph/               Tree-sitter AST → graph_nodes + graph_edges
│   ├── cli/                 Clap CLI + MCP server (stdio)
│   ├── api/                 tiny_http server on :7330
│   ├── plugin-host/         Plugin/skill/adapter registries
│   ├── embeddings/          mmap binary store + HNSW ANN search
│   └── adapters/            Agent adapter registry
├── dashboard/               Pure HTML/JS/CSS graph viewer
└── docs/                    This guide
```

### Design principles

- **No async runtime** — all code is synchronous, no tokio/async-std
- **Single SQLite connection** — one `Connection` per process
- **No build step for dashboard** — open `index.html` directly
- **Local-first** — everything lives in `.memorycore/`, no cloud
- **Privacy by default** — no telemetry, no external calls

### Depedency graph

```
cli (clap + MCP)
├── core (SQLite, FTS5, analysis)
├── graph (tree-sitter parsing)
│   └── core
├── daemon (notify watcher)
│   ├── core
│   ├── graph
│   ├── embeddings
│   └── plugin-host
├── api (tiny_http)
│   └── core
├── plugin-host (registries)
│   └── core
├── embeddings (mmap + HNSW)
│   └── core
└── adapters (registries)
    └── core
```

### Project layout

```
.memorycore/
├── index.db              SQLite (WAL mode, FTS5)
├── config.toml           api_addr, dashboard_addr
├── daemon.json           pid, started_at, last_activity_at
├── sessions/             JSONL.zst archives per agent
├── snapshots/            Content-addressed (SHA256)
├── embeddings/           Binary mmap + HNSW graph
├── plugins/              Plugin manifests
├── skills/               Skill directories
├── events/               Event records
└── logs/                 Daemon logs
```

---

## 19. Database Schema

The SQLite database at `.memorycore/index.db` uses WAL mode with FTS5.

### graph_nodes

```sql
CREATE TABLE graph_nodes (
    id TEXT PRIMARY KEY,          -- "file:src/main.rs"
    kind TEXT NOT NULL,           -- File, Folder, Function, Struct, ...
    name TEXT NOT NULL,           -- display name
    path TEXT,                    -- relative project path
    span_start INTEGER,           -- source line start
    span_end INTEGER,             -- source line end
    hash TEXT,                    -- content hash
    metadata TEXT,                -- JSON blob
    updated_at INTEGER
);
```

### graph_edges

```sql
CREATE TABLE graph_edges (
    id TEXT PRIMARY KEY,          -- "edge:src:dst:kind"
    source_id TEXT NOT NULL,      -- FK to graph_nodes.id
    target_id TEXT NOT NULL,      -- FK to graph_nodes.id
    kind TEXT NOT NULL,           -- calls, defines, imports, ...
    weight REAL DEFAULT 1.0,
    confidence REAL DEFAULT 1.0,
    metadata TEXT,
    updated_at INTEGER
);
```

### Other tables

- `file_contents` + `file_contents_fts`: indexed file content with FTS5
- `event_log`: append-only event stream
- `snapshots`: snapshot metadata
- `memory_cases`: pinned memory references
- `sessions` + `messages`: imported session data
- `embeddings`: vector embedding metadata
- `plugins`, `skills`, `adapters`: registry data

---

## 20. Project Status

- **105 tests**, all passing (103 unit + 2 integration)
- **0 warnings**, clean `cargo fmt` and `cargo clippy`
- **8 crates**, 26 source files, 12 test files
- **15+ API endpoints**, 11 MCP tools
- **Languages**: Rust, JavaScript/JSX, TypeScript
- **Docs**: This guide (`GUIDE.md`) + `README.md`
- **License**: MIT

### Current capabilities

| Area | Status |
|---|---|
| Code graph (Rust, JS, TS) | Complete |
| Search (FTS5) | Complete |
| Daemon (24/7 watcher) | Complete |
| Analyze + Impact trace | Complete |
| MCP server (11 tools) | Complete |
| HTTP API (15 endpoints) | Complete |
| Dashboard | Complete |
| Snapshots | Complete |
| Memory cases | Complete |
| Events | Complete |
| Sessions (JSONL import) | Complete |
| Embeddings (vector search) | Schema + CLI ready |
| Plugin registry | Schema + CLI ready |
| Skill registry | Schema + CLI ready |
| Adapter registry | Schema + CLI ready |
| TypeScript parser | Complete |
| File ignore patterns | Complete |

---

*Built with Rust, Tree-sitter, SQLite, and zero async overhead.*
*Local-first. Private by default. One project at a time.*
