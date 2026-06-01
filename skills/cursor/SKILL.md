Integrate with agent via MemoryCore harness — shared memory, graph, and activity tracking.

# Cursor Memory Skill

While coding in Cursor, use MemoryCore to stay aware of the full
project graph. Search, analyze, and snapshot without leaving the editor.

## Usage

```bash
memorycore skill execute cursor-memory --agent cursor --inputs '{"action":"analyze","target":"src/components/"}'
memorycore skill execute cursor-memory --agent cursor --inputs '{"action":"search","query":"TODO"}'
memorycore skill execute cursor-memory --agent cursor --inputs '{"action":"snapshot"}'
```

## What it does

- **analyze** — understand a file or folder before editing
- **search** — find relevant code quickly
- **snapshot** — checkpoint before risky changes
- **status** — check graph and daemon health
