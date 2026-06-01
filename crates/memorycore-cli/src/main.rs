use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use mcp::search_index;
use memorycore_core::{
    analyze_target, connect_project_db, create_snapshot, format_analysis_report, init_project,
    list_snapshots, now_unix, render_analysis_mermaid, snapshot_count, snapshot_details,
};
use memorycore_graph::impact::find_impact_with_depth;
use memorycore_graph::model::{GraphEdge, GraphNode};
use memorycore_graph::query::{graph_target_json_depth, graph_target_mermaid_depth};
use memorycore_graph::render::json::render_json;
use memorycore_graph::render::mermaid::render_mermaid;
use memorycore_graph::{scan_file, scan_folder};
use rusqlite::params;
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};

mod mcp;

#[derive(Debug, Parser)]
#[command(
    name = "memorycore",
    version,
    about = "Local-first coding memory system"
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    project_root: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Status,
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Graph {
        #[command(subcommand)]
        command: GraphCommand,
    },
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        kind: Option<String>,
    },
    Analyze {
        target: String,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = AnalysisFormat::Text)]
        format: AnalysisFormat,
    },
    Snapshots {
        #[command(subcommand)]
        command: SnapshotCommand,
    },
    Events {
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        node: Option<String>,
        #[arg(long)]
        follow: bool,
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    Plugins {
        #[command(subcommand)]
        command: PluginCommand,
    },
    Skills {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Adapters {
        #[command(subcommand)]
        command: AdapterCommand,
    },
    Embeddings {
        #[command(subcommand)]
        command: EmbeddingCommand,
    },
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Api {
        #[command(subcommand)]
        command: ApiCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Start,
    Status,
    Stop,
    Logs,
    #[command(hide = true)]
    Run,
}

#[derive(Debug, Subcommand)]
enum GraphCommand {
    File {
        path: PathBuf,
    },
    Folder {
        path: PathBuf,
    },
    Query {
        target: String,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, value_enum, default_value_t = GraphQueryFormat::Json)]
        format: GraphQueryFormat,
    },
    Impact {
        target: String,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long, default_value_t = 1)]
        depth: usize,
    },
    Export {
        #[arg(long, value_enum, default_value_t = ExportFormat::Mermaid)]
        format: ExportFormat,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    Serve,
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    Install { manifest: PathBuf },
    List,
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    Register { path: PathBuf },
    List,
}

#[derive(Debug, Subcommand)]
enum AdapterCommand {
    Register {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        session_dir: Option<PathBuf>,
        #[arg(long)]
        command: Option<String>,
    },
    List,
}

#[derive(Debug, Subcommand)]
enum EmbeddingCommand {
    Build,
    List,
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

#[derive(Debug, Subcommand)]
enum SnapshotCommand {
    Create {
        #[arg(long)]
        message: Option<String>,
    },
    List {
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    Show {
        hash: String,
    },
}

#[derive(Debug, Subcommand)]
enum MemoryCommand {
    Pin {
        name: String,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        target: Option<String>,
    },
    List,
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    Import {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        id: String,
        path: PathBuf,
    },
    List,
    Show {
        id: String,
    },
}

#[derive(Debug, Subcommand)]
enum ApiCommand {
    Serve,
}

#[derive(Debug, Clone, ValueEnum)]
enum ExportFormat {
    Mermaid,
    Json,
}

#[derive(Debug, Clone, ValueEnum)]
enum GraphQueryFormat {
    Json,
    Mermaid,
}

#[derive(Debug, Clone, ValueEnum)]
enum AnalysisFormat {
    Text,
    Json,
    Mermaid,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let project_root = cli
        .project_root
        .canonicalize()
        .with_context(|| format!("resolve project root {}", cli.project_root.display()))?;

