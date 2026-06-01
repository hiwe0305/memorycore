use memorycore_core::init_project;
use memorycore_graph::query::graph_target_json_depth;
use memorycore_plugin_host::{
    disable_plugin_graph, disable_skill_graph, install_plugin, list_plugins, list_skills,
    register_skill,
};
use rusqlite::Connection;
use std::fs;
use tempfile::tempdir;

#[test]
fn installs_valid_plugin_manifest() {
    let temp = tempdir().expect("create temp project");
    init_project(temp.path()).expect("init project");
    let plugin_dir = temp.path().join("typescript-analyzer");
    fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    let manifest_path = plugin_dir.join("plugin.json");
    fs::write(
        &manifest_path,
        r#"{
          "id": "memorycore.typescript-analyzer",
          "name": "TypeScript Analyzer",
          "version": "0.1.0",
          "entry": "dist/index.js",
          "capabilities": ["read_project_files", "write_graph", "emit_events"],
          "hooks": ["onFileChanged", "onGraphQuery"]
        }"#,
    )
    .expect("write manifest");

    let plugin = install_plugin(temp.path(), &manifest_path).expect("install plugin");
    assert_eq!(plugin.id, "memorycore.typescript-analyzer");

    let plugins = list_plugins(temp.path()).expect("list plugins");
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].name, "TypeScript Analyzer");

    let conn = Connection::open(temp.path().join(".memorycore/index.db")).expect("open db");
    let plugin_node_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE id = ?1 AND kind = 'Plugin'",
            ["plugin:memorycore.typescript-analyzer"],
            |row| row.get(0),
        )
        .expect("query plugin node");
    assert_eq!(plugin_node_count, 1);
    let plugin_edge_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE source_id = 'project:root' AND target_id = ?1 AND kind = 'contains'",
            ["plugin:memorycore.typescript-analyzer"],
            |row| row.get(0),
        )
        .expect("query plugin edge");
    assert_eq!(plugin_edge_count, 1);

    let graph = graph_target_json_depth(&conn, "plugin:memorycore.typescript-analyzer", 1)
        .expect("graph query");
    assert!(graph.contains("\"kind\": \"Plugin\""));
    assert!(graph.contains("\"id\": \"project:root\""));

    disable_plugin_graph(&conn, &manifest_path).expect("disable plugin graph");
    let plugin_enabled: i64 = conn
        .query_row(
            "SELECT json_extract(metadata, '$.enabled') FROM graph_nodes WHERE id = ?1",
            ["plugin:memorycore.typescript-analyzer"],
            |row| row.get(0),
        )
        .expect("query plugin enabled");
    assert_eq!(plugin_enabled, 0);
}

#[test]
fn disables_plugin_graph_state() {
    let temp = tempdir().expect("create temp project");
    init_project(temp.path()).expect("init project");
    let plugin_dir = temp.path().join("typescript-analyzer");
    fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    let manifest_path = plugin_dir.join("plugin.json");
    fs::write(
        &manifest_path,
        r#"{
          "id": "memorycore.typescript-analyzer",
          "name": "TypeScript Analyzer",
          "version": "0.1.0",
          "entry": "dist/index.js",
          "capabilities": ["read_project_files", "write_graph", "emit_events"],
          "hooks": ["onFileChanged", "onGraphQuery"]
        }"#,
    )
    .expect("write manifest");

    install_plugin(temp.path(), &manifest_path).expect("install plugin");
    let conn = Connection::open(temp.path().join(".memorycore/index.db")).expect("open db");
    disable_plugin_graph(&conn, &manifest_path).expect("disable plugin graph");

    let plugin_enabled: i64 = conn
        .query_row(
            "SELECT json_extract(metadata, '$.enabled') FROM graph_nodes WHERE id = ?1",
            ["plugin:memorycore.typescript-analyzer"],
            |row| row.get(0),
        )
        .expect("query plugin enabled");
    assert_eq!(plugin_enabled, 0);
}

#[test]
fn rejects_unknown_plugin_capability() {
    let temp = tempdir().expect("create temp project");
    init_project(temp.path()).expect("init project");
    let manifest_path = temp.path().join("plugin.json");
    fs::write(
        &manifest_path,
        r#"{
          "id": "memorycore.bad",
          "name": "Bad Plugin",
          "version": "0.1.0",
          "entry": "index.js",
          "capabilities": ["network_everything"],
          "hooks": []
        }"#,
    )
    .expect("write manifest");

    let err = install_plugin(temp.path(), &manifest_path).expect_err("reject plugin");
    assert!(err.to_string().contains("not allowed"));
}

#[test]
fn registers_skill_directory() {
    let temp = tempdir().expect("create temp project");
    init_project(temp.path()).expect("init project");
    let skill_dir = temp.path().join("generate-diagram");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(
        skill_dir.join("SKILL.md"),
        "# Generate Diagram\n\nUse graph MCP tools to produce Mermaid diagrams.\n",
    )
    .expect("write skill");

    let skill = register_skill(temp.path(), &skill_dir).expect("register skill");
    assert_eq!(skill.id, "generate-diagram");
    assert_eq!(skill.name, "generate-diagram");
    assert_eq!(
        skill.description.as_deref(),
        Some("Use graph MCP tools to produce Mermaid diagrams.")
    );

    let skills = list_skills(temp.path()).expect("list skills");
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].id, "generate-diagram");

    let conn = Connection::open(temp.path().join(".memorycore/index.db")).expect("open db");
    let skill_node_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE id = ?1 AND kind = 'Skill'",
            ["skill:generate-diagram"],
            |row| row.get(0),
        )
        .expect("query skill node");
    assert_eq!(skill_node_count, 1);
    let skill_edge_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE source_id = 'project:root' AND target_id = ?1 AND kind = 'contains'",
            ["skill:generate-diagram"],
            |row| row.get(0),
        )
        .expect("query skill edge");
    assert_eq!(skill_edge_count, 1);

    let graph = graph_target_json_depth(&conn, "skill:generate-diagram", 1).expect("graph query");
    assert!(graph.contains("\"kind\": \"Skill\""));
    assert!(graph.contains("\"id\": \"project:root\""));

    disable_skill_graph(&conn, &skill_dir.join("SKILL.md")).expect("disable skill graph");
    let skill_enabled: i64 = conn
        .query_row(
            "SELECT json_extract(metadata, '$.enabled') FROM graph_nodes WHERE id = ?1",
            ["skill:generate-diagram"],
            |row| row.get(0),
        )
        .expect("query skill enabled");
    assert_eq!(skill_enabled, 0);
}

#[test]
fn disables_skill_graph_state() {
    let temp = tempdir().expect("create temp project");
    init_project(temp.path()).expect("init project");
    let skill_dir = temp.path().join("generate-diagram");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    let skill_path = skill_dir.join("SKILL.md");
    fs::write(
        &skill_path,
        "# Generate Diagram\n\nUse graph MCP tools to produce Mermaid diagrams.\n",
    )
    .expect("write skill");

    register_skill(temp.path(), &skill_path).expect("register skill");
    let conn = Connection::open(temp.path().join(".memorycore/index.db")).expect("open db");
    disable_skill_graph(&conn, &skill_path).expect("disable skill graph");

    let skill_enabled: i64 = conn
        .query_row(
            "SELECT json_extract(metadata, '$.enabled') FROM graph_nodes WHERE id = ?1",
            ["skill:generate-diagram"],
            |row| row.get(0),
        )
        .expect("query skill enabled");
    assert_eq!(skill_enabled, 0);
}
