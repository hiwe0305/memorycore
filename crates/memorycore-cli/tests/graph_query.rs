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
fn graph_query_returns_focused_subset() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;

    run(&["init"], temp.path())?;
    run(&["graph", "file", "src/main.rs"], temp.path())?;

    let output = run(&["graph", "query", "src/main.rs"], temp.path())?;
    assert!(output.contains("\"focus\""));
    assert!(output.contains("file:src/main.rs"));
    assert!(output.contains("\"edges\""));
    Ok(())
}

#[test]
fn graph_query_can_render_mermaid() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;

    run(&["init"], temp.path())?;
    run(&["graph", "file", "src/main.rs"], temp.path())?;

    let output = run(
        &["graph", "query", "src/main.rs", "--format", "mermaid"],
        temp.path(),
    )?;
    assert!(output.starts_with("flowchart TD"));
    assert!(output.contains("File: main.rs"));
    assert!(output.contains("-->|contains|"));
    Ok(())
}

#[test]
fn graph_query_depth_includes_neighboring_symbols() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(temp.path().join("src/lib.rs"), "pub fn helper() {}\n")?;
    fs::write(
        temp.path().join("src/main.rs"),
        "use crate::helper::helper;\nfn main() { helper(); }\n",
    )?;

    run(&["init"], temp.path())?;
    run(&["graph", "folder", "src"], temp.path())?;

    let output = run(
        &["graph", "query", "src/main.rs", "--depth", "2"],
        temp.path(),
    )?;
    assert!(output.contains("import:src/main.rs#crate::helper::helper"));
    assert!(output.contains("symbol:src/lib.rs#helper"));
    assert!(output.contains("\"nodes\""));
    Ok(())
}