    match cli.command {
        Command::Init => {
            let layout = init_project(&project_root)?;
            println!("Initialized MemoryCore at {}", layout.memorycore.display());
        }
        Command::Status => {
            let conn = connect_project_db(&project_root)?;
            let node_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM graph_nodes", [], |row| row.get(0))?;
            let edge_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))?;
            let plugin_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM plugins", [], |row| row.get(0))?;
            let skill_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))?;
            let adapter_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM adapters", [], |row| row.get(0))?;
            let memory_case_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM memory_cases", [], |row| row.get(0))?;
            let embedding_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))?;
            let snapshot_total = snapshot_count(&conn)?;
            let daemon_status = memorycore_daemon::status(project_root.as_path());
            println!("MemoryCore project: {}", project_root.display());
            println!("Graph nodes: {node_count}");
            println!("Graph edges: {edge_count}");
            println!("Plugins: {plugin_count}");
            println!("Skills: {skill_count}");
            println!("Adapters: {adapter_count}");
            println!("Memory cases: {memory_case_count}");
            println!("Embeddings: {embedding_count}");
            println!("Snapshots: {snapshot_total}");
            match daemon_status {
                Ok(status) => println!(
                    "Daemon: running pid={} started_at={} last_activity_at={}",
                    status.pid, status.started_at, status.last_activity_at
                ),
                Err(error) => println!("Daemon: not running ({error})"),
            }
        }
        Command::Daemon { command } => handle_daemon(command, &project_root)?,
        Command::Graph { command } => handle_graph(command, &project_root)?,
        Command::Search { query, limit, kind } => {
            handle_search(&project_root, &query, limit, kind.as_deref())?
        }
        Command::Analyze {
            target,
            depth,
            limit,
            format,
        } => handle_analyze(&project_root, &target, depth, limit, format)?,
        Command::Snapshots { command } => handle_snapshots(command, &project_root)?,
        Command::Events {
            limit,
            status,
            node,
            follow,
            interval_ms,
        } => handle_events(
            &project_root,
            limit,
            status.as_deref(),
            node.as_deref(),
            follow,
            interval_ms,
        )?,
        Command::Mcp { command } => match command {
            McpCommand::Serve => mcp::serve(&project_root)?,
        },
        Command::Plugins { command } => handle_plugins(command, &project_root)?,
        Command::Skills { command } => handle_skills(command, &project_root)?,
        Command::Adapters { command } => handle_adapters(command, &project_root)?,
        Command::Embeddings { command } => handle_embeddings(command, &project_root)?,
        Command::Memory { command } => handle_memory(command, &project_root)?,
        Command::Sessions { command } => handle_sessions(command, &project_root)?,
        Command::Api { command } => match command {
            ApiCommand::Serve => memorycore_api::serve(&project_root, "127.0.0.1:7330")?,
        },
    }
    Ok(())
}

fn handle_analyze(
    project_root: &PathBuf,
    target: &str,
    depth: usize,
    limit: usize,
    format: AnalysisFormat,
) -> Result<()> {
    let conn = connect_project_db(project_root)?;
    let report = analyze_target(&conn, target, depth, limit)?;
    match format {
        AnalysisFormat::Text => print!("{}", format_analysis_report(&report)),
        AnalysisFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        AnalysisFormat::Mermaid => print!("{}", render_analysis_mermaid(&report)),
    }
    Ok(())
}

fn handle_daemon(command: DaemonCommand, project_root: &PathBuf) -> Result<()> {
    match command {
        DaemonCommand::Start => {
            let exe = std::env::current_exe().context("resolve current executable")?;
            let status = memorycore_daemon::start(project_root, &exe)?;
            println!("MemoryCore daemon running pid={}", status.pid);
        }
        DaemonCommand::Status => {
            let status = memorycore_daemon::status(project_root)?;
            println!(
                "MemoryCore daemon running pid={} started_at={} last_activity_at={}",
                status.pid, status.started_at, status.last_activity_at
            );
        }
        DaemonCommand::Stop => {
            let status = memorycore_daemon::stop(project_root)?;
            println!("Stopped MemoryCore daemon pid={}", status.pid);
        }
        DaemonCommand::Logs => {
            let path = memorycore_daemon::log_path(project_root);
            let logs = fs::read_to_string(&path)
                .with_context(|| format!("read daemon log {}", path.display()))?;
            print!("{logs}");
        }
        DaemonCommand::Run => memorycore_daemon::run(project_root)?,
    }
    Ok(())
}

