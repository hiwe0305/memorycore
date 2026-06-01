use anyhow::{bail, Result};
use memorycore_core::{append_event, connect_project_db, now_unix};
use memorycore_graph::model::{GraphEdge, GraphNode};
use memorycore_graph::store::{upsert_edge, upsert_node};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub trait AgentAdapter {
    fn agent_name(&self) -> &str;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredAdapter {
    pub id: String,
    pub agent: String,
    pub name: String,
    pub session_dir: Option<String>,
    pub command: Option<String>,
    pub enabled: bool,
}

pub fn register_adapter(
    project_root: &Path,
    agent: &str,
    name: Option<&str>,
    session_dir: Option<&Path>,
    command: Option<&str>,
) -> Result<RegisteredAdapter> {
    let agent = agent.trim();
    if agent.is_empty() {
        bail!("adapter agent cannot be empty");
    }
    let adapter_name = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(agent);
    let id = normalize_id(agent);
    if id.is_empty() {
        bail!("adapter id cannot be empty");
    }
    let session_dir = session_dir.map(|path| path.to_string_lossy().to_string());
    let command = command
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let conn = connect_project_db(project_root)?;
    let now = now_unix();
    conn.execute(
        r#"
        INSERT INTO adapters
            (id, agent, name, session_dir, command, enabled, registered_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)
        ON CONFLICT(id) DO UPDATE SET
            agent=excluded.agent,
            name=excluded.name,
            session_dir=excluded.session_dir,
            command=excluded.command,
            enabled=excluded.enabled,
            updated_at=excluded.updated_at
        "#,
        params![id, agent, adapter_name, session_dir, command, now],
    )?;
    let adapter = RegisteredAdapter {
        id,
        agent: agent.to_string(),
        name: adapter_name.to_string(),
        session_dir,
        command,
        enabled: true,
    };
    upsert_project_root_graph(&conn, project_root)?;
    upsert_adapter_graph(&conn, &adapter)?;
    append_event(
        &conn,
        "memorycore-adapters",
        "adapter_registered",
        &serde_json::json!({
            "id": adapter.id,
            "agent": adapter.agent,
            "name": adapter.name,
            "session_dir": adapter.session_dir,
            "command": adapter.command
        }),
    )?;
    Ok(adapter)
}

pub fn list_adapters(project_root: &Path) -> Result<Vec<RegisteredAdapter>> {
    let conn = connect_project_db(project_root)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, agent, name, session_dir, command, enabled
        FROM adapters
        ORDER BY agent, id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(RegisteredAdapter {
            id: row.get(0)?,
            agent: row.get(1)?,
            name: row.get(2)?,
            session_dir: row.get(3)?,
            command: row.get(4)?,
            enabled: row.get::<_, i64>(5)? == 1,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn upsert_project_root_graph(conn: &Connection, project_root: &Path) -> Result<()> {
    let project_name = project_root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| project_root.display().to_string());
    upsert_node(
        conn,
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
    )
}

fn upsert_adapter_graph(conn: &Connection, adapter: &RegisteredAdapter) -> Result<()> {
    let adapter_node_id = format!("adapter:{}", adapter.id);
    upsert_node(
        conn,
        &GraphNode {
            id: adapter_node_id.clone(),
            kind: "Adapter".to_string(),
            name: adapter.name.clone(),
            path: adapter.session_dir.clone(),
            span_start: None,
            span_end: None,
            hash: None,
            metadata: serde_json::json!({
                "agent": adapter.agent,
                "command": adapter.command,
                "enabled": adapter.enabled,
                "session_dir": adapter.session_dir,
            }),
        },
    )?;
    upsert_edge(
        conn,
        &GraphEdge {
            id: format!("edge:project:root:contains:{adapter_node_id}"),
            source_id: "project:root".to_string(),
            target_id: adapter_node_id,
            kind: "contains".to_string(),
            weight: 1.0,
            confidence: 1.0,
            metadata: serde_json::json!({
                "agent": adapter.agent,
                "session_dir": adapter.session_dir
            }),
        },
    )?;
    Ok(())
}

fn normalize_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use memorycore_core::init_project;
    use tempfile::tempdir;

    #[test]
    fn registers_adapter_in_sqlite_and_graph() -> Result<()> {
        let temp = tempdir()?;
        init_project(temp.path())?;
        let session_dir = temp.path().join(".memorycore/sessions/codex");
        let adapter = register_adapter(
            temp.path(),
            "codex",
            Some("Codex CLI"),
            Some(&session_dir),
            Some("codex"),
        )?;
        assert_eq!(adapter.id, "codex");

        let conn = connect_project_db(temp.path())?;
        let kind: String = conn.query_row(
            "SELECT kind FROM graph_nodes WHERE id = 'adapter:codex'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(kind, "Adapter");

        let edge_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE source_id = 'project:root' AND target_id = 'adapter:codex'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(edge_count, 1);
        Ok(())
    }
}
