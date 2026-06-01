# MemoryCore Project Structure

This document reflects the repository as it exists now, not the aspirational
future layout.

## Repository Layout

```text
memorycore/
├── Cargo.toml
├── README.md
├── dashboard/
├── docs/
└── crates/
    ├── memorycore-adapters/
    ├── memorycore-api/
    ├── memorycore-cli/
    ├── memorycore-core/
    ├── memorycore-daemon/
    ├── memorycore-embeddings/
    ├── memorycore-graph/
    └── memorycore-plugin-host/
```

## Crate Roles

- `memorycore-core`
  - project layout
  - SQLite migration and connection helpers
  - event logging
  - session/snapshot/storage primitives
  - shared snapshot creation/listing helpers

- `memorycore-daemon`
  - process lifecycle
  - native file wakeups plus background polling
  - file hash snapshots and change detection
  - event processing

- `memorycore-graph`
  - graph node and edge model
  - focused graph query helper
  - impact analysis helper
  - file/folder scanners
  - Rust symbol parser facade
  - Mermaid and JSON renderers

- `memorycore-cli`
  - clap command surface
  - project-root handling
  - graph, daemon, MCP, API, plugin, skill, and adapter commands

- `memorycore-api`
  - local HTTP API used by the dashboard and browser clients

- `memorycore-plugin-host`
  - plugin registry
  - skill registry
  - manifest validation

- `memorycore-embeddings`
  - local mmap-read binary vector store for message embeddings
  - multi-layer HNSW-like neighbor graph for local ANN search
  - SQLite metadata sync for embedding records

- `memorycore-adapters`
  - agent adapter registry
  - adapter graph nodes and project containment edges
  - local integration metadata for agent session paths and launch commands

## Active Supporting Directories

- `dashboard/` contains the static local graph UI.
- `docs/` contains the architecture and phase docs.

## Current Gaps

The repo does not yet contain the larger future tree described in older design
docs, such as a separate TypeScript MCP implementation, a React dashboard
bundle, or dedicated SDK packages. Those are roadmap items, not current files.
