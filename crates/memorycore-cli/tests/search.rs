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
fn search_finds_graph_nodes_memory_cases_and_events() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(
        temp.path().join("src/main.rs"),
        "fn main() { println!(\"hello world\"); }\n",
    )?;

    run(&["init"], temp.path())?;
    run(&["graph", "file", "src/main.rs"], temp.path())?;
    run(
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

    let output = run(&["search", "main.rs"], temp.path())?;
    assert!(output.contains("Function: main"));
    assert!(output.contains("MemoryCase: Auth refactor"));
    assert!(output.contains("src/main.rs"));
    assert!(output.contains("memory:auth-refactor"));
    Ok(())
}

#[test]
fn search_finds_snapshots() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(
        temp.path().join("src/main.rs"),
        "fn main() { println!(\"hello world\"); }\n",
    )?;

    run(&["init"], temp.path())?;
    run(
        &["snapshots", "create", "--message", "checkpoint alpha"],
        temp.path(),
    )?;

    let output = run(&["search", "checkpoint"], temp.path())?;
    assert!(output.contains("Snapshot:"));
    assert!(output.contains("checkpoint alpha"));
    assert!(output.contains("["));
    Ok(())
}

#[test]
fn search_finds_file_contents() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(
        temp.path().join("src/main.rs"),
        "fn main() { println!(\"hello world\"); }\n",
    )?;

    run(&["init"], temp.path())?;
    run(&["graph", "file", "src/main.rs"], temp.path())?;

    let output = run(&["search", "hello"], temp.path())?;
    assert!(output.contains("File:"));
    assert!(output.contains("[hello]"));
    Ok(())
}

#[test]
fn search_filters_by_kind() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(
        temp.path().join("src/main.rs"),
        "fn main() { println!(\"filter token\"); }\n",
    )?;

    run(&["init"], temp.path())?;
    run(&["graph", "file", "src/main.rs"], temp.path())?;
    run(
        &["snapshots", "create", "--message", "filter token snapshot"],
        temp.path(),
    )?;

    let mixed_output = run(&["search", "filter token"], temp.path())?;
    let file_pos = mixed_output.find("File:");
    let snapshot_pos = mixed_output.find("Snapshot:");
    assert!(file_pos.is_some());
    assert!(snapshot_pos.is_some());
    assert!(file_pos.unwrap() < snapshot_pos.unwrap());

    let snapshot_output = run(
        &["search", "filter token", "--kind", "Snapshot"],
        temp.path(),
    )?;
    assert!(snapshot_output.contains("Snapshot:"));
    assert!(snapshot_output.contains("filter token snapshot"));
    assert!(!snapshot_output.contains("File:"));

    let file_output = run(&["search", "filter token", "--kind", "File"], temp.path())?;
    assert!(file_output.contains("File:"));
    assert!(file_output.contains("[filter token]"));
    assert!(!file_output.contains("Snapshot:"));
    Ok(())
}
