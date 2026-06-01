# MemoryCore Daemon Runtime

## Goal

The daemon is the always-on local runtime. It keeps the project graph fresh,
records events, exposes the local API, and serves as the execution point for
MCP requests.

```bash
memorycore daemon start
memorycore daemon status
memorycore daemon stop
memorycore daemon logs
```

## Current Components

```text
memorycore-daemon
├── supervisor      process lifecycle and health reporting
├── watcher         native filesystem wakeups plus a background polling lane
├── event log       durable SQLite event stream
├── graph updater   rescans changed files into graph_nodes/graph_edges
├── snapshotter     hashes project files for change detection
├── MCP bridge      serves the stdio tool interface
└── local API       serves health, status, graph payloads, impact text, and events
```

## Current Event Model

The primary runtime event table is `event_log` in `index.db`.

Important event types currently emitted:

- `file_changed`
- `file_deleted`
- `git_commit_detected`
- `graph_file_scanned`
- `graph_folder_scanned`
- `snapshot_created`
- `plugin_installed`
- `plugin_changed`
- `plugin_deleted`
- `skill_registered`
- `skill_changed`
- `skill_deleted`

The implementation is intentionally simple: emit an event, process it
idempotently, and keep enough state in SQLite that the daemon can recover after
crashes.

## Current Watch Loop

The current watcher uses native filesystem events to wake the daemon for local
file changes and keeps the other runtime surfaces on a slower background
polling lane.

That gives the project a working 24/7 loop now:

1. Wake on a native filesystem event or a background poll timeout.
2. Coalesce file wake events by path before processing them.
3. If the wake came from the filesystem lane, process the changed paths
   directly through the graph layer and update the file-hash cache.
4. Emit `file_changed` and `file_deleted` events.
5. Changed-file rescans prune stale symbols, imports, module declarations, and
   outgoing call/import edges for that file before writing the fresh graph.
6. Deleted paths remove their file, symbol, and import graph nodes from SQLite.
7. Renamed paths are treated as delete-plus-create so graph state moves to the
   new path instead of lingering at the old one.
8. On the background lane, detect `.git/HEAD` changes and emit
   `git_commit_detected`.
9. On the background lane, re-sync registered plugin manifests and skill files
   from SQLite paths.
10. Session archive changes replace the previous session graph/messages and
   rebuild embeddings; session deletions remove the session graph state and
   message rows from SQLite.
11. Write a new project snapshot when any watched surface changes.
12. Append log output and update status state.

## Local API

The daemon and API layer currently expose:

- `GET /health`
- `GET /status`
- `GET /graph.json`
- `GET /graph/<node-id>`
- `GET /impact?target=<target>&depth=<n>&limit=<n>`
- `GET /events?limit=<n>&status=<status>&node_id=<graph-node-id>`
- `GET /snapshots?limit=<n>`
- `GET /snapshot/<hash>`

The CLI also exposes `memorycore events` and `memorycore events --follow` for
terminal inspection of the same SQLite event stream, including `--node` for a
node-scoped event tail. Snapshot history is available through
`memorycore snapshots create`, `memorycore snapshots list`, and
`memorycore snapshots show <hash>`, and `memorycore status` includes the
current snapshot count.

The API is bound to `127.0.0.1:7330`.

## Reliability Notes

- SQLite runs in WAL mode.
- Event processing is idempotent.
- The daemon records enough state to restart cleanly.
- The project remains local-first and filesystem-backed.

## Roadmap

The next daemon steps are richer git/session ingestion and broader dependency
refresh for files that import or call into changed Rust modules.