fn handle_graph(command: GraphCommand, project_root: &PathBuf) -> Result<()> {
    let conn = connect_project_db(project_root)?;
    match command {
        GraphCommand::File { path } => {
            let path = absolutize(project_root, path);
            let summary = scan_file(&conn, project_root, &path)?;
            println!(
                "Scanned file: files={} folders={} edges={}",
                summary.files, summary.folders, summary.edges
            );
        }
        GraphCommand::Folder { path } => {
            let path = absolutize(project_root, path);
            let summary = scan_folder(&conn, project_root, &path)?;
            println!(
                "Scanned folder: files={} folders={} edges={}",
                summary.files, summary.folders, summary.edges
            );
        }
        GraphCommand::Query {
            target,
            depth,
            format,
        } => match format {
            GraphQueryFormat::Json => {
                let rendered = graph_target_json_depth(&conn, &target, depth)?;
                print!("{rendered}");
            }
            GraphQueryFormat::Mermaid => {
                let rendered = graph_target_mermaid_depth(&conn, &target, depth)?;
                print!("{rendered}");
            }
        },
        GraphCommand::Impact {
            target,
            limit,
            depth,
        } => {
            let rendered = find_impact_with_depth(&conn, &target, limit, depth)?;
            print!("{rendered}");
        }
        GraphCommand::Export { format, output } => match format {
            ExportFormat::Mermaid => {
                let rendered = render_mermaid(&conn)?;
                if let Some(output) = output {
                    fs::write(&output, rendered)
                        .with_context(|| format!("write {}", output.display()))?;
                    println!("Wrote Mermaid graph to {}", output.display());
                } else {
                    print!("{rendered}");
                }
            }
            ExportFormat::Json => {
                let rendered = render_json(&conn)?;
                if let Some(output) = output {
                    fs::write(&output, rendered)
                        .with_context(|| format!("write {}", output.display()))?;
                    println!("Wrote JSON graph to {}", output.display());
                } else {
                    print!("{rendered}");
                }
            }
        },
    }
    Ok(())
}

fn handle_search(
    project_root: &PathBuf,
    query: &str,
    limit: usize,
    kind: Option<&str>,
) -> Result<()> {
    let conn = connect_project_db(project_root)?;
    let rendered = search_index(&conn, query, limit, kind)?;
    print!("{rendered}");
    Ok(())
}

fn handle_snapshots(command: SnapshotCommand, project_root: &PathBuf) -> Result<()> {
    let conn = connect_project_db(project_root)?;
    match command {
        SnapshotCommand::Create { message } => {
            let message = message.as_deref().unwrap_or("manual snapshot request");
            let outcome = create_snapshot(project_root, &conn, message, "memorycore-cli")?;
            println!(
                "Snapshot {} created with {} files, {} bytes, event_log id={}",
                outcome.record.hash,
                outcome.record.file_count,
                outcome.record.total_size,
                outcome.event_id
            );
        }
        SnapshotCommand::List { limit } => {
            let snapshots = list_snapshots(&conn, limit)?;
            if snapshots.is_empty() {
                println!("No snapshots found");
            } else {
                for snapshot in snapshots {
                    println!("{}", serde_json::to_string(&snapshot)?);
                }
            }
        }
        SnapshotCommand::Show { hash } => match snapshot_details(&conn, &hash)? {
            Some(details) => {
                println!("{}", serde_json::to_string(&details)?);
            }
            None => {
                println!("Snapshot not found: {hash}");
            }
        },
    }
    Ok(())
}

fn handle_events(
    project_root: &PathBuf,
    limit: usize,
    status: Option<&str>,
    node: Option<&str>,
    follow: bool,
    interval_ms: u64,
) -> Result<()> {
    let conn = connect_project_db(project_root)?;
    let mut last_seen_id = 0_i64;
    let initial_events = fetch_recent_events(project_root, &conn, limit, status, node)?;

    if initial_events.is_empty() {
        println!("No events found");
    } else {
        for event in &initial_events {
            last_seen_id = last_seen_id.max(event["id"].as_i64().unwrap_or(0));
            println!("{}", serde_json::to_string(event)?);
        }
    }

    if follow {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            let new_events = fetch_events_after(project_root, &conn, status, node, last_seen_id)?;
            if new_events.is_empty() {
                continue;
            }
            for event in new_events {
                last_seen_id = last_seen_id.max(event["id"].as_i64().unwrap_or(last_seen_id));
                println!("{}", serde_json::to_string(&event)?);
            }
        }
    }
    Ok(())
}

