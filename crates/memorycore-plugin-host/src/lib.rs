use anyhow::{bail, Context, Result};
use memorycore_core::{append_event, connect_project_db, now_unix};
use memorycore_graph::model::{GraphEdge, GraphNode};
use memorycore_graph::store::{upsert_edge, upsert_node};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: String,
    pub capabilities: Vec<String>,
    pub hooks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredPlugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: String,
    pub manifest_path: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredSkill {
    pub id: String,
    pub name: String,
    pub skill_path: String,
    pub description: Option<String>,
    pub enabled: bool,
}

const ALLOWED_CAPABILITIES: &[&str] = &[
    "read_project_files",
    "write_graph",
    "emit_events",
    "read_sessions",
    "write_sessions",
    "read_snapshots",
    "write_snapshots",
    "render_diagrams",
];

const ALLOWED_HOOKS: &[&str] = &[
    "onDaemonStart",
    "onDaemonStop",
    "onFileChanged",
    "onFileDeleted",
    "onSessionStarted",
    "onMessageCaptured",
    "onCommandCaptured",
    "onSnapshotCreated",
    "onGraphUpdated",
    "onGraphQuery",
    "onContextRequested",
    "onSearch",
];

pub fn load_manifest(path: &Path) -> Result<PluginManifest> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read plugin manifest {}", path.display()))?;
    let manifest: PluginManifest =
        serde_json::from_str(&text).context("parse plugin manifest JSON")?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn validate_manifest(manifest: &PluginManifest) -> Result<()> {
    if manifest.id.trim().is_empty() {
        bail!("plugin id cannot be empty");
    }
    if manifest.name.trim().is_empty() {
        bail!("plugin name cannot be empty");
    }
    if manifest.version.trim().is_empty() {
        bail!("plugin version cannot be empty");
    }
    if manifest.entry.trim().is_empty() {
        bail!("plugin entry cannot be empty");
    }
    for capability in &manifest.capabilities {
        if !ALLOWED_CAPABILITIES.contains(&capability.as_str()) {
            bail!("plugin capability {capability} is not allowed");
        }
    }
    for hook in &manifest.hooks {
        if !ALLOWED_HOOKS.contains(&hook.as_str()) {
            bail!("plugin hook {hook} is not allowed");
        }
    }
    Ok(())
}

pub fn install_plugin(project_root: &Path, manifest_path: &Path) -> Result<RegisteredPlugin> {
    let conn = connect_project_db(project_root)?;
    let manifest = load_manifest(manifest_path)?;
    let registered = upsert_plugin(&conn, manifest_path, &manifest)?;
    upsert_project_root_graph(&conn, project_root)?;
    upsert_plugin_graph(&conn, &registered)?;
    append_event(
        &conn,
        "memorycore-plugin-host",
        "plugin_installed",
        &serde_json::json!({
            "id": registered.id,
            "name": registered.name,
            "version": registered.version,
            "manifest_path": registered.manifest_path
        }),
    )?;
    Ok(registered)
}

