use anyhow::{bail, Context, Result};
use memorycore_core::{append_event, create_snapshot, init_project, now_unix, ProjectLayout};
use memorycore_embeddings::build_message_embeddings_with_conn;
use memorycore_graph::model::{GraphEdge, GraphNode};
use memorycore_graph::scan_file;
use memorycore_graph::scanner::{remove_file_content_tree_index, remove_file_graph_index};
use memorycore_graph::store::{upsert_edge, upsert_node};
use memorycore_plugin_host::{
    disable_plugin_graph, disable_skill_graph, install_plugin, register_skill,
};
use notify::{event::ModifyKind, Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;
use std::time::Instant;
use walkdir::WalkDir;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub pid: u32,
    pub started_at: i64,
    pub project_root: String,
    pub last_activity_at: i64,
}

#[derive(Debug, Clone)]
enum FileChangeEvent {
    Changed(PathBuf),
    Deleted(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

pub fn start(project_root: &Path, current_exe: &Path) -> Result<DaemonStatus> {
    let layout = init_project(project_root)?;
    if let Ok(status) = status(project_root) {
        if process_alive(status.pid, project_root) {
            return Ok(status);
        }
    }

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(layout.logs.join("daemon.log"))
        .context("open daemon log")?;
    let err_file = log_file.try_clone().context("clone daemon log")?;
    let mut command = Command::new(current_exe);
    command
        .arg("--project-root")
        .arg(project_root)
        .arg("daemon")
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file));

    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command.spawn().context("spawn memorycore daemon")?;

    let status = DaemonStatus {
        pid: child.id(),
        started_at: now_unix(),
        project_root: project_root.display().to_string(),
        last_activity_at: now_unix(),
    };
    write_status(&layout, &status)?;
    Ok(status)
}

pub fn run(project_root: &Path) -> Result<()> {
    let layout = init_project(project_root)?;
    let conn = memorycore_core::connect_project_db(project_root)?;
    let status = DaemonStatus {
        pid: std::process::id(),
        started_at: now_unix(),
        project_root: project_root.display().to_string(),
        last_activity_at: now_unix(),
    };
    write_status(&layout, &status)?;
    append_log(&layout, "daemon started")?;
    let mut cache = snapshot_project(project_root)?;
    let mut git_head = snapshot_git_head(project_root)?;
    let mut session_cache = snapshot_sessions(project_root)?;
    let mut plugin_cache = snapshot_registered_plugins(&conn)?;
    let mut skill_cache = snapshot_registered_skills(&conn)?;
    let (_watcher, fs_wake_rx) = match start_project_watcher(project_root) {
        Ok(watcher) => watcher,
        Err(error) => {
            append_log(&layout, &format!("filesystem watcher unavailable: {error}"))?;
            (None, None)
        }
    };
    if let Err(error) = full_rescan(project_root, &conn) {
        append_log(&layout, &format!("initial rescan failed: {error}"))?;
    }
    // Fire onDaemonStart hook for all registered plugins
    if let Err(error) = memorycore_plugin_host::execute_hook(
        project_root,
        "onDaemonStart",
        serde_json::json!({"started_at": now_unix()})
    ) {
        append_log(&layout, &format!("plugin onDaemonStart hook failed: {error}"))?;
    }
    let mut next_background_poll = Instant::now();
    loop {
        let now = Instant::now();
        let wait = next_background_poll.saturating_duration_since(now);
        let mut file_events = Vec::new();

        if let Some(rx) = fs_wake_rx.as_ref() {
            match rx.recv_timeout(wait) {
                Ok(event) => {
                    file_events.push(event);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    append_log(&layout, "filesystem watcher disconnected")?;
                }
            }
            while let Ok(event) = rx.try_recv() {
                file_events.push(event);
            }
        } else {
            thread::sleep(wait);
        }

        file_events = coalesce_file_events(file_events);
        let mut snapshot_needed = false;
        let mut file_lane_processed = false;
        for event in file_events {
            file_lane_processed = true;
            match process_file_event(project_root, &conn, &mut cache, event) {
                Ok(changed) => {
                    snapshot_needed |= changed;
                    if changed {
                        touch_status(&layout, &status)?;
                    }
                }
                Err(error) => append_log(&layout, &format!("file event failed: {error}"))?,
            }
        }
        if Instant::now() >= next_background_poll {
            if !file_lane_processed {
                match poll_project(project_root, &conn, &mut cache) {
                    Ok(changed) => {
                        snapshot_needed |= changed;
                        if changed {
                            touch_status(&layout, &status)?;
                        }
                    }
                    Err(error) => append_log(&layout, &format!("poll failed: {error}"))?,
                }
            }
            match poll_git(project_root, &conn, &mut git_head) {
                Ok(changed) => {
                    snapshot_needed |= changed;
                    if changed {
                        touch_status(&layout, &status)?;
                    }
                }
                Err(error) => append_log(&layout, &format!("git poll failed: {error}"))?,
            }
            match poll_sessions(project_root, &conn, &mut session_cache) {
                Ok(changed) => {
                    snapshot_needed |= changed;
                    if changed {
                        touch_status(&layout, &status)?;
                    }
                }
                Err(error) => append_log(&layout, &format!("session poll failed: {error}"))?,
            }
            match poll_plugins(project_root, &conn, &mut plugin_cache) {
                Ok(changed) => {
                    snapshot_needed |= changed;
                    if changed {
                        touch_status(&layout, &status)?;
                    }
                }
                Err(error) => append_log(&layout, &format!("plugin poll failed: {error}"))?,
            }
            match poll_skills(project_root, &conn, &mut skill_cache) {
                Ok(changed) => {
                    snapshot_needed |= changed;
                    if changed {
                        touch_status(&layout, &status)?;
                    }
                }
                Err(error) => append_log(&layout, &format!("skill poll failed: {error}"))?,
            }
            next_background_poll = Instant::now() + Duration::from_secs(5);
        }
        if snapshot_needed {
            if let Err(error) = create_project_snapshot(project_root, &conn) {
                append_log(&layout, &format!("snapshot failed: {error}"))?;
            } else {
                touch_status(&layout, &status)?;
                let _ = memorycore_plugin_host::execute_hook(
                    project_root,
                    "onSnapshotCreated",
                    serde_json::json!({"snapshot_needed": true})
                );
            }
        }
    }
}

pub fn status(project_root: &Path) -> Result<DaemonStatus> {
    let layout = ProjectLayout::new(project_root);
    let text = fs::read_to_string(status_path(&layout)).context("read daemon status")?;
    let status: DaemonStatus = serde_json::from_str(&text).context("parse daemon status")?;
    if !process_alive(status.pid, project_root) {
        bail!(
            "daemon status file exists, but pid {} is not running",
            status.pid
        );
    }
    Ok(status)
}

