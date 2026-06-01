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
fn snapshots_create_list_and_status() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;

    run(&["init"], temp.path())?;

    let created = run(
        &["snapshots", "create", "--message", "CLI snapshot"],
        temp.path(),
    )?;
    assert!(created.contains("Snapshot "));
    assert!(created.contains("event_log id="));

    let listed = run(&["snapshots", "list", "--limit", "5"], temp.path())?;
    assert!(listed.contains("\"message\":\"CLI snapshot\""));
    assert!(listed.contains("\"file_count\":1"));

    let status = run(&["status"], temp.path())?;
    assert!(status.contains("Snapshots: 1"));
    Ok(())
}

#[test]
fn snapshots_show_returns_snapshot_details() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;

    run(&["init"], temp.path())?;
    run(
        &["snapshots", "create", "--message", "CLI snapshot"],
        temp.path(),
    )?;

    let listed = run(&["snapshots", "list", "--limit", "1"], temp.path())?;
    let hash = listed
        .split("\"hash\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .context("extract snapshot hash")?;

    let shown = run(&["snapshots", "show", hash], temp.path())?;
    assert!(shown.contains("\"message\":\"CLI snapshot\""));
    assert!(shown.contains("\"files\""));
    assert!(shown.contains("src/main.rs"));
    Ok(())
}
