use anyhow::{bail, Context, Result};
use memorycore_core::{
    analyze_target, connect_project_db, create_snapshot, format_analysis_report,
    format_search_hits, render_analysis_mermaid, search_hits,
};
use memorycore_graph::impact::find_impact_with_depth;
use memorycore_graph::query::graph_target_json_depth;
use memorycore_graph::query::graph_target_mermaid_depth;
use memorycore_graph::render::mermaid::render_mermaid;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

pub fn serve(project_root: &Path) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    while let Some(request) = read_message(&mut reader)? {
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            write_error(
                &mut writer,
                request.get("id").cloned(),
                -32600,
                "missing method",
            )?;
            continue;
        };

        if method.starts_with("notifications/") {
            continue;
        }

        let id = request.get("id").cloned();
        let result = match method {
            "initialize" => Ok(initialize_result()),
            "tools/list" => Ok(tools_list_result()),
            "tools/call" => handle_tool_call(project_root, request.get("params")),
            _ => Err(anyhow::anyhow!("unsupported MCP method {method}")),
        };

        match result {
            Ok(result) => write_response(&mut writer, id, result)?,
            Err(error) => write_error(&mut writer, id, -32000, &error.to_string())?,
        }
    }
    Ok(())
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "memorycore",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn tools_list_result() -> Value {
    json!({
        "tools": [
            {
                "name": "memorycore_search",
                "description": "Search local MemoryCore graph nodes and event log.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "kind": { "type": "string", "description": "Comma-separated hit kinds to include (e.g. File,Folder,Plugin)" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "memorycore_snapshot",
                "description": "Create a local snapshot in SQLite.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    }
                }
            },
            {
                "name": "memorycore_graph_query",
                "description": "Return graph nodes and edges from SQLite.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "target": { "type": "string" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                        "depth": { "type": "integer", "minimum": 0, "maximum": 10 }
                    }
                }
            },
            {
                "name": "memorycore_graph_render",
                "description": "Render the current graph as Mermaid.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "format": { "type": "string", "enum": ["mermaid"] },
                        "target": { "type": "string" },
                        "depth": { "type": "integer", "minimum": 0, "maximum": 10 }
                    }
                }
            },
            {
                "name": "memorycore_find_impact",
                "description": "Find incoming and outgoing graph edges for a target node/path/name.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                        "depth": { "type": "integer", "minimum": 0, "maximum": 10 }
                    },
                    "required": ["target"]
                }
            },
            {
                "name": "memorycore_adapters",
                "description": "List registered local agent adapters for this project.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "agent": { "type": "string", "description": "Optional agent id/name filter." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                    }
                }
            },
            {
                "name": "memorycore_memory_cases",
                "description": "List pinned MemoryCore memory cases for this project.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Optional filter across id, name, summary, and target." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                    }
                }
            },
            {
                "name": "memorycore_sessions",
                "description": "List imported sessions or show messages for one session.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Optional session id to show messages for." },
                        "agent": { "type": "string", "description": "Optional agent/session filter for list mode." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                    }
                }
            },
            {
                "name": "memorycore_embeddings",
                "description": "List local embedding metadata stored in SQLite.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "chunk_type": { "type": "string", "description": "Optional chunk type filter." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                    }
                }
            },
            {
                "name": "memorycore_embedding_search",
                "description": "Search local message embeddings from the binary vector store.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "memorycore_analyze",
                "description": "Analyze a file, folder, symbol, system target, or memory case using graph, search, and memory context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string" },
                        "format": { "type": "string", "enum": ["text", "mermaid"] },
                        "depth": { "type": "integer", "minimum": 0, "maximum": 4 },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                    },
                    "required": ["target"]
                }
            }
        ]
    })
}