fn fetch_recent_events(
    project_root: &PathBuf,
    conn: &rusqlite::Connection,
    limit: usize,
    status: Option<&str>,
    node: Option<&str>,
) -> Result<Vec<serde_json::Value>> {
    if let Some(status) = status {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, timestamp, source, event_type, event_data, status, attempts, error
            FROM event_log
            WHERE status = ?1
            ORDER BY timestamp DESC, id DESC
            LIMIT ?2
            "#,
        )?;
        let mut rows = stmt.query(params![status, limit as i64])?;
        collect_events(project_root, conn, &mut rows, node)
    } else {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, timestamp, source, event_type, event_data, status, attempts, error
            FROM event_log
            ORDER BY timestamp DESC, id DESC
            LIMIT ?1
            "#,
        )?;
        let mut rows = stmt.query(params![limit as i64])?;
        collect_events(project_root, conn, &mut rows, node)
    }
}

fn fetch_events_after(
    project_root: &PathBuf,
    conn: &rusqlite::Connection,
    status: Option<&str>,
    node: Option<&str>,
    after_id: i64,
) -> Result<Vec<serde_json::Value>> {
    if let Some(status) = status {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, timestamp, source, event_type, event_data, status, attempts, error
            FROM event_log
            WHERE status = ?1 AND id > ?2
            ORDER BY id ASC
            "#,
        )?;
        let mut rows = stmt.query(params![status, after_id])?;
        collect_events(project_root, conn, &mut rows, node)
    } else {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, timestamp, source, event_type, event_data, status, attempts, error
            FROM event_log
            WHERE id > ?1
            ORDER BY id ASC
            "#,
        )?;
        let mut rows = stmt.query(params![after_id])?;
        collect_events(project_root, conn, &mut rows, node)
    }
}

fn resolve_event_node_id(
    project_root: &PathBuf,
    conn: &rusqlite::Connection,
    event_data: &serde_json::Value,
) -> Result<Option<String>> {
    if let Some(node_id) = event_data.get("id").and_then(serde_json::Value::as_str) {
        if let Some(resolved) = conn
            .query_row(
                r#"
                SELECT id
                FROM graph_nodes
                WHERE id = ?1
                LIMIT 1
                "#,
                [node_id],
                |row| row.get(0),
            )
            .ok()
        {
            return Ok(Some(resolved));
        }
    }
    if let Some(path) = event_data.get("path").and_then(serde_json::Value::as_str) {
        let rel = std::path::Path::new(path)
            .strip_prefix(project_root)
            .ok()
            .unwrap_or_else(|| std::path::Path::new(path))
            .to_string_lossy()
            .replace('\\', "/");
        if let Some(resolved) = conn
            .query_row(
                r#"
                SELECT id
                FROM graph_nodes
                WHERE id = ?1 OR path = ?2
                LIMIT 1
                "#,
                (format!("file:{rel}"), rel.clone()),
                |row| row.get(0),
            )
            .ok()
        {
            return Ok(Some(resolved));
        }
    }
    Ok(None)
}