pub fn list_plugins(project_root: &Path) -> Result<Vec<RegisteredPlugin>> {
    let conn = connect_project_db(project_root)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, name, version, entry, manifest_path, enabled
        FROM plugins
        ORDER BY id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(RegisteredPlugin {
            id: row.get(0)?,
            name: row.get(1)?,
            version: row.get(2)?,
            entry: row.get(3)?,
            manifest_path: row.get(4)?,
            enabled: row.get::<_, i64>(5)? == 1,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("list plugins")
}

pub fn register_skill(project_root: &Path, skill_path: &Path) -> Result<RegisteredSkill> {
    let conn = connect_project_db(project_root)?;
    let skill_md = if skill_path.is_dir() {
        skill_path.join("SKILL.md")
    } else {
        skill_path.to_path_buf()
    };
    if !skill_md.is_file() {
        bail!("skill path must point to a SKILL.md file or directory containing SKILL.md");
    }
    let text =
        fs::read_to_string(&skill_md).with_context(|| format!("read {}", skill_md.display()))?;
    let name = skill_md
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "skill".to_string());
    let id = normalize_id(&name);
    let description = first_description_line(&text);
    let now = now_unix();
    conn.execute(
        r#"
        INSERT INTO skills
            (id, name, skill_path, description, enabled, registered_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)
        ON CONFLICT(id) DO UPDATE SET
            name=excluded.name,
            skill_path=excluded.skill_path,
            description=excluded.description,
            enabled=excluded.enabled,
            updated_at=excluded.updated_at
        "#,
        params![
            id,
            name,
            skill_md.to_string_lossy().to_string(),
            description,
            now
        ],
    )?;
    let registered = RegisteredSkill {
        id,
        name,
        skill_path: skill_md.to_string_lossy().to_string(),
        description,
        enabled: true,
    };
    upsert_project_root_graph(&conn, project_root)?;
    upsert_skill_graph(&conn, &registered)?;
    append_event(
        &conn,
        "memorycore-plugin-host",
        "skill_registered",
        &serde_json::json!({
            "id": registered.id,
            "name": registered.name,
            "skill_path": registered.skill_path
        }),
    )?;
    Ok(registered)
}

pub fn list_skills(project_root: &Path) -> Result<Vec<RegisteredSkill>> {
    let conn = connect_project_db(project_root)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, name, skill_path, description, enabled
        FROM skills
        ORDER BY id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(RegisteredSkill {
            id: row.get(0)?,
            name: row.get(1)?,
            skill_path: row.get(2)?,
            description: row.get(3)?,
            enabled: row.get::<_, i64>(4)? == 1,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("list skills")
}

pub fn disable_plugin_graph(conn: &Connection, manifest_path: &Path) -> Result<()> {
    let plugin_id: Option<String> = conn
        .query_row(
            "SELECT id FROM plugins WHERE manifest_path = ?1 LIMIT 1",
            [manifest_path.to_string_lossy().to_string()],
            |row| row.get(0),
        )
        .ok();
    let Some(plugin_id) = plugin_id else {
        return Ok(());
    };
    let node_id = format!("plugin:{plugin_id}");
    conn.execute(
        r#"
        INSERT INTO graph_nodes
            (id, kind, name, path, span_start, span_end, hash, metadata, updated_at)
        SELECT
            id,
            kind,
            name,
            path,
            span_start,
            span_end,
            hash,
            json_set(COALESCE(metadata, '{}'), '$.enabled', 0),
            ?2
        FROM graph_nodes
        WHERE id = ?1
        ON CONFLICT(id) DO UPDATE SET
            metadata=excluded.metadata,
            updated_at=excluded.updated_at
        "#,
        params![node_id, now_unix()],
    )?;
    Ok(())
}

pub fn disable_skill_graph(conn: &Connection, skill_path: &Path) -> Result<()> {
    let skill_id: Option<String> = conn
        .query_row(
            "SELECT id FROM skills WHERE skill_path = ?1 LIMIT 1",
            [skill_path.to_string_lossy().to_string()],
            |row| row.get(0),
        )
        .ok();
    let Some(skill_id) = skill_id else {
        return Ok(());
    };
    let node_id = format!("skill:{skill_id}");
    conn.execute(
        r#"
        INSERT INTO graph_nodes
            (id, kind, name, path, span_start, span_end, hash, metadata, updated_at)
        SELECT
            id,
            kind,
            name,
            path,
            span_start,
            span_end,
            hash,
            json_set(COALESCE(metadata, '{}'), '$.enabled', 0),
            ?2
        FROM graph_nodes
        WHERE id = ?1
        ON CONFLICT(id) DO UPDATE SET
            metadata=excluded.metadata,
            updated_at=excluded.updated_at
        "#,
        params![node_id, now_unix()],
    )?;
    Ok(())
}

fn upsert_plugin(
    conn: &Connection,
    manifest_path: &Path,
    manifest: &PluginManifest,
) -> Result<RegisteredPlugin> {
    let now = now_unix();
    conn.execute(
        r#"
        INSERT INTO plugins
            (id, name, version, entry, manifest_path, capabilities, hooks, enabled, installed_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8)
        ON CONFLICT(id) DO UPDATE SET
            name=excluded.name,
            version=excluded.version,
            entry=excluded.entry,
            manifest_path=excluded.manifest_path,
            capabilities=excluded.capabilities,
            hooks=excluded.hooks,
            enabled=excluded.enabled,
            updated_at=excluded.updated_at
        "#,
        params![
            manifest.id,
            manifest.name,
            manifest.version,
            manifest.entry,
            manifest_path.to_string_lossy().to_string(),
            serde_json::to_string(&manifest.capabilities)?,
            serde_json::to_string(&manifest.hooks)?,
            now
        ],
    )?;
    Ok(RegisteredPlugin {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        entry: manifest.entry.clone(),
        manifest_path: manifest_path.to_string_lossy().to_string(),
        enabled: true,
    })
}

fn upsert_plugin_graph(conn: &Connection, plugin: &RegisteredPlugin) -> Result<()> {
    let plugin_node_id = format!("plugin:{}", plugin.id);
    upsert_node(
        conn,
        &GraphNode {
            id: plugin_node_id.clone(),
            kind: "Plugin".to_string(),
            name: plugin.name.clone(),
            path: Some(plugin.manifest_path.clone()),
            span_start: None,
            span_end: None,
            hash: Some(plugin.version.clone()),
            metadata: serde_json::json!({
                "entry": plugin.entry,
                "enabled": plugin.enabled,
                "version": plugin.version,
            }),
        },
    )?;
    upsert_edge(
        conn,
        &GraphEdge {
            id: format!("edge:project:root:contains:{plugin_node_id}"),
            source_id: "project:root".to_string(),
            target_id: plugin_node_id,
            kind: "contains".to_string(),
            weight: 1.0,
            confidence: 1.0,
            metadata: serde_json::json!({
                "manifest_path": plugin.manifest_path
            }),
        },
    )?;
    Ok(())
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

fn upsert_skill_graph(conn: &Connection, skill: &RegisteredSkill) -> Result<()> {
    let skill_node_id = format!("skill:{}", skill.id);
    upsert_node(
        conn,
        &GraphNode {
            id: skill_node_id.clone(),
            kind: "Skill".to_string(),
            name: skill.name.clone(),
            path: Some(skill.skill_path.clone()),
            span_start: None,
            span_end: None,
            hash: skill
                .description
                .as_ref()
                .map(|desc| format!("{:x}", Sha256::digest(desc.as_bytes()))),
            metadata: serde_json::json!({
                "description": skill.description,
                "enabled": skill.enabled,
            }),
        },
    )?;
    upsert_edge(
        conn,
        &GraphEdge {
            id: format!("edge:project:root:contains:{skill_node_id}"),
            source_id: "project:root".to_string(),
            target_id: skill_node_id,
            kind: "contains".to_string(),
            weight: 1.0,
            confidence: 1.0,
            metadata: serde_json::json!({
                "skill_path": skill.skill_path
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

fn first_description_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
}

pub fn plugin_manifest_path(plugin_dir: &Path) -> PathBuf {
    plugin_dir.join("plugin.json")
}
