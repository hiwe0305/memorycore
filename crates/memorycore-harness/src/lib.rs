//! MemoryCore Harness — agent lifecycle, skill dispatch, and memory bridge.
//!
//! The harness is an event-driven orchestration layer that:
//! - Tracks agent activity and status
//! - Routes skill execution requests
//! - Bridges agent actions into the memory graph
//! - Exposes agent context via MCP tools

use anyhow::{Context, Result};
use memorycore_core::{append_event, connect_project_db, now_unix};
use memorycore_graph::model::{GraphEdge, GraphNode};
use memorycore_graph::store::{upsert_edge, upsert_node};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub agent_name: String,
    pub status: String,         // active, idle, error, offline
    pub last_seen: i64,
    pub session_dir: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityRecord {
    pub id: i64,
    pub agent_name: String,
    pub activity_type: String,
    pub target: Option<String>,
    pub summary: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRunRecord {
    pub id: i64,
    pub skill_id: String,
    pub agent_name: Option<String>,
    pub inputs: serde_json::Value,
    pub output_summary: Option<String>,
    pub success: bool,
    pub duration_ms: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessStatus {
    pub running: bool,
    pub agent_count: usize,
    pub activity_count: usize,
    pub skill_run_count: usize,
    pub uptime_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub skill_path: String,
    pub description: Option<String>,
    pub entry: Option<String>,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Agent tracking
// ---------------------------------------------------------------------------

/// Record an agent activity in the event log and agent_activity table.
pub fn record_activity(
    project_root: &Path,
    agent_name: &str,
    activity_type: &str,
    target: Option<&str>,
    summary: Option<&str>,
    metadata: serde_json::Value,
) -> Result<i64> {
    let conn = connect_project_db(project_root)?;
    let now = now_unix();

    conn.execute(
        r#"
        INSERT INTO agent_activity
            (agent_name, activity_type, target, summary, metadata, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![agent_name, activity_type, target, summary, metadata.to_string(), now],
    )?;
    let id = conn.last_insert_rowid();

    // Upsert project root first
    let project_name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
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

    // Upsert agent in graph
    let agent_node_id = format!("agent:{agent_name}");
    upsert_node(
        &conn,
        &GraphNode {
            id: agent_node_id.clone(),
            kind: "Agent".to_string(),
            name: agent_name.to_string(),
            path: None,
            span_start: None,
            span_end: None,
            hash: None,
            metadata: serde_json::json!({
                "status": "active",
                "last_seen": now,
                "last_activity": activity_type,
                "activity_id": id,
            }),
        },
    )?;

    // Connect agent to project root
    upsert_edge(
        &conn,
        &GraphEdge {
            id: format!("edge:project:root:contains:{agent_node_id}"),
            source_id: "project:root".to_string(),
            target_id: agent_node_id,
            kind: "contains".to_string(),
            weight: 1.0,
            confidence: 1.0,
            metadata: serde_json::json!({}),
        },
    )?;

    append_event(
        &conn,
        "memorycore-harness",
        &format!("agent_{activity_type}"),
        &serde_json::json!({
            "agent": agent_name,
            "activity_type": activity_type,
            "target": target,
            "summary": summary,
            "activity_id": id,
        }),
    )?;

    Ok(id)
}

/// List recent agent activity, optionally filtered by agent name.
pub fn list_activity(
    project_root: &Path,
    agent_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<ActivityRecord>> {
    let conn = connect_project_db(project_root)?;

    let (sql, params_list): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(agent) = agent_filter {
        (
            r#"
            SELECT id, agent_name, activity_type, target, summary, metadata, created_at
            FROM agent_activity
            WHERE agent_name = ?1
            ORDER BY created_at DESC, id DESC
            LIMIT ?2
            "#.to_string(),
            vec![Box::new(agent.to_string()), Box::new(limit as i64)],
        )
    } else {
        (
            r#"
            SELECT id, agent_name, activity_type, target, summary, metadata, created_at
            FROM agent_activity
            ORDER BY created_at DESC, id DESC
            LIMIT ?1
            "#.to_string(),
            vec![Box::new(limit as i64)],
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params_list.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        let meta_text: String = row.get(5)?;
        Ok(ActivityRecord {
            id: row.get(0)?,
            agent_name: row.get(1)?,
            activity_type: row.get(2)?,
            target: row.get(3)?,
            summary: row.get(4)?,
            metadata: serde_json::from_str(&meta_text).unwrap_or_default(),
            created_at: row.get(6)?,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// List all unique agents seen in activity log, with their latest status.
pub fn list_agents(project_root: &Path) -> Result<Vec<AgentRecord>> {
    let conn = connect_project_db(project_root)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT a.agent_name, a.activity_type, a.created_at,
            COALESCE(ad.session_dir, '') as session_dir
        FROM agent_activity a
        LEFT JOIN adapters ad ON ad.agent = a.agent_name
        WHERE a.id IN (
            SELECT MAX(id) FROM agent_activity GROUP BY agent_name
        )
        ORDER BY a.created_at DESC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AgentRecord {
            agent_name: row.get(0)?,
            status: row.get::<_, String>(1)?.replace("agent_", ""),
            last_seen: row.get(2)?,
            session_dir: Some(row.get::<_, String>(3)?).filter(|s| !s.is_empty()),
            metadata: serde_json::json!({}),
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Skill execution
// ---------------------------------------------------------------------------

/// Execute a skill by id. The skill must be registered and have an entry point.
/// Falls back to returning the SKILL.md content if no entry point exists.
pub fn execute_skill(
    project_root: &Path,
    skill_id: &str,
    agent_name: Option<&str>,
    inputs: serde_json::Value,
) -> Result<SkillRunRecord> {
    let conn = connect_project_db(project_root)?;
    let now = now_unix();
    let start = Instant::now();

    // Look up skill
    let skill_row = conn.query_row(
        r#"
        SELECT id, name, skill_path, description, enabled
        FROM skills
        WHERE id = ?1 OR name = ?1
        LIMIT 1
        "#,
        [skill_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, bool>(4)?,
            ))
        },
    );

    let (sid, _sname, spath, _sdesc, enabled) = match skill_row {
        Ok(r) => r,
        Err(_) => {
            // Register the skill run as failed
            let duration = start.elapsed().as_millis() as i64;
            let record = SkillRunRecord {
                id: 0,
                skill_id: skill_id.to_string(),
                agent_name: agent_name.map(String::from),
                inputs,
                output_summary: Some(format!("skill not found: {skill_id}")),
                success: false,
                duration_ms: Some(duration),
                created_at: now,
            };
            log_skill_run(&conn, &record)?;
            return Ok(record);
        }
    };

    if !enabled {
        let duration = start.elapsed().as_millis() as i64;
        let record = SkillRunRecord {
            id: 0,
            skill_id: sid,
            agent_name: agent_name.map(String::from),
            inputs,
            output_summary: Some("skill is disabled".to_string()),
            success: false,
            duration_ms: Some(duration),
            created_at: now,
        };
        log_skill_run(&conn, &record)?;
        return Ok(record);
    }

    // Find entry point: look for execute.sh alongside SKILL.md
    let skill_dir = std::path::Path::new(&spath).parent()
        .unwrap_or_else(|| std::path::Path::new(&spath));
    let entry_sh = skill_dir.join("execute.sh");

    let (success, output_summary) = if entry_sh.is_file() {
        // Try to execute the skill entry point
        match execute_skill_entry(project_root, &sid, &_sname, &entry_sh, &inputs) {
            Ok(output) => {
                let summary = output.chars().take(200).collect::<String>();
                (true, Some(summary))
            }
            Err(e) => {
                (false, Some(format!("execution error: {e}")))
            }
        }
    } else {
        // No entry point — just return SKILL.md content as output
        let content = match std::fs::read_to_string(&spath) {
            Ok(c) => c,
            Err(_) => format!("skill file not found: {}", spath),
        };
        (true, Some(content.chars().take(500).collect::<String>()))
    };

    let duration = start.elapsed().as_millis() as i64;
    let record = SkillRunRecord {
        id: 0,
        skill_id: sid,
        agent_name: agent_name.map(String::from),
        inputs,
        output_summary,
        success,
        duration_ms: Some(duration),
        created_at: now,
    };
    log_skill_run(&conn, &record)?;

    // Record as agent activity if agent_name is provided
    if let Some(agent) = agent_name {
        let _ = record_activity(
            project_root,
            agent,
            "skill_executed",
            Some(&record.skill_id),
            Some(&format!("skill run {} success={}", record.skill_id, record.success)),
            serde_json::json!({
                "skill_id": record.skill_id,
                "duration_ms": duration,
                "success": record.success,
            }),
        );
    }

    Ok(record)
}

fn execute_skill_entry(
    project_root: &Path,
    skill_id: &str,
    skill_name: &str,
    entry_path: &Path,
    inputs: &serde_json::Value,
) -> Result<String> {
    use std::process::Command;

    let input_json = serde_json::to_string(inputs)?;

    let output = Command::new(entry_path)
        .arg("--project-root")
        .arg(project_root)
        .arg("--skill-id")
        .arg(skill_id)
        .arg("--skill-name")
        .arg(skill_name)
        .arg("--inputs")
        .arg(&input_json)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .with_context(|| format!("execute skill {}", entry_path.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        anyhow::bail!("skill exited with code {:?}\nstderr: {}", output.status.code(), stderr);
    }

    if !stderr.is_empty() {
        eprintln!("skill stderr: {stderr}");
    }

    Ok(stdout)
}

fn log_skill_run(conn: &Connection, record: &SkillRunRecord) -> Result<i64> {
    conn.execute(
        r#"
        INSERT INTO skill_runs
            (skill_id, agent_name, inputs, output_summary, success, duration_ms, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
        params![
            record.skill_id,
            record.agent_name,
            record.inputs.to_string(),
            record.output_summary,
            record.success as i64,
            record.duration_ms,
            record.created_at,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// List skill runs, optionally filtered by skill_id.
pub fn list_skill_runs(
    project_root: &Path,
    skill_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<SkillRunRecord>> {
    let conn = connect_project_db(project_root)?;

    if let Some(sid) = skill_filter {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, skill_id, agent_name, inputs, output_summary, success, duration_ms, created_at
            FROM skill_runs
            WHERE skill_id = ?1
            ORDER BY created_at DESC, id DESC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![sid, limit as i64], |row| {
            row_to_skill_run(row)
        })?;
        let out: Vec<SkillRunRecord> = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    } else {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, skill_id, agent_name, inputs, output_summary, success, duration_ms, created_at
            FROM skill_runs
            ORDER BY created_at DESC, id DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            row_to_skill_run(row)
        })?;
        let out: Vec<SkillRunRecord> = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    }
}

fn row_to_skill_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<SkillRunRecord> {
    let inputs_text: String = row.get(3)?;
    Ok(SkillRunRecord {
        id: row.get(0)?,
        skill_id: row.get(1)?,
        agent_name: row.get(2)?,
        inputs: serde_json::from_str(&inputs_text).unwrap_or_default(),
        output_summary: row.get(4)?,
        success: row.get::<_, i64>(5)? == 1,
        duration_ms: row.get(6)?,
        created_at: row.get(7)?,
    })
}

/// Get harness status summary.
pub fn status(project_root: &Path) -> Result<HarnessStatus> {
    let conn = connect_project_db(project_root)?;
    let agent_count: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT agent_name) FROM agent_activity", [], |row| row.get(0)
    ).unwrap_or(0);
    let activity_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM agent_activity", [], |row| row.get(0)
    ).unwrap_or(0);
    let skill_run_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skill_runs", [], |row| row.get(0)
    ).unwrap_or(0);

    Ok(HarnessStatus {
        running: true,
        agent_count: agent_count as usize,
        activity_count: activity_count as usize,
        skill_run_count: skill_run_count as usize,
        uptime_seconds: 0,  // set by caller if needed
    })
}

// ---------------------------------------------------------------------------
// Agent auto-discovery
// ---------------------------------------------------------------------------


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredAgent {
    pub name: String,
    pub agent_type: String,
    pub session_dir: Option<String>,
    pub command: Option<String>,
    pub config_path: Option<String>,
}

/// Auto-discover agents on this machine and register adapters for them.
pub fn auto_connect(project_root: &Path) -> Result<Vec<DiscoveredAgent>> {
    let mut discovered = Vec::new();

    // 1. Codex CLI
    if let Some(home) = dirs_home() {
        let codex_config = home.join(".codex").join("config.toml");
        if codex_config.is_file() {
            let session_dir = home.join(".codex").join("archived_sessions");
            if session_dir.is_dir() {
                discovered.push(DiscoveredAgent {
                    name: "codex".to_string(),
                    agent_type: "codex".to_string(),
                    session_dir: Some(session_dir.to_string_lossy().to_string()),
                    command: Some("codex".to_string()),
                    config_path: Some(codex_config.to_string_lossy().to_string()),
                });
            }
        }
    }

    // 2. Claude Desktop
    if let Some(home) = dirs_home() {
        let claude_config = home.join(".claude.json");
        if claude_config.is_file() {
            discovered.push(DiscoveredAgent {
                name: "claude".to_string(),
                agent_type: "claude-desktop".to_string(),
                session_dir: None,
                command: None,
                config_path: Some(claude_config.to_string_lossy().to_string()),
            });
        }
    }

    // 3. Cursor
    if let Some(home) = dirs_home() {
        let cursor_dir = home.join(".cursor");
        if cursor_dir.is_dir() {
            let _session_dir = cursor_dir.join("machineId");
            discovered.push(DiscoveredAgent {
                name: "cursor".to_string(),
                agent_type: "cursor".to_string(),
                session_dir: None,
                command: None,
                config_path: Some(cursor_dir.to_string_lossy().to_string()),
            });
        }
    }

    // 4. Project-local agents — look for common agent dirs
    let agent_hints = ["mcp", "memory-service", "nemoclaw", "telegram-adapter", "zalo-adapter"];
    for hint in &agent_hints {
        let candidate = project_root.join(hint);
        if candidate.is_dir() {
            discovered.push(DiscoveredAgent {
                name: hint.to_string(),
                agent_type: "project-agent".to_string(),
                session_dir: None,
                command: None,
                config_path: Some(candidate.to_string_lossy().to_string()),
            });
        }
    }

    // Register each discovered agent as an adapter
    let _conn = connect_project_db(project_root)?;
    for agent in &discovered {
        let name = Some(agent.name.as_str());
        let session_dir = agent
            .session_dir
            .as_ref()
            .map(|s| std::path::Path::new(s));
        let command = Some(agent.agent_type.as_str());
        let _ = memorycore_adapters::register_adapter(
            project_root,
            &agent.name,
            name,
            session_dir,
            command,
        );
    }

    // Generate MCP config files for each discovered agent
    generate_mcp_configs(project_root, &discovered)?;

    Ok(discovered)
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

fn generate_mcp_configs(project_root: &Path, agents: &[DiscoveredAgent]) -> Result<()> {
    let mcp_dir = project_root.join(".memorycore").join("mcp");
    std::fs::create_dir_all(&mcp_dir)?;

    for agent in agents {
        // Write per-agent MCP config so the agent can connect to MemoryCore
        let config = serde_json::json!({
            "mcpServers": {
                "memorycore": {
                    "command": "memorycore",
                    "args": ["--project-root", project_root.to_string_lossy().to_string(), "mcp", "serve"],
                    "env": {},
                    "disabled": false,
                    "autoApprove": []
                }
            }
        });
        let config_path = mcp_dir.join(format!("{}-mcp.json", agent.name));
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&config)?,
        )?;
    }

    // Also write a combined MCP config for all agents
    let mut all_servers = serde_json::Map::new();
    for agent in agents {
        all_servers.insert(
            format!("memorycore-{}", agent.name),
            serde_json::json!({
                "command": "memorycore",
                "args": ["--project-root", project_root.to_string_lossy().to_string(), "mcp", "serve"],
                "env": {},
                "disabled": false,
                "autoApprove": []
            }),
        );
    }
    let combined = serde_json::json!({ "mcpServers": all_servers });
    std::fs::write(
        mcp_dir.join("all-agents.json"),
        serde_json::to_string_pretty(&combined)?,
    )?;

    Ok(())
}


#[cfg(test)]
mod tests {
    use serde_json::json;
    use super::*;
    use memorycore_core::init_project;
    use tempfile::tempdir;

    #[test]
    fn records_and_lists_agent_activity() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");

        let id1 = record_activity(
            temp.path(), "codex", "search", Some("auth.rs"), Some("searched for auth"), json!({"query": "auth"})
        ).expect("record activity");
        assert!(id1 > 0);

        let id2 = record_activity(
            temp.path(), "cursor", "analyze", Some("main.rs"), Some("analyzed main"), json!({"depth": 2})
        ).expect("record activity");
        assert!(id2 > 0);

        let all = list_activity(temp.path(), None, 10).expect("list all");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].agent_name, "cursor");

        let filtered = list_activity(temp.path(), Some("codex"), 10).expect("list codex");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].agent_name, "codex");
    }

    #[test]
    fn executes_skill_without_entry_returns_skill_md() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");

        // Register a simple skill (no entry point)
        let skill_dir = temp.path().join("skills").join("test-skill");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(skill_dir.join("SKILL.md"), "# Test Skill\n\nSimple test.").expect("write skill");
        memorycore_plugin_host::register_skill(temp.path(), &skill_dir).expect("register");

        let result = execute_skill(temp.path(), "test-skill", None, json!({})).expect("execute");
        assert!(result.success);
        assert!(result.output_summary.as_deref().unwrap_or("").contains("Test Skill"));
    }

    #[test]
    fn skill_run_fails_for_unknown_skill() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");

        let result = execute_skill(temp.path(), "nonexistent", None, json!({})).expect("execute");
        assert!(!result.success);
        assert!(result.output_summary.as_deref().unwrap_or("").contains("not found"));
    }
}
