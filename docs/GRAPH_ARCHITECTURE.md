# MemoryCore Graph Architecture

## Goal

MemoryCore treats the graph as a control map for the project. The current
implementation can answer questions about files, folders, Rust symbols, and the
relationships between them. It also serves as the backing model for the local
dashboard and JSON/Mermaid exports.

## Current Node Model

The active schema stores generic graph nodes in SQLite with these fields:

```sql
id TEXT PRIMARY KEY,
kind TEXT NOT NULL,
name TEXT NOT NULL,
path TEXT,
span_start INTEGER,
span_end INTEGER,
hash TEXT,
metadata TEXT NOT NULL DEFAULT '{}',
updated_at INTEGER NOT NULL
```

Current node kinds written by the implemented scanner include:

- `Project`
- `Folder`
- `File`
- `Import`
- `Function`
- `Struct`
- `Enum`
- `Trait`
- `TypeAlias`
- `Module`

Memory cases, sessions, messages, snapshots, plugins, skills, and adapters are also
stored as graph nodes with kinds `MemoryCase`, `Session`, `Message`,
`Snapshot`, `Plugin`, `Skill`, and `Adapter`.

The schema is generic enough to add memory, session, decision, snapshot, and
plugin/skill nodes later without a migration shape change.

## Current Edge Model

The active edge table stores a generic source, target, and kind.

Current edges written by the implemented scanner include:

- `contains`
- `defines`
- `imports`
- `resolves_import`
- `resolves_import_symbol`
- `calls`
- `declares_module`
- `explains`

The wider design still reserves room for `imports`, `depends_on`, `mentions`,
`changed_by`, and richer dependency resolution.

## Current Extraction Path

1. `memorycore graph file <path>`, `memorycore graph folder <path>`, or
   `memorycore graph query <target>` resolves
   the path against the project root.
   `memorycore graph query <target> --depth 2` expands the neighborhood.
2. The scanner upserts `Project`, `Folder`, and `File` nodes.
3. Rust files are passed to the tree-sitter parser facade.
4. Rust definitions are emitted as `Function`, `Struct`, `Enum`, `Trait`,
   `TypeAlias`, or `Module` nodes.
5. `memorycore memory pin <name>` creates a `MemoryCase` node and can link it
   to a resolved target with an `explains` edge. Session import writes a
   `Session` node plus `Message` child nodes. Snapshot creation writes a
   `Snapshot` node plus a `contains` edge from `project:root` so snapshots are
   visible in the main graph. Plugin install/register, skill register, and
   adapter register also write `Plugin`, `Skill`, and `Adapter` nodes plus
   `contains` edges from `project:root` so the runtime registries are
   graph-visible too.
   Memory cases are also exposed through `/memory-cases`, dashboard Memory
   rail filtering, and the inspector memory feed.
   Sessions are exposed through `/sessions`, dashboard Sessions rail filtering,
   and the inspector sessions feed.
6. `defines` edges connect the file to each symbol node, `imports` edges
   connect the file to import nodes, `resolves_import` edges point those import
   nodes at local files when they exist, and `resolves_import_symbol` edges
   point import nodes at matching symbol nodes inside the resolved file. `calls`
   edges link callable symbols to callees. Same-file calls are resolved during
   parse, and folder scans add a project-level pass so cross-file calls can be
   wired once the relevant symbols exist in SQLite. `declares_module` file
   edges are also resolved in the folder pass for Rust `mod foo;` declarations.
7. The renderers export the resulting subgraph as Mermaid or JSON.
8. `memorycore graph query <target> --format mermaid` renders a focused subset directly, and
   `--depth` can expand the neighborhood.

## Symbol Parsing

Rust symbol extraction is intentionally conservative:

- it looks for definitions, not full semantic resolution
- it records source spans on symbol nodes
- it writes a stable file-relative symbol id
- it annotates metadata with language and leaf symbol name

This gives the project a real symbol graph now, while leaving richer semantic
linking and dependency normalization for later phases.

## Diagram Output

MemoryCore currently supports two render paths:

- Mermaid for quick text export
- JSON for the dashboard and local API

The dashboard is expected to consume the JSON graph and render an interactive
node view. That is the current target shape for the UI layer.
