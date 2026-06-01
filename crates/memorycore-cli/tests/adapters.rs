use anyhow::{Context, Result};
use rusqlite::Connection;
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
fn registers_adapter_into_sqlite_graph_and_search() -> Result<()> {
    let temp = tempdir()?;
    run(&["init"], temp.path())?;

    let output = run(
        &[
            "adapters",
            "register",
            "--agent",
            "codex",
            "--name",
            "Codex CLI",
            "--session-dir",
            ".memorycore/sessions/codex",
            "--command",
            "codex",
        ],
        temp.path(),
    )?;
    assert!(output.contains("Registered adapter codex Codex CLI (codex)"));

    let list = run(&["adapters", "list"], temp.path())?;
    assert!(list.contains("codex codex enabled Codex CLI"));

    let search = run(&["search", "Codex CLI", "--kind", "Adapter"], temp.path())?;
    assert!(search.contains("Adapter: Codex CLI [adapter:codex]"));

    let conn = Connection::open(temp.path().join(".memorycore/index.db"))?;
    let adapter_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM adapters WHERE id = 'codex'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(adapter_count, 1);
    let graph_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_nodes WHERE id = 'adapter:codex' AND kind = 'Adapter'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(graph_count, 1);
    Ok(())
}