pub fn stop(project_root: &Path) -> Result<DaemonStatus> {
    let status = status(project_root)?;
    let kill_status = Command::new("kill")
        .arg(status.pid.to_string())
        .status()
        .context("send daemon stop signal")?;
    if !kill_status.success() {
        bail!("failed to stop daemon pid {}", status.pid);
    }
    let layout = ProjectLayout::new(project_root);
    let _ = fs::remove_file(status_path(&layout));
    Ok(status)
}

pub fn log_path(project_root: &Path) -> PathBuf {
    ProjectLayout::new(project_root).logs.join("daemon.log")
}

fn write_status(layout: &ProjectLayout, status: &DaemonStatus) -> Result<()> {
    fs::write(status_path(layout), serde_json::to_string_pretty(status)?)
        .context("write daemon status")
}

fn touch_status(layout: &ProjectLayout, status: &DaemonStatus) -> Result<()> {
    let mut updated = status.clone();
    updated.last_activity_at = now_unix();
    write_status(layout, &updated)
}

fn status_path(layout: &ProjectLayout) -> PathBuf {
    layout.memorycore.join("daemon.json")
}

fn process_alive(pid: u32, project_root: &Path) -> bool {
    let cmdline_path = Path::new("/proc").join(pid.to_string()).join("cmdline");
    let Ok(bytes) = fs::read(cmdline_path) else {
        return false;
    };
    let parts: Vec<String> = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect();
    let joined = parts.join(" ");
    joined.contains("memorycore")
        && joined.contains("daemon")
        && joined.contains("run")
        && joined.contains(&project_root.display().to_string())
}

fn append_log(layout: &ProjectLayout, message: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(layout.logs.join("daemon.log"))?;
    writeln!(file, "{} {}", now_unix(), message)?;
    Ok(())
}

fn start_project_watcher(
    project_root: &Path,
) -> Result<(
    Option<RecommendedWatcher>,
    Option<Receiver<FileChangeEvent>>,
)> {
    let root = project_root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", project_root.display()))?;
    let (tx, rx) = mpsc::channel();
    let watcher = RecommendedWatcher::new(
        move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                if matches!(event.kind, EventKind::Modify(ModifyKind::Name(_)))
                    && event.paths.len() >= 2
                {
                    let from = event.paths.first().cloned();
                    let to = event.paths.last().cloned();
                    if let (Some(from), Some(to)) = (from, to) {
                        if !is_ignored_path(&from) || !is_ignored_path(&to) {
                            let _ = tx.send(FileChangeEvent::Renamed { from, to });
                        }
                    }
                } else {
                    let deleted = matches!(event.kind, EventKind::Remove(_));
                    for path in event.paths {
                        if !is_ignored_path(&path) {
                            let event = if deleted {
                                FileChangeEvent::Deleted(path)
                            } else {
                                FileChangeEvent::Changed(path)
                            };
                            let _ = tx.send(event);
                        }
                    }
                }
            }
        },
        Config::default(),
    )
    .context("create filesystem watcher")?;
    let mut watcher = watcher;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("watch {}", root.display()))?;
    Ok((Some(watcher), Some(rx)))
}

fn coalesce_file_events(events: Vec<FileChangeEvent>) -> Vec<FileChangeEvent> {
    let mut by_path: HashMap<PathBuf, FileChangeEvent> = HashMap::new();
    let mut renamed = Vec::new();
    for event in events {
        match event {
            FileChangeEvent::Renamed { from, to } => {
                renamed.push(FileChangeEvent::Renamed { from, to })
            }
            FileChangeEvent::Changed(path) => {
                by_path
                    .entry(path.clone())
                    .and_modify(|current| {
                        if !matches!(current, FileChangeEvent::Deleted(_)) {
                            *current = FileChangeEvent::Changed(path.clone());
                        }
                    })
                    .or_insert(FileChangeEvent::Changed(path));
            }
            FileChangeEvent::Deleted(path) => {
                by_path.insert(path.clone(), FileChangeEvent::Deleted(path));
            }
        }
    }
    let mut merged: Vec<FileChangeEvent> = by_path.into_values().collect();
    merged.extend(renamed);
    merged.sort_by(|left, right| event_sort_key(left).cmp(&event_sort_key(right)));
    merged
}

fn event_sort_key(event: &FileChangeEvent) -> PathBuf {
    match event {
        FileChangeEvent::Changed(path) | FileChangeEvent::Deleted(path) => path.clone(),
        FileChangeEvent::Renamed { from, .. } => from.clone(),
    }
}

fn process_file_event(
    project_root: &Path,
    conn: &rusqlite::Connection,
    cache: &mut HashMap<PathBuf, String>,
    event: FileChangeEvent,
) -> Result<bool> {
    match event {
        FileChangeEvent::Deleted(path) => {
            let result = delete_path(project_root, conn, cache, &path);
            let _ = memorycore_plugin_host::execute_hook(
                project_root,
                "onFileDeleted",
                serde_json::json!({"path": path.to_string_lossy().to_string()})
            );
            result
        }
        FileChangeEvent::Changed(path) => {
            let result = scan_changed_path(project_root, conn, cache, &path);
            let _ = memorycore_plugin_host::execute_hook(
                project_root,
                "onFileChanged",
                serde_json::json!({"path": path.to_string_lossy().to_string()})
            );
            result
        }
        FileChangeEvent::Renamed { from, to } => {
            let mut changed = delete_path(project_root, conn, cache, &from)?;
            changed |= scan_changed_path(project_root, conn, cache, &to)?;
            append_event(
                conn,
                "memorycore-daemon",
                "file_changed",
                &serde_json::json!({
                    "from": from.to_string_lossy().to_string(),
                    "to": to.to_string_lossy().to_string(),
                    "kind": "rename"
                }),
            )?;
            Ok(changed)
        }
    }
}

fn delete_path(
    project_root: &Path,
    conn: &rusqlite::Connection,
    cache: &mut HashMap<PathBuf, String>,
    path: &Path,
) -> Result<bool> {
    let existed = cache.remove(path).is_some();
    remove_file_content_tree_index(conn, path)?;
    remove_file_graph_index(conn, project_root, path)?;
    append_event(
        conn,
        "memorycore-daemon",
        "file_deleted",
        &serde_json::json!({
            "path": path.to_string_lossy().to_string()
        }),
    )?;
    Ok(existed)
}

