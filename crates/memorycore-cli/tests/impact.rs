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
fn impact_depth_expands_beyond_direct_edges() -> Result<()> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("src"))?;
    fs::write(
        temp.path().join("src/lib.rs"),
        "mod helper;\npub fn helper() {}\n",
    )?;
    fs::write(
        temp.path().join("src/main.rs"),
        "use crate::helper::helper;\nfn main() { helper(); }\n",
    )?;
    fs::write(temp.path().join("src/helper.rs"), "pub fn helper() {}\n")?;

    run(&["init"], temp.path())?;
    run(&["graph", "folder", "src"], temp.path())?;

    let shallow = run(&["graph", "impact", "src/main.rs"], temp.path())?;
    assert!(
        shallow.contains("- file:src/main.rs -imports-> import:src/main.rs#crate::helper::helper")
    );
    assert!(!shallow.contains("symbol:src/helper.rs#helper"));

    let deep = run(
        &["graph", "impact", "src/main.rs", "--depth", "2"],
        temp.path(),
    )?;
    assert!(deep.contains("symbol:src/helper.rs#helper"));
    assert!(deep.contains("- import:src/main.rs#crate::helper::helper -resolves_import_symbol-> symbol:src/helper.rs#helper"));
    Ok(())
}