fn handle_tool_call(project_root: &Path, params: Option<&Value>) -> Result<Value> {
    let params = params.context("missing tools/call params")?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .context("missing tool name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let conn = connect_project_db(project_root)?;

    let text = match name {
        "memorycore_search" => {
            let query = required_string(&arguments, "query")?;
            let limit = limit(&arguments, 10);
            let kind = arguments.get("kind").and_then(Value::as_str);
            search_index(&conn, query, limit as usize, kind)?
        }
        "memorycore_snapshot" => snapshot(project_root, &conn, &arguments)?,
        "memorycore_graph_query" => graph_query(&conn, &arguments)?,
        "memorycore_graph_render" => graph_render(&conn, &arguments)?,
        "memorycore_find_impact" => {
            let target = required_string(&arguments, "target")?;
            let limit = limit(&arguments, 25);
            let depth = depth(&arguments, 1);
            find_impact_with_depth(&conn, target, limit as usize, depth as usize)?
        }
        "memorycore_adapters" => adapters(project_root, &arguments)?,
        "memorycore_memory_cases" => memory_cases(&conn, &arguments)?,
        "memorycore_sessions" => sessions(&conn, &arguments)?,
        "memorycore_embeddings" => embeddings(project_root, &arguments)?,
        "memorycore_embedding_search" => embedding_search(project_root, &arguments)?,
        "memorycore_analyze" => analyze(&conn, &arguments)?,
        _ => bail!("unknown MemoryCore tool {name}"),
    };

    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ]
    }))
}

pub(crate) fn search_index(
    conn: &Connection,
    query: &str,
    limit: usize,
    kind_filter: Option<&str>,
) -> Result<String> {
    let hits = search_hits(conn, query, limit, kind_filter)?;
    Ok(format_search_hits(&hits))
}

fn snapshot(project_root: &Path, conn: &Connection, arguments: &Value) -> Result<String> {
    let message = arguments
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("MCP snapshot request");
    let outcome = create_snapshot(project_root, conn, message, "memorycore-mcp")?;
    Ok(format!(
        "Snapshot {} created with {} files, {} bytes, event_log id={}",
        outcome.record.hash, outcome.record.file_count, outcome.record.total_size, outcome.event_id
    ))
}