fn scan_changed_path(
    project_root: &Path,
    conn: &rusqlite::Connection,
    cache: &mut HashMap<PathBuf, String>,
    path: &Path,
) -> Result<bool> {
    if path.is_dir() {
        let summary = memorycore_graph::scan_folder(conn, project_root, path)?;
        append_event(
            conn,
            "memorycore-daemon",
            "file_changed",
            &serde_json::json!({
                "path": path.to_string_lossy().to_string(),
                "kind": "directory",
                "files": summary.files,
                "folders": summary.folders,
                "edges": summary.edges
            }),
        )?;
        return Ok(true);
    }

    if path.is_file() {
        let bytes = fs::read(path)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let previous = cache.get(path);
        if previous == Some(&hash) {
            return Ok(false);
        }
        scan_file(conn, project_root, path)?;
        cache.insert(path.to_path_buf(), hash.clone());
        append_event(
            conn,
            "memorycore-daemon",
            "file_changed",
            &serde_json::json!({
                "path": path.to_string_lossy().to_string(),
                "hash": hash
            }),
        )?;
        return Ok(true);
    }

    Ok(false)
}

fn full_rescan(project_root: &Path, conn: &rusqlite::Connection) -> Result<()> {
    let root = project_root.canonicalize()?;
    let _ = memorycore_graph::scan_folder(conn, &root, &root);
    Ok(())
}

fn poll_project(
    project_root: &Path,
    conn: &rusqlite::Connection,
    cache: &mut HashMap<PathBuf, String>,
) -> Result<bool> {
    let current = snapshot_project(project_root)?;
    let mut changed = false;
    for (path, hash) in &current {
        match cache.get(path) {
            Some(previous) if previous == hash => {}
            _ => {
                changed = true;
                let changed_path = path.clone();
                append_event(
                    conn,
                    "memorycore-daemon",
                    "file_changed",
                    &serde_json::json!({
                        "path": changed_path.to_string_lossy().to_string(),
                        "hash": hash
                    }),
                )?;
                if changed_path.is_file() {
                    let _ = scan_file(conn, project_root, &changed_path);
                }
            }
        }
    }

    for path in cache.keys() {
        if !current.contains_key(path) {
            changed = true;
            let _ = remove_file_content_tree_index(conn, path);
            let _ = remove_file_graph_index(conn, project_root, path);
            append_event(
                conn,
                "memorycore-daemon",
                "file_deleted",
                &serde_json::json!({
                    "path": path.to_string_lossy().to_string()
                }),
            )?;
        }
    }

    *cache = current;
    Ok(changed)
}

fn poll_git(
    project_root: &Path,
    conn: &rusqlite::Connection,
    previous: &mut Option<String>,
) -> Result<bool> {
    let current = snapshot_git_head(project_root)?;
    let mut changed = false;
    if current != *previous {
        changed = true;
        if let Some(commit) = current.as_deref() {
            append_event(
                conn,
                "memorycore-daemon",
                "git_commit_detected",
                &serde_json::json!({
                    "commit": commit
                }),
            )?;
        }
        *previous = current;
    }
    Ok(changed)
}

fn poll_sessions(
    project_root: &Path,
    conn: &rusqlite::Connection,
    previous: &mut HashMap<PathBuf, String>,
) -> Result<bool> {
    let current = snapshot_sessions(project_root)?;
    let mut changed = false;
    for (path, hash) in &current {
        match previous.get(path) {
            Some(previous_hash) if previous_hash == hash => {}
            _ => {
                changed = true;
                import_session_archive(project_root, conn, path)?;
                append_event(
                    conn,
                    "memorycore-daemon",
                    "session_changed",
                    &serde_json::json!({
                        "path": path.to_string_lossy().to_string(),
                        "hash": hash
                    }),
                )?;
            }
        }
    }

    for path in previous.keys() {
        if !current.contains_key(path) {
            changed = true;
            let _ = remove_session_archive_state(conn, project_root, path);
            let _ = build_message_embeddings_with_conn(project_root, conn);
            append_event(
                conn,
                "memorycore-daemon",
                "session_deleted",
                &serde_json::json!({
                    "path": path.to_string_lossy().to_string()
                }),
            )?;
        }
    }

    *previous = current;
    Ok(changed)
}

fn poll_plugins(
    project_root: &Path,
    conn: &rusqlite::Connection,
    previous: &mut HashMap<PathBuf, String>,
) -> Result<bool> {
    let current = snapshot_registered_plugins(conn)?;
    let mut changed = false;
    for (path, hash) in &current {
        match previous.get(path) {
            Some(previous_hash) if previous_hash == hash => {}
            _ => {
                changed = true;
                install_plugin(project_root, path)?;
                append_event(
                    conn,
                    "memorycore-daemon",
                    "plugin_changed",
                    &serde_json::json!({
                        "path": path.to_string_lossy().to_string(),
                        "hash": hash
                    }),
                )?;
            }
        }
    }

    for path in previous.keys() {
        if !current.contains_key(path) {
            changed = true;
            conn.execute(
                "UPDATE plugins SET enabled = 0, updated_at = ?2 WHERE manifest_path = ?1",
                (path.to_string_lossy().to_string(), now_unix()),
            )?;
            disable_plugin_graph(conn, path)?;
            append_event(
                conn,
                "memorycore-daemon",
                "plugin_deleted",
                &serde_json::json!({
                    "path": path.to_string_lossy().to_string()
                }),
            )?;
        }
    }

    *previous = current;
    Ok(changed)
}

fn poll_skills(
    project_root: &Path,
    conn: &rusqlite::Connection,
    previous: &mut HashMap<PathBuf, String>,
) -> Result<bool> {
    let current = snapshot_registered_skills(conn)?;
    let mut changed = false;
    for (path, hash) in &current {
        match previous.get(path) {
            Some(previous_hash) if previous_hash == hash => {}
            _ => {
                changed = true;
                register_skill(project_root, path)?;
                append_event(
                    conn,
                    "memorycore-daemon",
                    "skill_changed",
                    &serde_json::json!({
                        "path": path.to_string_lossy().to_string(),
                        "hash": hash
                    }),
                )?;
            }
        }
    }

    for path in previous.keys() {
        if !current.contains_key(path) {
            changed = true;
            conn.execute(
                "UPDATE skills SET enabled = 0, updated_at = ?2 WHERE skill_path = ?1",
                (path.to_string_lossy().to_string(), now_unix()),
            )?;
            disable_skill_graph(conn, path)?;
            append_event(
                conn,
                "memorycore-daemon",
                "skill_deleted",
                &serde_json::json!({
                    "path": path.to_string_lossy().to_string()
                }),
            )?;
        }
    }

    *previous = current;
    Ok(changed)
}

fn snapshot_project(project_root: &Path) -> Result<HashMap<PathBuf, String>> {
    let mut files = HashMap::new();
    let root = project_root.canonicalize()?;
    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(|entry| !is_ignored(entry))
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || is_probably_binary(path) {
            continue;
        }
        let bytes = fs::read(path)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        files.insert(path.to_path_buf(), hash);
    }
    Ok(files)
}

