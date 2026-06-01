use anyhow::{Context, Result};
use memorycore_core::connect_project_db;
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
fn imports_session_jsonl_zst_and_indexes_messages() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("inputs"))?;
    let input = temp.path().join("inputs/session.jsonl");
    fs::write(
        &input,
        r#"{"role":"user","content":"How does auth work?","timestamp":1000,"metadata":{"agent":"codex"}}
{"role":"assistant","content":"It uses SQLite.","timestamp":1001,"metadata":{"agent":"codex"}}
"#,
    )?;

    run(&["init"], temp.path())?;
    let output = run(
        &[
            "sessions",
            "import",
            "--agent",
            "codex",
            "--id",
            "session-auth",
            "inputs/session.jsonl",
        ],
        temp.path(),
    )?;
    assert!(output.contains("Imported session session-auth"));

    let archive = temp
        .path()
        .join(".memorycore")
        .join("sessions")
        .join("codex")
        .join("session-auth.jsonl.zst");
    let bytes = fs::read(&archive)?;
    assert_eq!(&bytes[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
    let decoded = Command::new("zstd")
        .arg("-dc")
        .arg(&archive)
        .output()
        .context("decompress session archive")?;
    assert!(decoded.status.success());
    let decoded = String::from_utf8_lossy(&decoded.stdout);
    assert!(decoded.contains("How does auth work?"));
    assert!(decoded.contains("It uses SQLite."));

    let list = run(&["sessions", "list"], temp.path())?;
    assert!(list.contains("session-auth codex"));

    let show = run(&["sessions", "show", "session-auth"], temp.path())?;
    assert!(show.contains("user How does auth work?"));
    assert!(show.contains("assistant It uses SQLite."));

    let search = run(&["search", "auth"], temp.path())?;
    assert!(search.contains("Message: session-auth"));
    assert!(search.contains("How does auth work?"));

    let conn = connect_project_db(temp.path())?;
    let session_nodes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_nodes WHERE kind = 'Session' AND id = ?1",
        ["session:codex:session-auth"],
        |row| row.get(0),
    )?;
    assert_eq!(session_nodes, 1);

    let message_nodes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_nodes WHERE kind = 'Message' AND path = ?1",
        [archive.to_string_lossy().to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(message_nodes, 2);

    Ok(())
}