fn collect_events(
    project_root: &PathBuf,
    conn: &rusqlite::Connection,
    rows: &mut rusqlite::Rows<'_>,
    node_filter: Option<&str>,
) -> Result<Vec<serde_json::Value>> {
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let timestamp: i64 = row.get(1)?;
        let source: String = row.get(2)?;
        let event_type: String = row.get(3)?;
        let event_data_text: String = row.get(4)?;
        let status: String = row.get(5)?;
        let attempts: i64 = row.get(6)?;
        let error: Option<String> = row.get(7)?;
        let event_data = serde_json::from_str::<serde_json::Value>(&event_data_text)
            .unwrap_or_else(|_| serde_json::Value::String(event_data_text));
        let node_id = resolve_event_node_id(project_root, conn, &event_data)?;
        if let Some(filter) = node_filter {
            if node_id.as_deref() != Some(filter) {
                continue;
            }
        }
        events.push(json!({
            "id": id,
            "timestamp": timestamp,
            "source": source,
            "event_type": event_type,
            "event_data": event_data,
            "status": status,
            "attempts": attempts,
            "error": error,
            "node_id": node_id
        }));
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fetch_recent_events_returns_latest_rows_first() -> Result<()> {
        let temp = tempdir()?;
        init_project(temp.path())?;
        let conn = connect_project_db(temp.path())?;
        memorycore_core::append_event(
            &conn,
            "graph",
            "file_changed",
            &json!({"path": "src/main.rs"}),
        )?;
        memorycore_core::append_event(
            &conn,
            "daemon",
            "snapshot_created",
            &json!({"hash": "abc"}),
        )?;

        let events = fetch_recent_events(&temp.path().to_path_buf(), &conn, 2, None, None)?;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event_type"], "snapshot_created");
        assert_eq!(events[1]["event_type"], "file_changed");
        Ok(())
    }

    #[test]
    fn fetch_events_after_only_returns_newer_rows() -> Result<()> {
        let temp = tempdir()?;
        init_project(temp.path())?;
        let conn = connect_project_db(temp.path())?;
        memorycore_core::append_event(
            &conn,
            "graph",
            "file_changed",
            &json!({"path": "src/main.rs"}),
        )?;
        let first = memorycore_core::append_event(
            &conn,
            "daemon",
            "snapshot_created",
            &json!({"hash": "abc"}),
        )?;
        memorycore_core::append_event(
            &conn,
            "daemon",
            "git_commit_detected",
            &json!({"sha": "123"}),
        )?;

        let events = fetch_events_after(&temp.path().to_path_buf(), &conn, None, None, first)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event_type"], "git_commit_detected");
        Ok(())
    }

    #[test]
    fn fetch_recent_events_filters_by_node_id() -> Result<()> {
        let temp = tempdir()?;
        init_project(temp.path())?;
        let conn = connect_project_db(temp.path())?;
        std::fs::create_dir_all(temp.path().join("src"))?;
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;
        memorycore_graph::scan_file(&conn, temp.path(), &temp.path().join("src/main.rs"))?;
        memorycore_core::append_event(
            &conn,
            "memorycore-daemon",
            "snapshot_created",
            &json!({"hash": "abc"}),
        )?;

        let events = fetch_recent_events(
            &temp.path().to_path_buf(),
            &conn,
            10,
            None,
            Some("file:src/main.rs"),
        )?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["node_id"], "file:src/main.rs");
        Ok(())
    }
}

fn handle_plugins(command: PluginCommand, project_root: &PathBuf) -> Result<()> {
    match command {
        PluginCommand::Install { manifest } => {
            let manifest = absolutize(project_root, manifest);
            let plugin = memorycore_plugin_host::install_plugin(project_root, &manifest)?;
            println!(
                "Installed plugin {} {} ({})",
                plugin.id, plugin.version, plugin.manifest_path
            );
        }
        PluginCommand::List => {
            let plugins = memorycore_plugin_host::list_plugins(project_root)?;
            if plugins.is_empty() {
                println!("No plugins registered");
            } else {
                for plugin in plugins {
                    let state = if plugin.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    println!("{} {} {} {}", plugin.id, plugin.version, state, plugin.name);
                }
            }
        }
    }
    Ok(())
}

fn handle_skills(command: SkillCommand, project_root: &PathBuf) -> Result<()> {
    match command {
        SkillCommand::Register { path } => {
            let path = absolutize(project_root, path);
            let skill = memorycore_plugin_host::register_skill(project_root, &path)?;
            println!("Registered skill {} ({})", skill.id, skill.skill_path);
        }
        SkillCommand::List => {
            let skills = memorycore_plugin_host::list_skills(project_root)?;
            if skills.is_empty() {
                println!("No skills registered");
            } else {
                for skill in skills {
                    let state = if skill.enabled { "enabled" } else { "disabled" };
                    let description = skill.description.unwrap_or_default();
                    println!("{} {} {} {}", skill.id, state, skill.name, description);
                }
            }
        }
    }
    Ok(())
}

fn handle_adapters(command: AdapterCommand, project_root: &PathBuf) -> Result<()> {
    match command {
        AdapterCommand::Register {
            agent,
            name,
            session_dir,
            command,
        } => {
            let session_dir = session_dir.map(|path| absolutize(project_root, path));
            let adapter = memorycore_adapters::register_adapter(
                project_root,
                &agent,
                name.as_deref(),
                session_dir.as_deref(),
                command.as_deref(),
            )?;
            println!(
                "Registered adapter {} {} ({})",
                adapter.id, adapter.name, adapter.agent
            );
        }
        AdapterCommand::List => {
            let adapters = memorycore_adapters::list_adapters(project_root)?;
            if adapters.is_empty() {
                println!("No adapters registered");
            } else {
                for adapter in adapters {
                    let state = if adapter.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    println!(
                        "{} {} {} {} {}",
                        adapter.id,
                        adapter.agent,
                        state,
                        adapter.name,
                        adapter.session_dir.unwrap_or_default()
                    );
                }
            }
        }
    }
    Ok(())
}