fn snapshot_git_head(project_root: &Path) -> Result<Option<String>> {
    let git_dir = project_root.join(".git");
    let head_path = git_dir.join("HEAD");
    let Ok(head) = fs::read_to_string(&head_path) else {
        return Ok(None);
    };
    let head = head.trim().to_string();
    if let Some(reference) = head.strip_prefix("ref:") {
        let ref_path = git_dir.join(reference.trim());
        if let Ok(commit) = fs::read_to_string(ref_path) {
            let commit = commit.trim();
            if !commit.is_empty() {
                return Ok(Some(commit.to_string()));
            }
        }
    } else if !head.is_empty() {
        return Ok(Some(head));
    }
    Ok(None)
}

fn snapshot_sessions(project_root: &Path) -> Result<HashMap<PathBuf, String>> {
    let mut archives = HashMap::new();
    let sessions_root = ProjectLayout::new(project_root).sessions;
    if !sessions_root.exists() {
        return Ok(archives);
    }
    for entry in WalkDir::new(&sessions_root)
        .into_iter()
        .filter_entry(|entry| !is_ignored(entry))
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("zst") {
            continue;
        }
        let bytes = fs::read(path)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        archives.insert(path.to_path_buf(), hash);
    }
    Ok(archives)
}

fn snapshot_registered_plugins(conn: &rusqlite::Connection) -> Result<HashMap<PathBuf, String>> {
    let mut manifest_paths = Vec::new();
    let mut stmt = conn.prepare(
        r#"
        SELECT manifest_path
        FROM plugins
        WHERE COALESCE(manifest_path, '') <> ''
        ORDER BY id
        "#,
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        manifest_paths.push(PathBuf::from(row?));
    }

    snapshot_paths(&manifest_paths)
}

fn snapshot_registered_skills(conn: &rusqlite::Connection) -> Result<HashMap<PathBuf, String>> {
    let mut skill_paths = Vec::new();
    let mut stmt = conn.prepare(
        r#"
        SELECT skill_path
        FROM skills
        WHERE COALESCE(skill_path, '') <> ''
        ORDER BY id
        "#,
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        skill_paths.push(PathBuf::from(row?));
    }

    snapshot_paths(&skill_paths)
}

fn snapshot_paths(paths: &[PathBuf]) -> Result<HashMap<PathBuf, String>> {
    let mut output = HashMap::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(path)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        output.insert(path.clone(), hash);
    }
    Ok(output)
}

pub fn import_session_archive(
    project_root: &Path,
    conn: &rusqlite::Connection,
    path: &Path,
) -> Result<()> {
    let (agent, session_id) = session_identity(project_root, path)?;
    let session_path = path.to_string_lossy().to_string();
    let session_node_id = format!("session:{agent}:{session_id}");
    let content = decompress_session_archive(path)?;
    let mut rows = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line)
            .with_context(|| format!("parse JSONL line in {}", path.display()))?;
        let role = value
            .get("role")
            .and_then(serde_json::Value::as_str)
            .context("missing role")?
            .to_string();
        let content = value
            .get("content")
            .and_then(serde_json::Value::as_str)
            .context("missing content")?
            .to_string();
        let timestamp = value.get("timestamp").and_then(serde_json::Value::as_i64);
        let metadata = value
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        rows.push((role, content, timestamp, metadata));
    }
    remove_session_archive_state(conn, project_root, path)?;
    if rows.is_empty() {
        append_event(
            conn,
            "memorycore-daemon",
            "session_changed",
            &serde_json::json!({
                "id": session_id,
                "agent": agent,
                "path": session_path,
                "message_count": 0
            }),
        )?;
        return Ok(());
    }
    let message_count = rows.len();
    let started_at = rows.first().and_then(|row| row.2).unwrap_or_else(now_unix);
    let ended_at = rows.last().and_then(|row| row.2).unwrap_or(started_at);
    let project_name = project_root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| project_root.display().to_string());
    upsert_node(
        &conn,
        &GraphNode {
            id: "project:root".to_string(),
            kind: "Project".to_string(),
            name: project_name,
            path: Some(".".to_string()),
            span_start: None,
            span_end: None,
            hash: None,
            metadata: serde_json::json!({}),
        },
    )?;
    let session_name = format!("{agent} {session_id}");
    upsert_node(
        &conn,
        &GraphNode {
            id: session_node_id.clone(),
            kind: "Session".to_string(),
            name: session_name,
            path: Some(session_path.clone()),
            span_start: None,
            span_end: None,
            hash: None,
            metadata: serde_json::json!({
                "agent": agent,
                "session_id": session_id,
                "message_count": rows.len(),
                "source": path.to_string_lossy().to_string(),
            }),
        },
    )?;
    upsert_edge(
        &conn,
        &GraphEdge {
            id: format!("edge:project:root:contains:{session_node_id}"),
            source_id: "project:root".to_string(),
            target_id: session_node_id.clone(),
            kind: "contains".to_string(),
            weight: 1.0,
            confidence: 1.0,
            metadata: serde_json::json!({
                "agent": agent,
                "session_id": session_id,
                "path": session_path
            }),
        },
    )?;
    conn.execute(
        r#"
        INSERT OR REPLACE INTO sessions
            (id, agent, started_at, ended_at, token_count, message_count)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        (
            &session_id,
            &agent,
            started_at,
            Some(ended_at),
            0_i64,
            rows.len() as i64,
        ),
    )?;
    conn.execute("DELETE FROM messages WHERE session_id = ?1", [&session_id])?;
    conn.execute(
        "DELETE FROM messages_fts WHERE session_id = ?1",
        [&session_id],
    )?;
    for (index, (role, content, timestamp, metadata)) in rows.into_iter().enumerate() {
        conn.execute(
            r#"
            INSERT INTO messages (session_id, role, content, timestamp, metadata)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            (
                &session_id,
                &role,
                &content,
                timestamp.unwrap_or(started_at),
                metadata.to_string(),
            ),
        )?;
        let rowid = conn.last_insert_rowid();
        conn.execute(
            r#"
            INSERT INTO messages_fts (rowid, session_id, role, content, timestamp)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            (
                rowid,
                &session_id,
                &role,
                &content,
                timestamp.unwrap_or(started_at),
            ),
        )?;
        let message_node_id = format!("message:{agent}:{session_id}:{index}");
        upsert_node(
            &conn,
            &GraphNode {
                id: message_node_id.clone(),
                kind: "Message".to_string(),
                name: format!("{role} #{index}"),
                path: Some(session_path.clone()),
                span_start: None,
                span_end: None,
                hash: Some(format!("{:x}", Sha256::digest(content.as_bytes()))),
                metadata: serde_json::json!({
                    "agent": agent,
                    "session_id": session_id,
                    "role": role,
                    "timestamp": timestamp.unwrap_or(started_at),
                    "content_length": content.len(),
                }),
            },
        )?;
        upsert_edge(
            &conn,
            &GraphEdge {
                id: format!("edge:{session_node_id}:contains:{message_node_id}"),
                source_id: session_node_id.clone(),
                target_id: message_node_id,
                kind: "contains".to_string(),
                weight: 1.0,
                confidence: 1.0,
                metadata: serde_json::json!({
                    "role": role,
                    "index": index,
                }),
            },
        )?;
    }
    append_event(
        conn,
        "memorycore-daemon",
        "session_changed",
        &serde_json::json!({
            "id": session_id,
            "agent": agent,
            "path": session_path,
            "message_count": message_count
        }),
    )?;
    let _ = build_message_embeddings_with_conn(project_root, conn);
    Ok(())
}

