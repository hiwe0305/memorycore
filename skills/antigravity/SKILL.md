Integrate with agent via MemoryCore harness — shared memory, graph, and activity tracking.

# Antigravity Memory Skill

Antigravity multi-agent system shares memory through MemoryCore. Each
agent records its activity and can query the collective graph.

## Usage

```bash
memorycore skill execute antigravity-memory --agent antigravity --inputs '{"action":"record","message":"completed training run 42"}'
memorycore skill execute antigravity-memory --agent antigravity --inputs '{"action":"search","query":"training results"}'
memorycore skill execute antigravity-memory --agent antigravity --inputs '{"action":"broadcast","target":"agent-alpha","message":"model ready"}'
```

## What it does

- **record** — log agent activity with metadata
- **search** — find related agent activity and code
- **analyze** — understand code context
- **broadcast** — record inter-agent communication in harness