fn graph_query(conn: &Connection, arguments: &Value) -> Result<String> {
    if let Some(target) = arguments.get("target").and_then(Value::as_str) {
        let depth = depth(arguments, 1);
        return graph_target_json_depth(conn, target, depth as usize);
    }
    let query = arguments.get("query").and_then(Value::as_str).unwrap_or("");
    let limit = limit(arguments, 25);
    let like = format!("%{query}%");
    let mut output = String::from("nodes:\n");

    let mut stmt = conn.prepare(
        r#"
        SELECT id, kind, name, COALESCE(path, '')
        FROM graph_nodes
        WHERE ?1 = '%%' OR id LIKE ?1 OR name LIKE ?1 OR path LIKE ?1
        ORDER BY kind, path, name
        LIMIT ?2
        "#,
    )?;
    let nodes = stmt.query_map(params![like, limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for node in nodes {
        let (id, kind, name, path) = node?;
        output.push_str(&format!("- {id} [{kind}] {name} {path}\n"));
    }

    output.push_str("edges:\n");
    let mut stmt = conn.prepare(
        r#"
        SELECT source_id, kind, target_id
        FROM graph_edges
        ORDER BY source_id, kind, target_id
        LIMIT ?1
        "#,
    )?;
    let edges = stmt.query_map([limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for edge in edges {
        let (source, kind, target) = edge?;
        output.push_str(&format!("- {source} -{kind}-> {target}\n"));
    }

    Ok(output)
}

fn graph_render(conn: &Connection, arguments: &Value) -> Result<String> {
    let format = arguments
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("mermaid");
    if format != "mermaid" {
        bail!("only mermaid graph rendering is supported in the MVP MCP server");
    }
    if let Some(target) = arguments.get("target").and_then(Value::as_str) {
        let depth = depth(arguments, 1);
        graph_target_mermaid_depth(conn, target, depth as usize)
    } else {
        render_mermaid(conn)
    }
}

fn adapters(project_root: &Path, arguments: &Value) -> Result<String> {
    let agent_filter = arguments
        .get("agent")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let limit = limit(arguments, 25) as usize;
    let mut output = String::new();
    let mut count = 0;
    for adapter in memorycore_adapters::list_adapters(project_root)? {
        if let Some(agent_filter) = agent_filter.as_deref() {
            let haystack = format!("{} {}", adapter.agent, adapter.name).to_lowercase();
            if !haystack.contains(agent_filter) {
                continue;
            }
        }
        if count >= limit {
            break;
        }
        count += 1;
        let state = if adapter.enabled {
            "enabled"
        } else {
            "disabled"
        };
        output.push_str(&format!(
            "- adapter:{} [{}] agent={} name={} session_dir={} command={}\n",
            adapter.id,
            state,
            adapter.agent,
            adapter.name,
            adapter.session_dir.as_deref().unwrap_or(""),
            adapter.command.as_deref().unwrap_or("")
        ));
    }
    if output.is_empty() {
        output.push_str("No adapters registered.\n");
    }
    Ok(output)
}

fn memory_cases(conn: &Connection, arguments: &Value) -> Result<String> {
    let query_filter = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let limit = limit(arguments, 25) as usize;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, name, COALESCE(summary, ''), COALESCE(target, ''), created_at, updated_at
        FROM memory_cases
        ORDER BY updated_at DESC, created_at DESC, id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;
    let mut output = String::new();
    let mut count = 0;
    for row in rows {
        let (id, name, summary, target, created_at, updated_at) = row?;
        if let Some(query_filter) = query_filter.as_deref() {
            let haystack = format!("{id} {name} {summary} {target}").to_lowercase();
            if !haystack.contains(query_filter) {
                continue;
            }
        }
        if count >= limit {
            break;
        }
        count += 1;
        output.push_str(&format!(
            "- {id} name={} target={} created_at={} updated_at={} summary={}\n",
            name, target, created_at, updated_at, summary
        ));
    }
    if output.is_empty() {
        output.push_str("No memory cases registered.\n");
    }
    Ok(output)
}

fn sessions(conn: &Connection, arguments: &Value) -> Result<String> {
    let limit = limit(arguments, 25) as usize;
    if let Some(session_id) = arguments.get("id").and_then(Value::as_str) {
        return session_messages(conn, session_id, limit);
    }
    let agent_filter = arguments
        .get("agent")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let mut stmt = conn.prepare(
        r#"
        SELECT id, agent, started_at, ended_at, token_count, message_count
        FROM sessions
        ORDER BY started_at DESC, id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;
    let mut output = String::new();
    let mut count = 0;
    for row in rows {
        let (id, agent, started_at, ended_at, token_count, message_count) = row?;
        if let Some(agent_filter) = agent_filter.as_deref() {
            let haystack = format!("{id} {agent}").to_lowercase();
            if !haystack.contains(agent_filter) {
                continue;
            }
        }
        if count >= limit {
            break;
        }
        count += 1;
        output.push_str(&format!(
            "- session:{}:{} agent={} started_at={} ended_at={} messages={} tokens={}\n",
            agent,
            id,
            agent,
            started_at,
            ended_at.unwrap_or_default(),
            message_count,
            token_count
        ));
    }
    if output.is_empty() {
        output.push_str("No sessions registered.\n");
    }
    Ok(output)
}

fn session_messages(conn: &Connection, session_id: &str, limit: usize) -> Result<String> {
    let session: Option<(String, String, i64, Option<i64>, i64, i64)> = conn
        .query_row(
            r#"
            SELECT id, agent, started_at, ended_at, token_count, message_count
            FROM sessions
            WHERE id = ?1
            "#,
            [session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .ok();
    let Some((id, agent, started_at, ended_at, token_count, message_count)) = session else {
        return Ok(format!("Session not found: {session_id}\n"));
    };
    let mut output = format!(
        "session:{}:{} agent={} started_at={} ended_at={} messages={} tokens={}\n",
        agent,
        id,
        agent,
        started_at,
        ended_at.unwrap_or_default(),
        message_count,
        token_count
    );
    let mut stmt = conn.prepare(
        r#"
        SELECT role, content, timestamp
        FROM messages
        WHERE session_id = ?1
        ORDER BY timestamp, id
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map((session_id, limit as i64), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut count = 0;
    for row in rows {
        let (role, content, timestamp) = row?;
        count += 1;
        output.push_str(&format!("- {} {} {}\n", timestamp, role, content));
    }
    if count == 0 {
        output.push_str("No messages found.\n");
    }
    Ok(output)
}

fn embeddings(project_root: &Path, arguments: &Value) -> Result<String> {
    let chunk_type_filter = arguments
        .get("chunk_type")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let limit = limit(arguments, 25) as usize;
    let mut output = format!(
        "store={}\n",
        memorycore_embeddings::embeddings_path(project_root).display()
    );
    let mut count = 0;
    for record in memorycore_embeddings::list_embeddings(project_root)? {
        if let Some(chunk_type_filter) = chunk_type_filter.as_deref() {
            let chunk_type = record["chunk_type"].as_str().unwrap_or("").to_lowercase();
            if !chunk_type.contains(chunk_type_filter) {
                continue;
            }
        }
        if count >= limit {
            break;
        }
        count += 1;
        output.push_str(&format!(
            "- id={} type={} chunk={} offset={} metadata={}\n",
            record["id"].as_i64().unwrap_or_default(),
            record["chunk_type"].as_str().unwrap_or(""),
            record["chunk_id"].as_str().unwrap_or(""),
            record["embedding_offset"].as_i64().unwrap_or_default(),
            record["metadata"]
        ));
    }
    if count == 0 {
        output.push_str("No embeddings registered.\n");
    }
    Ok(output)
}

fn embedding_search(project_root: &Path, arguments: &Value) -> Result<String> {
    let query = required_string(arguments, "query")?;
    let limit = limit(arguments, 10) as usize;
    let hits = memorycore_embeddings::search_embeddings(project_root, query, limit)?;
    let mut output = format!("query={query}\n");
    for hit in hits {
        output.push_str(&format!(
            "- score={:.4} id={} type={} chunk={} offset={} metadata={} snippet={}\n",
            hit.score,
            hit.id,
            hit.chunk_type,
            hit.chunk_id,
            hit.embedding_offset,
            hit.metadata,
            hit.snippet.unwrap_or_default()
        ));
    }
    if output.lines().count() == 1 {
        output.push_str("No embedding hits.\n");
    }
    Ok(output)
}

fn analyze(conn: &Connection, arguments: &Value) -> Result<String> {
    let target = required_string(arguments, "target")?;
    let depth = depth(arguments, 1) as usize;
    let limit = limit(arguments, 10) as usize;
    let report = analyze_target(conn, target, depth, limit)?;
    if arguments.get("format").and_then(Value::as_str) == Some("mermaid") {
        Ok(render_analysis_mermaid(&report))
    } else {
        Ok(format_analysis_report(&report))
    }
}

fn required_string<'a>(arguments: &'a Value, field: &str) -> Result<&'a str> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("missing required string argument {field}"))
}

fn limit(arguments: &Value, default: i64) -> i64 {
    arguments
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(default)
        .clamp(1, 100)
}

fn depth(arguments: &Value, default: i64) -> i64 {
    arguments
        .get("depth")
        .and_then(Value::as_i64)
        .unwrap_or(default)
        .clamp(0, 10)
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>()?);
        }
    }

    let length = content_length.context("missing Content-Length header")?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

fn write_response(writer: &mut impl Write, id: Option<Value>, result: Value) -> Result<()> {
    write_message(
        writer,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }),
    )
}

fn write_error(writer: &mut impl Write, id: Option<Value>, code: i64, message: &str) -> Result<()> {
    write_message(
        writer,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message
            }
        }),
    )
}