fn remove_session_archive_state(
    conn: &rusqlite::Connection,
    project_root: &Path,
    path: &Path,
) -> Result<()> {
    let (_agent, session_id) = session_identity(project_root, path)?;
    let session_path = path.to_string_lossy().to_string();
    conn.execute("DELETE FROM messages WHERE session_id = ?1", [&session_id])?;
    conn.execute(
        "DELETE FROM messages_fts WHERE session_id = ?1",
        [&session_id],
    )?;
    conn.execute("DELETE FROM sessions WHERE id = ?1", [&session_id])?;
    conn.execute(
        "DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE path = ?1) OR target_id IN (SELECT id FROM graph_nodes WHERE path = ?1)",
        [&session_path],
    )?;
    conn.execute("DELETE FROM graph_nodes WHERE path = ?1", [&session_path])?;
    Ok(())
}

fn session_identity(project_root: &Path, path: &Path) -> Result<(String, String)> {
    let sessions_root = ProjectLayout::new(project_root).sessions;
    let rel = path.strip_prefix(&sessions_root).unwrap_or(path);
    let agent = rel
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
        .context("resolve session agent")?
        .to_string();
    let session_file = path
        .file_name()
        .and_then(|stem| stem.to_str())
        .context("resolve session id")?;
    let session_id = if let Some(base) = session_file.strip_suffix(".jsonl.zst") {
        base.to_string()
    } else {
        session_file
            .strip_suffix(".jsonl")
            .unwrap_or(session_file)
            .to_string()
    };
    Ok((agent, session_id))
}

fn decompress_session_archive(path: &Path) -> Result<String> {
    let output = Command::new("zstd")
        .arg("-dc")
        .arg(path)
        .output()
        .with_context(|| format!("decompress session archive {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "zstd decompressor failed with status {}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn is_ignored(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    matches!(
        name.as_ref(),
        ".git" | ".memorycore" | "target" | "node_modules" | ".codegraph" | ".codex" | ".agents"
    )
}

fn is_ignored_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(".git")
                | Some(".memorycore")
                | Some("target")
                | Some("node_modules")
                | Some(".codegraph")
                | Some(".codex")
                | Some(".agents")
        )
    })
}

fn is_probably_binary(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()).unwrap_or(""),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "pdf" | "zip" | "gz" | "zst" | "db"
    )
}

