# MemoryCore Extensibility: MCP, Plugin, Skill, Adapter

## Principle

MemoryCore keeps four extension surfaces separate:

```text
MCP     agents call MemoryCore
Plugin  MemoryCore gains runtime capabilities
Skill   agents learn repeatable workflows
Adapter MemoryCore records local agent integration metadata
```

That separation keeps agent workflows, runtime hooks, agent integration
metadata, and user-facing tools from collapsing into one mixed abstraction.

## MCP

The current MCP server is a stdio process launched with `memorycore mcp serve`.
It speaks to the local project state and exposes a small tool set:

- `memorycore_search`
- `memorycore_snapshot`
- `memorycore_graph_query` with optional target/depth
- `memorycore_graph_render`
- `memorycore_find_impact`
- `memorycore_adapters`
- `memorycore_memory_cases`
- `memorycore_sessions`
- `memorycore_embeddings`
- `memorycore_embedding_search`
- `memorycore_analyze`

The MCP layer is deliberately thin. It is a request/response surface over the
local index, graph, adapter registry, memory-case registry, and session store,
plus embedding metadata, local vector search, and target analysis/Mermaid output,
not a second indexing engine.

## Plugin Registry

Plugins are tracked in SQLite and managed with the CLI.

Current commands:

```bash
memorycore plugins install <manifest.json>
memorycore plugins list
```

The manifest is validated against the current capability allowlist before a
plugin is stored. The registry currently stores:

- plugin id
- name
- version
- entry path
- manifest path
- enabled state
- capabilities
- hooks

## Skill Registry

Skills are also tracked in SQLite and managed with the CLI.

Current commands:

```bash
memorycore skills register <skill-dir-or-SKILL.md>
memorycore skills list
```

Skills are project-local workflow definitions for agents. They do not run in
the background; they describe how to use MemoryCore tools for a repeatable
task.

The daemon keeps registered plugin manifests and skill files in sync by
polling the paths stored in SQLite and marking stale rows disabled when the
source file disappears.

## Adapter Registry

Adapters are tracked in SQLite and managed with the CLI.

Current commands:

```bash
memorycore adapters register --agent <agent> [--name <name>] [--session-dir <path>] [--command <cmd>]
memorycore adapters list
```

Adapters describe how a local agent relates to the project: its stable agent
id, display name, optional session directory, and optional launch command.
Registering an adapter also writes an `Adapter` graph node plus a `contains`
edge from `project:root`, so adapter integrations appear in graph query,
dashboard navigation, and search. Local tools can read the same registry
through `GET /adapters?agent=<filter>&limit=<n>`.

## What This Means In Practice

- MCP is for active tool calls.
- Plugins are for runtime extension and hook dispatch.
- Skills are for repeatable agent guidance.
- Adapters are for local agent integration metadata and navigation.

Keeping those roles separate matches the current codebase and leaves room for
more sophisticated plugin hooks and richer workflow automation later.
