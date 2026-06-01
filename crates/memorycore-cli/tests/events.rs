use anyhow::{Context, Result};
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
fn events_lists_recent_event_log_rows() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;

    run(&["init"], temp.path())?;
    run(&["graph", "file", "src/main.rs"], temp.path())?;

    let output = run(
        &["events", "--limit", "5", "--status", "pending"],
        temp.path(),
    )?;
    assert!(output.contains("\"event_type\":\"graph_file_scanned\""));
    assert!(output.contains("\"source\":\"memorycore-graph\""));
    assert!(output.contains("\"status\":\"pending\""));
    assert!(output.contains("\"node_id\":\"file:src/main.rs\""));
    Ok(())
}

#[test]
fn events_filters_by_node_id() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;

    run(&["init"], temp.path())?;
    run(&["graph", "file", "src/main.rs"], temp.path())?;

    let output = run(
        &["events", "--limit", "5", "--node", "file:src/main.rs"],
        temp.path(),
    )?;
    assert!(output.contains("\"node_id\":\"file:src/main.rs\""));
    assert!(!output.contains("\"event_type\":\"snapshot_created\""));
    Ok(())
}