fn create_project_snapshot(project_root: &Path, conn: &rusqlite::Connection) -> Result<()> {
    let _ = create_snapshot(
        project_root,
        conn,
        "daemon project snapshot",
        "memorycore-daemon",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memorycore_core::{connect_project_db, init_project};
    use memorycore_graph::scan_file;
    use memorycore_plugin_host::{install_plugin, register_skill};
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    #[test]
    fn poll_project_records_file_changes() {
        let temp = tempdir().expect("temp dir");
        fs::write(temp.path().join("main.rs"), "fn main() {}\n").expect("write file");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");

        let mut cache = snapshot_project(temp.path()).expect("snapshot");
        fs::write(
            temp.path().join("main.rs"),
            "fn main(){println!(\"hi\");}\n",
        )
        .expect("change");

        let changed = poll_project(temp.path(), &conn, &mut cache).expect("poll");
        assert!(changed);

        let event_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE event_type = 'file_changed'",
                [],
                |row| row.get(0),
            )
            .expect("event count");
        assert!(event_count >= 1);
    }

    #[test]
    fn poll_project_prunes_stale_graph_for_changed_files() {
        let temp = tempdir().expect("temp dir");
        let path = temp.path().join("main.rs");
        fs::write(
            &path,
            "fn helper() {}\nfn other() {}\nfn main() { helper(); }\n",
        )
        .expect("write file");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");

        let mut cache = snapshot_project(temp.path()).expect("snapshot");
        scan_file(&conn, temp.path(), &path).expect("initial scan");
        fs::write(&path, "fn other() {}\nfn main() { other(); }\n").expect("rewrite file");

        let changed = poll_project(temp.path(), &conn, &mut cache).expect("poll");
        assert!(changed);

        let helper_nodes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE id = 'symbol:main.rs#helper'",
                [],
                |row| row.get(0),
            )
            .expect("count helper nodes");
        assert_eq!(helper_nodes, 0);

        let stale_calls: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_edges WHERE kind = 'calls' AND target_id = 'symbol:main.rs#helper'",
                [],
                |row| row.get(0),
            )
            .expect("count stale calls");
        assert_eq!(stale_calls, 0);
    }

    #[test]
    fn touch_status_updates_last_activity() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let layout = ProjectLayout::new(temp.path());
        let status = DaemonStatus {
            pid: 1234,
            started_at: 42,
            project_root: temp.path().display().to_string(),
            last_activity_at: 100,
        };
        write_status(&layout, &status).expect("write status");

        let before: DaemonStatus = serde_json::from_str(
            &fs::read_to_string(layout.memorycore.join("daemon.json")).expect("read status"),
        )
        .expect("parse status");
        assert_eq!(before.last_activity_at, 100);

        touch_status(&layout, &status).expect("touch status");
        let after: DaemonStatus = serde_json::from_str(
            &fs::read_to_string(layout.memorycore.join("daemon.json")).expect("read status"),
        )
        .expect("parse status");
        assert!(after.last_activity_at >= before.last_activity_at);
    }

    #[test]
    fn status_rejects_stale_pid_files() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let layout = ProjectLayout::new(temp.path());
        let daemon_status = DaemonStatus {
            pid: u32::MAX,
            started_at: 1,
            project_root: temp.path().display().to_string(),
            last_activity_at: 2,
        };
        write_status(&layout, &daemon_status).expect("write status");
        let result = status(temp.path());
        assert!(result.is_err());
    }

    #[test]
    fn poll_project_removes_deleted_file_content_index() {
        let temp = tempdir().expect("temp dir");
        let path = temp.path().join("main.rs");
        fs::write(&path, "fn main() { println!(\"hello\"); }\n").expect("write file");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");
        scan_file(&conn, temp.path(), &path).expect("initial scan");

        let mut cache = snapshot_project(temp.path()).expect("snapshot");
        let changed = poll_project(temp.path(), &conn, &mut cache).expect("initial poll");
        assert!(!changed);
        let indexed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_contents WHERE path = ?1",
                [path.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .expect("count indexed file");
        assert_eq!(indexed, 1);

        fs::remove_file(&path).expect("remove file");
        let changed = poll_project(temp.path(), &conn, &mut cache).expect("delete poll");
        assert!(changed);

        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_contents WHERE path = ?1",
                [path.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .expect("count deleted file");
        assert_eq!(remaining, 0);

        let graph_remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE path = ?1",
                [path.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .expect("count deleted graph nodes");
        assert_eq!(graph_remaining, 0);
    }

    #[test]
    fn poll_git_change_triggers_snapshot() {
        let temp = tempdir().expect("temp dir");
        let git_dir = temp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).expect("git dirs");
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").expect("write head");
        fs::write(git_dir.join("refs/heads/main"), "abc123\n").expect("write ref");
        fs::write(temp.path().join("main.rs"), "fn main() {}\n").expect("write file");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");

        let mut git_head = snapshot_git_head(temp.path()).expect("snapshot git");
        fs::write(git_dir.join("refs/heads/main"), "def456\n").expect("update ref");
        let changed = poll_git(temp.path(), &conn, &mut git_head).expect("poll git");
        assert!(changed);
        create_project_snapshot(temp.path(), &conn).expect("snapshot after git change");

        let snapshot_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .expect("snapshot count");
        let snapshot_event_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE event_type = 'snapshot_created'",
                [],
                |row| row.get(0),
            )
            .expect("snapshot events");
        assert!(snapshot_count >= 1);
        assert!(snapshot_event_count >= 1);
    }

    #[test]
    fn poll_session_change_triggers_snapshot() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");
        let sessions_dir = ProjectLayout::new(temp.path()).sessions.join("codex");
        fs::create_dir_all(&sessions_dir).expect("create session dir");

        let jsonl = sessions_dir.join("demo.jsonl");
        let archive = sessions_dir.join("demo.jsonl.zst");
        fs::write(&jsonl, r#"{"role":"user","content":"hello","timestamp":1}"#)
            .expect("write jsonl");
        let status = Command::new("zstd")
            .arg("-q")
            .arg("-f")
            .arg("-o")
            .arg(&archive)
            .arg(&jsonl)
            .status()
            .expect("compress session archive");
        assert!(status.success());
        let _ = fs::remove_file(&jsonl);

        let mut session_cache = snapshot_sessions(temp.path()).expect("session snapshot");
        fs::write(
            &jsonl,
            r#"{"role":"user","content":"hello again","timestamp":2}"#,
        )
        .expect("write jsonl update");
        let status = Command::new("zstd")
            .arg("-q")
            .arg("-f")
            .arg("-o")
            .arg(&archive)
            .arg(&jsonl)
            .status()
            .expect("recompress session archive");
        assert!(status.success());
        let _ = fs::remove_file(&jsonl);

        let changed = poll_sessions(temp.path(), &conn, &mut session_cache).expect("poll sessions");
        assert!(changed);
        create_project_snapshot(temp.path(), &conn).expect("snapshot after session change");

        let snapshot_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .expect("snapshot count");
        let snapshot_event_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE event_type = 'snapshot_created'",
                [],
                |row| row.get(0),
            )
            .expect("snapshot events");
        assert!(snapshot_count >= 1);
        assert!(snapshot_event_count >= 1);
    }

    #[test]
    fn import_session_archive_replaces_existing_session_graph_and_messages() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");
        let sessions_dir = ProjectLayout::new(temp.path()).sessions.join("codex");
        fs::create_dir_all(&sessions_dir).expect("create session dir");

        let jsonl = sessions_dir.join("demo.jsonl");
        let archive = sessions_dir.join("demo.jsonl.zst");
        fs::write(
            &jsonl,
            concat!(
                r#"{"role":"user","content":"hello","timestamp":1}"#,
                "\n",
                r#"{"role":"assistant","content":"world","timestamp":2}"#,
                "\n"
            ),
        )
        .expect("write jsonl");
        let status = Command::new("zstd")
            .arg("-q")
            .arg("-f")
            .arg("-o")
            .arg(&archive)
            .arg(&jsonl)
            .status()
            .expect("compress session archive");
        assert!(status.success());
        let _ = fs::remove_file(&jsonl);

        import_session_archive(temp.path(), &conn, &archive).expect("import initial archive");

        fs::write(
            &jsonl,
            r#"{"role":"user","content":"hello again","timestamp":3}"#,
        )
        .expect("write jsonl replacement");
        let status = Command::new("zstd")
            .arg("-q")
            .arg("-f")
            .arg("-o")
            .arg(&archive)
            .arg(&jsonl)
            .status()
            .expect("recompress session archive");
        assert!(status.success());
        let _ = fs::remove_file(&jsonl);

        import_session_archive(temp.path(), &conn, &archive).expect("replace session archive");

        let message_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .expect("count messages");
        assert_eq!(message_count, 1);

        let graph_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE path = ?1",
                [archive.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .expect("count session graph nodes");
        assert_eq!(graph_count, 2);

        let embedding_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
            .expect("count embeddings");
        assert_eq!(embedding_count, 1);
    }

    #[test]
    fn poll_session_deletion_removes_session_graph_and_messages() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");
        let sessions_dir = ProjectLayout::new(temp.path()).sessions.join("codex");
        fs::create_dir_all(&sessions_dir).expect("create session dir");

        let jsonl = sessions_dir.join("demo.jsonl");
        let archive = sessions_dir.join("demo.jsonl.zst");
        fs::write(&jsonl, r#"{"role":"user","content":"hello","timestamp":1}"#)
            .expect("write jsonl");
        let status = Command::new("zstd")
            .arg("-q")
            .arg("-f")
            .arg("-o")
            .arg(&archive)
            .arg(&jsonl)
            .status()
            .expect("compress session archive");
        assert!(status.success());
        let _ = fs::remove_file(&jsonl);

        import_session_archive(temp.path(), &conn, &archive).expect("import session archive");
        let mut session_cache = snapshot_sessions(temp.path()).expect("snapshot sessions");

        fs::remove_file(&archive).expect("remove archive");
        let changed = poll_sessions(temp.path(), &conn, &mut session_cache).expect("poll delete");
        assert!(changed);

        let messages: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .expect("count messages");
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .expect("count sessions");
        let graph_nodes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE path = ?1",
                [archive.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .expect("count session graph nodes");
        let embeddings: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
            .expect("count embeddings");

        assert_eq!(messages, 0);
        assert_eq!(sessions, 0);
        assert_eq!(graph_nodes, 0);
        assert_eq!(embeddings, 0);
    }

    #[test]
    fn snapshot_project_hashes_files() {
        let temp = tempdir().expect("temp dir");
        fs::write(temp.path().join("main.rs"), "fn main() {}\n").expect("write file");

        let snapshot = snapshot_project(temp.path()).expect("snapshot");
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot.keys().any(|path| path.ends_with("main.rs")));
    }

    #[test]
    fn snapshot_git_head_reads_ref_or_direct_hash() {
        let temp = tempdir().expect("temp dir");
        let git_dir = temp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).expect("git dirs");
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").expect("write head");
        fs::write(git_dir.join("refs/heads/main"), "abc123\n").expect("write ref");

        let commit = snapshot_git_head(temp.path()).expect("snapshot git");
        assert_eq!(commit.as_deref(), Some("abc123"));

        fs::write(git_dir.join("HEAD"), "def456\n").expect("write detached head");
        let commit = snapshot_git_head(temp.path()).expect("snapshot detached git");
        assert_eq!(commit.as_deref(), Some("def456"));
    }

    #[test]
    fn session_identity_strips_jsonl_zst_suffix() {
        let temp = tempdir().expect("temp dir");
        let layout = ProjectLayout::new(temp.path());
        let session = layout.sessions.join("codex").join("demo.jsonl.zst");
        let (agent, session_id) = session_identity(temp.path(), &session).expect("identity");
        assert_eq!(agent, "codex");
        assert_eq!(session_id, "demo");
    }

    #[test]
    fn watcher_ignores_memorycore_and_target_paths() {
        assert!(is_ignored_path(Path::new(
            "/tmp/project/.memorycore/index.db"
        )));
        assert!(is_ignored_path(Path::new(
            "/tmp/project/target/debug/memorycore"
        )));
        assert!(!is_ignored_path(Path::new("/tmp/project/src/main.rs")));
    }

    #[test]
    fn coalesces_file_events_by_path_and_preserves_delete() {
        let events = vec![
            FileChangeEvent::Changed(PathBuf::from("src/main.rs")),
            FileChangeEvent::Deleted(PathBuf::from("src/main.rs")),
            FileChangeEvent::Changed(PathBuf::from("src/lib.rs")),
            FileChangeEvent::Renamed {
                from: PathBuf::from("src/old.rs"),
                to: PathBuf::from("src/new.rs"),
            },
        ];

        let merged = coalesce_file_events(events);
        assert_eq!(merged.len(), 3);
        assert!(merged
            .iter()
            .any(|event| matches!(event, FileChangeEvent::Deleted(path) if path == &PathBuf::from("src/main.rs"))));
        assert!(merged
            .iter()
            .any(|event| matches!(event, FileChangeEvent::Changed(path) if path == &PathBuf::from("src/lib.rs"))));
        assert!(merged.iter().any(|event| matches!(
            event,
            FileChangeEvent::Renamed { from, to }
                if from == &PathBuf::from("src/old.rs") && to == &PathBuf::from("src/new.rs")
        )));
    }

    #[test]
    fn renamed_file_event_is_preserved() {
        let events = vec![FileChangeEvent::Renamed {
            from: PathBuf::from("src/old.rs"),
            to: PathBuf::from("src/new.rs"),
        }];

        let merged = coalesce_file_events(events);
        assert_eq!(merged.len(), 1);
        assert!(matches!(
            &merged[0],
            FileChangeEvent::Renamed { from, to }
                if from == &PathBuf::from("src/old.rs") && to == &PathBuf::from("src/new.rs")
        ));
    }

    #[test]
    fn native_watcher_triggers_on_file_change() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let (_watcher, rx) = start_project_watcher(temp.path()).expect("start watcher");

        let path = temp.path().join("src").join("main.rs");
        fs::create_dir_all(path.parent().expect("parent dir")).expect("create src dir");
        fs::write(&path, "fn main() {}\n").expect("write watched file");

        rx.expect("watcher receiver")
            .recv_timeout(Duration::from_secs(5))
            .expect("file watcher event");
    }

    #[test]
    fn renamed_file_event_moves_graph_state_to_new_path() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");
        let mut cache = HashMap::new();
        let old_path = temp.path().join("src").join("old.rs");
        let new_path = temp.path().join("src").join("new.rs");
        fs::create_dir_all(old_path.parent().expect("parent dir")).expect("create src dir");
        fs::write(&old_path, "fn main() {}\n").expect("write old file");

        let changed = process_file_event(
            temp.path(),
            &conn,
            &mut cache,
            FileChangeEvent::Changed(old_path.clone()),
        )
        .expect("seed graph state");
        assert!(changed);

        fs::rename(&old_path, &new_path).expect("rename file");
        let changed = process_file_event(
            temp.path(),
            &conn,
            &mut cache,
            FileChangeEvent::Renamed {
                from: old_path.clone(),
                to: new_path.clone(),
            },
        )
        .expect("process rename");
        assert!(changed);

        let old_nodes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE path = ?1",
                [old_path
                    .strip_prefix(temp.path())
                    .expect("relative old path")
                    .to_string_lossy()
                    .to_string()],
                |row| row.get(0),
            )
            .expect("count old nodes");
        let new_nodes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE path = ?1",
                [new_path
                    .strip_prefix(temp.path())
                    .expect("relative new path")
                    .to_string_lossy()
                    .to_string()],
                |row| row.get(0),
            )
            .expect("count new nodes");
        let old_content: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_contents WHERE path = ?1",
                [old_path.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .expect("count old content");
        let new_content: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_contents WHERE path = ?1",
                [new_path.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .expect("count new content");

        assert_eq!(old_nodes, 0);
        assert!(new_nodes >= 1);
        assert_eq!(old_content, 0);
        assert_eq!(new_content, 1);
    }

    #[test]
    fn renamed_directory_event_moves_nested_graph_and_content_state() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");
        let mut cache = HashMap::new();
        let old_dir = temp.path().join("src").join("old");
        let new_dir = temp.path().join("src").join("new");
        let old_file = old_dir.join("mod.rs");
        let new_file = new_dir.join("mod.rs");
        fs::create_dir_all(&old_dir).expect("create old dir");
        fs::write(&old_file, "pub fn hello() {}\n").expect("write old file");

        let changed = process_file_event(
            temp.path(),
            &conn,
            &mut cache,
            FileChangeEvent::Changed(old_file.clone()),
        )
        .expect("seed graph state");
        assert!(changed);

        fs::rename(&old_dir, &new_dir).expect("rename directory");
        let changed = process_file_event(
            temp.path(),
            &conn,
            &mut cache,
            FileChangeEvent::Renamed {
                from: old_dir.clone(),
                to: new_dir.clone(),
            },
        )
        .expect("process directory rename");
        assert!(changed);

        let old_rel_dir = old_dir
            .strip_prefix(temp.path())
            .expect("relative old dir")
            .to_string_lossy()
            .to_string();
        let new_rel_dir = new_dir
            .strip_prefix(temp.path())
            .expect("relative new dir")
            .to_string_lossy()
            .to_string();
        let old_graph_nodes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE path = ?1 OR path LIKE ?2",
                [old_rel_dir.clone(), format!("{old_rel_dir}/%")],
                |row| row.get(0),
            )
            .expect("count old graph nodes");
        let new_graph_nodes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE path = ?1 OR path LIKE ?2",
                [new_rel_dir.clone(), format!("{new_rel_dir}/%")],
                |row| row.get(0),
            )
            .expect("count new graph nodes");
        let old_content: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_contents WHERE path = ?1 OR path LIKE ?2",
                [
                    old_file.to_string_lossy().to_string(),
                    format!("{}/%", old_file.to_string_lossy()),
                ],
                |row| row.get(0),
            )
            .expect("count old content");
        let new_content: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_contents WHERE path = ?1 OR path LIKE ?2",
                [
                    new_file.to_string_lossy().to_string(),
                    format!("{}/%", new_file.to_string_lossy()),
                ],
                |row| row.get(0),
            )
            .expect("count new content");

        assert_eq!(old_graph_nodes, 0);
        assert!(new_graph_nodes >= 1);
        assert_eq!(old_content, 0);
        assert_eq!(new_content, 1);
    }

    #[test]
    fn poll_plugins_and_skills_refresh_registry_state() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");

        let plugin_manifest = temp.path().join("plugin.json");
        fs::write(
            &plugin_manifest,
            r#"{
              "id": "test-plugin",
              "name": "Test Plugin",
              "version": "1.0.0",
              "entry": "bin/test-plugin",
              "capabilities": ["read_project_files"],
              "hooks": ["onDaemonStart"]
            }"#,
        )
        .expect("write plugin");
        install_plugin(temp.path(), &plugin_manifest).expect("install plugin");

        let skill_dir = temp.path().join("skills").join("test-skill");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "# Test Skill\nInitial description\n",
        )
        .expect("write skill");
        register_skill(temp.path(), &skill_dir).expect("register skill");

        let mut plugin_cache = snapshot_registered_plugins(&conn).expect("plugin snapshot");
        let mut skill_cache = snapshot_registered_skills(&conn).expect("skill snapshot");

        fs::write(
            &plugin_manifest,
            r#"{
              "id": "test-plugin",
              "name": "Test Plugin",
              "version": "1.1.0",
              "entry": "bin/test-plugin",
              "capabilities": ["read_project_files", "emit_events"],
              "hooks": ["onDaemonStart"]
            }"#,
        )
        .expect("update plugin");
        fs::write(
            skill_dir.join("SKILL.md"),
            "# Test Skill\nUpdated description\n",
        )
        .expect("update skill");

        let plugin_changed =
            poll_plugins(temp.path(), &conn, &mut plugin_cache).expect("poll plugins");
        let skill_changed = poll_skills(temp.path(), &conn, &mut skill_cache).expect("poll skills");
        assert!(plugin_changed);
        assert!(skill_changed);
        create_project_snapshot(temp.path(), &conn).expect("snapshot after plugin/skill changes");

        fs::remove_file(&plugin_manifest).expect("remove plugin");
        fs::remove_file(skill_dir.join("SKILL.md")).expect("remove skill");
        let plugin_changed =
            poll_plugins(temp.path(), &conn, &mut plugin_cache).expect("delete plugin");
        let skill_changed =
            poll_skills(temp.path(), &conn, &mut skill_cache).expect("delete skill");
        assert!(plugin_changed);
        assert!(skill_changed);

        let plugin_version: String = conn
            .query_row(
                "SELECT version FROM plugins WHERE id = 'test-plugin'",
                [],
                |row| row.get(0),
            )
            .expect("plugin version");
        assert_eq!(plugin_version, "1.1.0");

        let skill_description: String = conn
            .query_row(
                "SELECT COALESCE(description, '') FROM skills WHERE name = 'test-skill'",
                [],
                |row| row.get(0),
            )
            .expect("skill description");
        assert_eq!(skill_description, "Updated description");

        let plugin_enabled: i64 = conn
            .query_row(
                "SELECT enabled FROM plugins WHERE id = 'test-plugin'",
                [],
                |row| row.get(0),
            )
            .expect("plugin enabled");
        assert_eq!(plugin_enabled, 0);
        let plugin_graph_enabled: i64 = conn
            .query_row(
                "SELECT json_extract(metadata, '$.enabled') FROM graph_nodes WHERE id = 'plugin:test-plugin'",
                [],
                |row| row.get(0),
            )
            .expect("plugin graph enabled");
        assert_eq!(plugin_graph_enabled, 0);

        let skill_enabled: i64 = conn
            .query_row(
                "SELECT enabled FROM skills WHERE id = 'test-skill'",
                [],
                |row| row.get(0),
            )
            .expect("skill enabled");
        assert_eq!(skill_enabled, 0);
        let skill_graph_enabled: i64 = conn
            .query_row(
                "SELECT json_extract(metadata, '$.enabled') FROM graph_nodes WHERE id = 'skill:test-skill'",
                [],
                |row| row.get(0),
            )
            .expect("skill graph enabled");
        assert_eq!(skill_graph_enabled, 0);

        let plugin_event_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE event_type = 'plugin_changed'",
                [],
                |row| row.get(0),
            )
            .expect("plugin change events");
        let skill_event_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE event_type = 'skill_changed'",
                [],
                |row| row.get(0),
            )
            .expect("skill change events");
        let plugin_deleted_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE event_type = 'plugin_deleted'",
                [],
                |row| row.get(0),
            )
            .expect("plugin deleted events");
        let skill_deleted_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE event_type = 'skill_deleted'",
                [],
                |row| row.get(0),
            )
            .expect("skill deleted events");
        let snapshot_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .expect("snapshot count");
        let snapshot_event_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_log WHERE event_type = 'snapshot_created'",
                [],
                |row| row.get(0),
            )
            .expect("snapshot events");
        assert!(plugin_event_count >= 1);
        assert!(skill_event_count >= 1);
        assert!(plugin_deleted_count >= 1);
        assert!(skill_deleted_count >= 1);
        assert!(snapshot_count >= 1);
        assert!(snapshot_event_count >= 1);
    }
}
