use memorycore_core::{append_event, connect_project_db, init_project};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn init_creates_memorycore_layout_and_schema() {
    let temp = tempdir().expect("create temp project");
    let layout = init_project(temp.path()).expect("init project");

    assert!(layout.memorycore.is_dir());
    assert!(layout.index_db.is_file());
    assert!(layout.sessions.join("codex").is_dir());
    assert!(layout.sessions.join("claude").is_dir());
    assert!(layout.sessions.join("cursor").is_dir());
    assert!(layout.sessions.join("antigravity").is_dir());
    assert!(layout.snapshot_objects.is_dir());
    assert!(layout.snapshot_refs.is_dir());
    assert!(layout.embeddings.is_dir());
    assert!(layout.plugins.is_dir());
    assert!(layout.skills.is_dir());
    assert!(layout.events.is_dir());
    assert!(layout.logs.is_dir());
    assert!(layout.memorycore.join("config.toml").is_file());

    let conn = connect_project_db(temp.path()).expect("open project database");
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("read journal mode");
    assert_eq!(journal_mode.to_lowercase(), "wal");

    for table in [
        "graph_nodes",
        "graph_edges",
        "event_log",
        "messages",
        "plugins",
        "skills",
    ] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table', 'virtual table') AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .expect("query schema table");
        assert_eq!(count, 1, "expected schema table {table}");
    }

    let event_id = append_event(
        &conn,
        "memorycore-test",
        "verification_event",
        &json!({ "ok": true }),
    )
    .expect("append event");
    assert_eq!(event_id, 1);

    let event_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE event_type = 'verification_event'",
            [],
            |row| row.get(0),
        )
        .expect("count verification events");
    assert_eq!(event_count, 1);
}
