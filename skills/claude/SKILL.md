Integrate with agent via MemoryCore harness — shared memory, graph, and activity tracking.

# Claude Memory Skill

Connect Claude Desktop to MemoryCore's code graph. Claude can browse
the project structure, analyze specific files, and trace impact chains
before making changes.

## Usage

```bash
memorycore skill execute claude-memory --agent claude --inputs '{"action":"analyze","target":"src/main.ts"}'
memorycore skill execute claude-memory --agent claude --inputs '{"action":"graph","target":"src/"}'
memorycore skill execute claude-memory --agent claude --inputs '{"action":"impact","target":"src/auth.rs","depth":2}'
```

## What it does

- **analyze** — combined report: graph context + search hits + memory
- **search** — full-text FTS5 search
- **graph** — Mermaid subgraph export
- **impact** — dependency chain trace