fn handle_embeddings(command: EmbeddingCommand, project_root: &PathBuf) -> Result<()> {
    match command {
        EmbeddingCommand::Build => {
            let count = memorycore_embeddings::build_message_embeddings(project_root)?;
            println!("Built {count} embeddings");
        }
        EmbeddingCommand::List => {
            let rows = memorycore_embeddings::list_embeddings(project_root)?;
            if rows.is_empty() {
                println!("No embeddings registered");
            } else {
                for row in rows {
                    println!("{}", serde_json::to_string(&row)?);
                }
            }
        }
        EmbeddingCommand::Search { query, limit } => {
            let hits = memorycore_embeddings::search_embeddings(project_root, &query, limit)?;
            if hits.is_empty() {
                println!("No embedding hits");
            } else {
                for hit in hits {
                    println!("{}", serde_json::to_string(&hit)?);
                }
            }
        }
    }
    Ok(())
}

fn handle_memory(command: MemoryCommand, project_root: &PathBuf) -> Result<()> {
    let conn = connect_project_db(project_root)?;
    match command {
        MemoryCommand::Pin {
            name,
            summary,
            target,
        } => {
            let memory_id = memory_case_id(&name);
            let node = GraphNode {
                id: memory_id.clone(),
                kind: "MemoryCase".to_string(),
                name: name.clone(),
                path: None,
                span_start: None,
                span_end: None,
                hash: None,
                metadata: json!({
                    "summary": summary,
                    "target": target,
                    "source": "cli"
                }),
            };
            memorycore_graph::store::upsert_node(&conn, &node)?;

            let project_node = GraphNode {
                id: "project:root".to_string(),
                kind: "Project".to_string(),
                name: "root".to_string(),
                path: Some(project_root.to_string_lossy().to_string()),
                span_start: None,
                span_end: None,
                hash: None,
                metadata: json!({}),
            };
            memorycore_graph::store::upsert_node(&conn, &project_node)?;

            let contains = GraphEdge {
                id: format!("edge:{}:contains:{memory_id}", project_node.id),
                source_id: project_node.id.clone(),
                target_id: memory_id.clone(),
                kind: "contains".to_string(),
                weight: 1.0,
                confidence: 1.0,
                metadata: json!({}),
            };
            memorycore_graph::store::upsert_edge(&conn, &contains)?;

            if let Some(target) = target.as_deref() {
                if let Some(target_id) = resolve_graph_target(&conn, target)? {
                    let explains = GraphEdge {
                        id: format!("edge:{memory_id}:explains:{target_id}"),
                        source_id: memory_id.clone(),
                        target_id: target_id.clone(),
                        kind: "explains".to_string(),
                        weight: 1.0,
                        confidence: 1.0,
                        metadata: json!({ "target": target }),
                    };
                    memorycore_graph::store::upsert_edge(&conn, &explains)?;
                    println!("Pinned memory case {memory_id} -> {target_id}");
                } else {
                    println!("Pinned memory case {memory_id} (target not resolved: {target})");
                }
            } else {
                println!("Pinned memory case {memory_id}");
            }

            conn.execute(
                r#"
                INSERT OR REPLACE INTO memory_cases
                    (id, name, summary, target, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![&memory_id, &name, summary, target, now_unix(), now_unix()],
            )?;
            memorycore_core::append_event(
                &conn,
                "memorycore-cli",
                "memory_case_pinned",
                &json!({
                    "id": memory_id,
                    "name": name
                }),
            )?;
        }
        MemoryCommand::List => {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, name, COALESCE(summary, ''), COALESCE(target, '')
                FROM memory_cases
                ORDER BY updated_at DESC, name
                "#,
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            let mut count = 0;
            for row in rows {
                let (id, name, summary, target) = row?;
                count += 1;
                println!("{id} {name} {summary} {target}");
            }
            if count == 0 {
                println!("No memory cases registered");
            }
        }
    }
    Ok(())
}

fn handle_sessions(command: SessionCommand, project_root: &PathBuf) -> Result<()> {
    let conn = connect_project_db(project_root)?;
    match command {
        SessionCommand::Import { agent, id, path } => {
            let path = absolutize(project_root, path);
            let layout = memorycore_core::ProjectLayout::new(project_root);
            let agent_dir = layout.sessions.join(&agent);
            fs::create_dir_all(&agent_dir)?;
            let session_path = agent_dir.join(format!("{id}.jsonl.zst"));
            let session_rows = read_session_rows(&path)?;
            write_session_archive(&session_path, &session_rows)?;
            memorycore_daemon::import_session_archive(project_root, &conn, &session_path)?;
            memorycore_core::append_event(
                &conn,
                "memorycore-cli",
                "session_imported",
                &json!({
                    "id": id,
                    "agent": agent,
                    "messages": session_rows.len(),
                    "archive": session_path.to_string_lossy().to_string()
                }),
            )?;
            println!(
                "Imported session {} ({}) with {} messages",
                id,
                session_path.display(),
                session_rows.len()
            );
        }
        SessionCommand::List => {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, agent, started_at, COALESCE(ended_at, 0), message_count
                FROM sessions
                ORDER BY started_at DESC, id
                "#,
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?;
            let mut count = 0;
            for row in rows {
                let (id, agent, started_at, ended_at, message_count) = row?;
                count += 1;
                println!("{id} {agent} {started_at} {ended_at} {message_count}");
            }
            if count == 0 {
                println!("No sessions registered");
            }
        }
        SessionCommand::Show { id } => {
            let mut stmt = conn.prepare(
                r#"
                SELECT role, content, timestamp, metadata
                FROM messages
                WHERE session_id = ?1
                ORDER BY timestamp, id
                "#,
            )?;
            let rows = stmt.query_map([&id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            let mut count = 0;
            for row in rows {
                let (role, content, timestamp, metadata) = row?;
                count += 1;
                println!("{timestamp} {role} {content} {metadata}");
            }
            if count == 0 {
                println!("No messages found for session {id}");
            }
        }
    }
    Ok(())
}

fn memory_case_id(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    format!(
        "memory:{}-{}",
        if slug.is_empty() { "case" } else { slug },
        now_unix()
    )
}

fn resolve_graph_target(conn: &rusqlite::Connection, target: &str) -> Result<Option<String>> {
    let like = format!("%{target}%");
    let target_id: Option<String> = conn
        .query_row(
            r#"
            SELECT id
            FROM graph_nodes
            WHERE id LIKE ?1 OR name LIKE ?1 OR path LIKE ?1
            ORDER BY kind, path, name
            LIMIT 1
            "#,
            [like],
            |row| row.get(0),
        )
        .ok();
    Ok(target_id)
}

#[derive(Debug, Clone)]
struct SessionRow {
    role: String,
    content: String,
    timestamp: Option<i64>,
    metadata: Option<serde_json::Value>,
}

fn read_session_rows(path: &PathBuf) -> Result<Vec<SessionRow>> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    let content = if ext == "zst" {
        decompress_session_archive(path)?
    } else {
        fs::read_to_string(path)
            .with_context(|| format!("read session input {}", path.display()))?
    };
    let mut rows = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line)
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
        let metadata = value.get("metadata").cloned();
        rows.push(SessionRow {
            role,
            content,
            timestamp,
            metadata,
        });
    }
    Ok(rows)
}

fn write_session_archive(path: &PathBuf, rows: &[SessionRow]) -> Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open session archive {}", path.display()))?;
    let mut child = ProcessCommand::new("zstd")
        .arg("-c")
        .stdin(Stdio::piped())
        .stdout(file)
        .spawn()
        .context("spawn zstd compressor")?;
    let mut stdin = child.stdin.take().context("open zstd stdin")?;
    for row in rows {
        let value = json!({
            "role": row.role,
            "content": row.content,
            "timestamp": row.timestamp,
            "metadata": row.metadata
        });
        writeln!(stdin, "{}", value)?;
    }
    drop(stdin);
    let status = child.wait().context("wait for zstd compressor")?;
    if !status.success() {
        anyhow::bail!("zstd compressor failed with status {status}");
    }
    Ok(())
}

fn decompress_session_archive(path: &PathBuf) -> Result<String> {
    let output = ProcessCommand::new("zstd")
        .arg("-dc")
        .arg(path)
        .output()
        .with_context(|| format!("decompress session archive {}", path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "zstd decompressor failed with status {}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut content = String::new();
    content.push_str(&String::from_utf8_lossy(&output.stdout));
    Ok(content)
}

fn absolutize(project_root: &PathBuf, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    }
}
