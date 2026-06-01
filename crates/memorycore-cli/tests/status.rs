use anyhow::{Context, Result};
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
fn status_reports_daemon_state_when_running() -> Result<()> {
    let temp = tempdir()?;
    run(&["init"], temp.path())?;
    run(&["daemon", "start"], temp.path())?;
    let output = run(&["status"], temp.path())?;
    assert!(output.contains("Daemon: running pid="));
    assert!(output.contains("last_activity_at="));
    run(&["daemon", "stop"], temp.path())?;
    Ok(())
}
