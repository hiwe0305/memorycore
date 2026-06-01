Integrate with agent via MemoryCore harness — shared memory, graph, and activity tracking.

# Codex Memory Skill

Integrate Codex CLI agent with MemoryCore. This skill lets Codex query
the code graph, search indexed files, record its own activity, and
retrieve memory context.

## Usage

```bash
memorycore skill execute codex-memory --agent codex --inputs '{"action":"search","query":"auth"}'
memorycore skill execute codex-memory --agent codex --inputs '{"action":"analyze","target":"src/auth.rs"}'
memorycore skill execute codex-memory --agent codex --inputs '{"action":"record","target":"src/main.rs","query":"refactored"}'
```

## What it does

- **search** — full-text search across code graph, events, snapshots
- **analyze** — combined graph + search + memory report
- **record** — log agent activity to harness
- **status** — project snapshot and daemon health