fn write_message(writer: &mut impl Write, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memorycore_core::init_project;
    use memorycore_graph::scan_folder;
    use tempfile::tempdir;

    #[test]
    fn graph_render_honors_depth_for_targets() {
        let temp = tempdir().expect("temp dir");
        std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
        std::fs::write(
            temp.path().join("src/lib.rs"),
            "mod helper;\npub fn helper() {}\n",
        )
        .expect("lib");
        std::fs::write(
            temp.path().join("src/main.rs"),
            "use crate::helper::helper;\nfn main() { helper(); }\n",
        )
        .expect("main");
        std::fs::write(temp.path().join("src/helper.rs"), "pub fn helper() {}\n").expect("helper");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan");

        let output = graph_render(
            &conn,
            &json!({
                "format": "mermaid",
                "target": "src/main.rs",
                "depth": 2
            }),
        )
        .expect("render");
        assert!(output.contains("Function: helper"));
        assert!(output.contains("Import: crate::helper::helper"));
        assert!(output.contains("resolves_import_symbol"));
    }

    #[test]
    fn graph_query_honors_target_depth() {
        let temp = tempdir().expect("temp dir");
        std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
        std::fs::write(
            temp.path().join("src/lib.rs"),
            "mod helper;\npub fn helper() {}\n",
        )
        .expect("lib");
        std::fs::write(
            temp.path().join("src/main.rs"),
            "use crate::helper::helper;\nfn main() { helper(); }\n",
        )
        .expect("main");
        std::fs::write(temp.path().join("src/helper.rs"), "pub fn helper() {}\n").expect("helper");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan");

        let output = graph_query(
            &conn,
            &json!({
                "target": "src/main.rs",
                "depth": 2
            }),
        )
        .expect("query");
        assert!(output.contains("\"nodes\""));
        assert!(output.contains("symbol:src/helper.rs#helper"));
        assert!(output.contains("resolves_import_symbol"));
    }

    #[test]
    fn adapters_tool_lists_registered_adapters() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        memorycore_adapters::register_adapter(
            temp.path(),
            "codex",
            Some("Codex CLI"),
            Some(&temp.path().join(".memorycore/sessions/codex")),
            Some("codex"),
        )
        .expect("register adapter");

        let output = adapters(
            temp.path(),
            &json!({
                "agent": "codex",
                "limit": 10
            }),
        )
        .expect("adapters");
        assert!(output.contains("adapter:codex"));
        assert!(output.contains("agent=codex"));
        assert!(output.contains("name=Codex CLI"));
    }

    #[test]
    fn memory_cases_tool_lists_pinned_context() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        conn.execute(
            r#"
            INSERT INTO memory_cases (id, name, summary, target, created_at, updated_at)
            VALUES
                ('memory:auth', 'Auth refactor', 'auth notes', 'src/auth.rs', 1, 2),
                ('memory:graph', 'Graph refactor', 'graph notes', 'src/graph.rs', 1, 3)
            "#,
            [],
        )
        .expect("insert memory cases");

        let output = memory_cases(
            &conn,
            &json!({
                "query": "auth",
                "limit": 10
            }),
        )
        .expect("memory cases");
        assert!(output.contains("memory:auth"));
        assert!(output.contains("Auth refactor"));
        assert!(output.contains("src/auth.rs"));
        assert!(!output.contains("memory:graph"));
    }

    #[test]
    fn sessions_tool_lists_and_shows_messages() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        conn.execute(
            r#"
            INSERT INTO sessions (id, agent, started_at, ended_at, token_count, message_count)
            VALUES ('demo', 'codex', 10, NULL, 0, 2)
            "#,
            [],
        )
        .expect("insert session");
        conn.execute(
            r#"
            INSERT INTO messages (session_id, role, content, timestamp, metadata)
            VALUES
                ('demo', 'user', 'hello session', 10, '{}'),
                ('demo', 'assistant', 'hi back', 11, '{}')
            "#,
            [],
        )
        .expect("insert messages");

        let list = sessions(&conn, &json!({"agent": "codex", "limit": 10})).expect("list");
        assert!(list.contains("session:codex:demo"));
        assert!(list.contains("messages=2"));

        let show = sessions(&conn, &json!({"id": "demo", "limit": 10})).expect("show");
        assert!(show.contains("session:codex:demo"));
        assert!(show.contains("10 user hello session"));
        assert!(show.contains("11 assistant hi back"));
    }

    #[test]
    fn embeddings_tool_lists_embedding_metadata() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        conn.execute(
            r#"
            INSERT INTO sessions (id, agent, started_at, ended_at, token_count, message_count)
            VALUES ('demo', 'codex', 10, NULL, 0, 1)
            "#,
            [],
        )
        .expect("insert session");
        conn.execute(
            r#"
            INSERT INTO messages (session_id, role, content, timestamp, metadata)
            VALUES ('demo', 'user', 'hello embeddings', 10, '{}')
            "#,
            [],
        )
        .expect("insert message");
        memorycore_embeddings::build_message_embeddings_with_conn(temp.path(), &conn)
            .expect("build embeddings");

        let output = embeddings(
            temp.path(),
            &json!({
                "chunk_type": "message",
                "limit": 10
            }),
        )
        .expect("embeddings");
        assert!(output.contains(".memorycore/embeddings/chunks.bin"));
        assert!(output.contains("type=message"));
        assert!(output.contains("chunk=message:demo:"));
    }

    #[test]
    fn embedding_search_tool_returns_vector_hits() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        conn.execute(
            r#"
            INSERT INTO sessions (id, agent, started_at, ended_at, token_count, message_count)
            VALUES ('demo', 'codex', 10, NULL, 0, 2)
            "#,
            [],
        )
        .expect("insert session");
        conn.execute(
            r#"
            INSERT INTO messages (session_id, role, content, timestamp, metadata)
            VALUES
                ('demo', 'user', 'rust import resolver', 10, '{}'),
                ('demo', 'assistant', 'api dashboard payload', 11, '{}')
            "#,
            [],
        )
        .expect("insert messages");
        memorycore_embeddings::build_message_embeddings_with_conn(temp.path(), &conn)
            .expect("build embeddings");

        let output = embedding_search(
            temp.path(),
            &json!({
                "query": "rust resolver",
                "limit": 10
            }),
        )
        .expect("embedding search");
        assert!(output.contains("score="));
        assert!(output.contains("type=message"));
        assert!(output.contains("rust import resolver"));
    }

    #[test]
    fn analyze_tool_reports_target_context() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        conn.execute(
            r#"
            INSERT INTO graph_nodes (id, kind, name, path, metadata, updated_at)
            VALUES
                ('file:src/main.rs', 'File', 'main.rs', 'src/main.rs', '{}', 1),
                ('symbol:src/main.rs#main', 'Function', 'main', 'src/main.rs', '{}', 1)
            "#,
            [],
        )
        .expect("insert nodes");
        conn.execute(
            r#"
            INSERT INTO graph_edges (id, source_id, target_id, kind, updated_at)
            VALUES ('edge:file:src/main.rs:defines:symbol:src/main.rs#main', 'file:src/main.rs', 'symbol:src/main.rs#main', 'defines', 1)
            "#,
            [],
        )
        .expect("insert edge");
        conn.execute(
            r#"
            INSERT INTO memory_cases (id, name, summary, target, created_at, updated_at)
            VALUES ('memory:main', 'Main flow', 'notes', 'src/main.rs', 1, 2)
            "#,
            [],
        )
        .expect("insert memory");

        let output = analyze(
            &conn,
            &json!({
                "target": "src/main.rs",
                "depth": 1,
                "limit": 10
            }),
        )
        .expect("analyze");
        assert!(output.contains("MemoryCore Analysis"));
        assert!(output.contains("file:src/main.rs"));
        assert!(output.contains("memory:main"));
    }
}
