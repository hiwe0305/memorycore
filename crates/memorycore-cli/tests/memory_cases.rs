use anyhow::{Context, Result};
use rusqlite::Connection;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn run(args: &[&str], cwd: &std::path::Path) -> Result<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_memorycore"))
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("run memorycore {args:?}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "command failed: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[test]
fn pins_memory_case_into_graph_and_registry() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;

    run(&["init"], temp.path())?;
    run(&["graph", "file", "src/main.rs"], temp.path())?;
    let out = run(
        &[
            "memory",
            "pin",
            "Auth refactor",
            "--summary",
            "Track auth refactor decisions",
            "--target",
            "main.rs",
        ],
        temp.path(),
    )?;
    assert!(out.contains("Pinned memory case memory:auth-refactor"));

    let conn = Connection::open(temp.path().join(".memorycore/index.db"))?;
    let case_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_cases WHERE name = 'Auth refactor'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(case_count, 1);

    let node_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_nodes WHERE kind = 'MemoryCase'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(node_count, 1);

    let explains_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_edges WHERE kind = 'explains'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(explains_count, 1);

    let list = run(&["memory", "list"], temp.path())?;
    assert!(list.contains("Auth refactor"));
    assert!(list.contains("Track auth refactor decisions"));

    Ok(())
}
