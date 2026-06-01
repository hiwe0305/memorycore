use anyhow::{anyhow, Result};
use memorycore_core::{
    analyze_target, connect_project_db, list_snapshots, render_analysis_mermaid, search_hits,
    search_surface_counts, snapshot_count, snapshot_details,
};
use memorycore_graph::impact::find_impact_with_depth;
use memorycore_graph::query::{graph_subset_json_depth, graph_subset_mermaid_depth};
use memorycore_graph::render::json::render_json;
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

pub fn serve(project_root: &Path, addr: &str) -> Result<()> {
    let server = Server::http(addr).map_err(|err| anyhow!("bind API server on {addr}: {err}"))?;
    for request in server.incoming_requests() {
        if let Err(error) = handle_request(project_root, request) {
            eprintln!("memorycore-api error: {error}");
        }
    }
    Ok(())
}

fn handle_request(project_root: &Path, request: Request) -> Result<()> {
    let method = request.method().clone();
    let url = request.url().to_string();
    let (path, query) = split_url(&url);
    let mut response = match (method, path) {
        (Method::Get, "/health") => {
            json_response(&json!({"ok": true, "service": "memorycore"}), 200)
        }
        (Method::Get, "/status") => {
            let conn = connect_project_db(project_root)?;
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
            let embedding_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))?;
            let event_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM event_log", [], |row| row.get(0))?;
            let snapshot_total = snapshot_count(&conn)?;
            let daemon = daemon_status_state(project_root);
            json_response(
                &json!({
                    "project_root": project_root.display().to_string(),
                    "graph_nodes": node_count,
                    "graph_edges": edge_count,
                    "plugins": plugin_count,
                    "skills": skill_count,
                    "adapters": adapter_count,
                    "embeddings": embedding_count,
                    "events": event_count,
                    "snapshots": snapshot_total,
                    "daemon": daemon
                }),
                200,
            )
        }
        (Method::Get, "/graph.json") => {
            let conn = connect_project_db(project_root)?;
            Response::from_string(render_json(&conn)?).with_status_code(StatusCode(200))
        }
        (Method::Get, "/search") => {
            let conn = connect_project_db(project_root)?;
            let query_text = parse_query_param(query, "q").unwrap_or_default();
            let limit = parse_query_param(query, "limit")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(10);
            let kind = parse_query_param(query, "kind");
            let mut hits = search_hits(&conn, &query_text, limit, kind.as_deref())?;
            let surfaces = search_surface_counts(&conn, &query_text)?;
            for hit in &mut hits {
                if hit.kind == "File" && hit.node_id.is_none() {
                    if let Some(node_id) =
                        resolve_file_node_id(project_root, &conn, hit.path.as_deref())?
                    {
                        hit.node_id = Some(node_id);
                    }
                }
            }
            json_response(
                &json!({
                    "query": query_text,
                    "limit": limit,
                    "kind": kind,
                    "surfaces": surfaces,
                    "hits": hits
                }),
                200,
            )
        }
        (Method::Get, "/impact") => {
            let conn = connect_project_db(project_root)?;
            let target = parse_query_param(query, "target").unwrap_or_default();
            if target.trim().is_empty() {
                json_response(&json!({"error": "missing target"}), 400)
            } else {
                let limit = parse_query_param(query, "limit")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(25);
                let depth = parse_query_param(query, "depth")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1);
                let impact = find_impact_with_depth(&conn, &target, limit, depth)?;
                text_response(&impact, 200, "text/plain; charset=utf-8")
            }
        }
        (Method::Get, "/analyze") => {
            let conn = connect_project_db(project_root)?;
            let target = parse_query_param(query, "target").unwrap_or_default();
            if target.trim().is_empty() {
                json_response(&json!({"error": "missing target"}), 400)
            } else {
                let depth = parse_query_param(query, "depth")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1);
                let limit = parse_query_param(query, "limit")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(10);
                let report = analyze_target(&conn, &target, depth, limit)?;
                if parse_query_param(query, "format").as_deref() == Some("mermaid") {
                    text_response(
                        &render_analysis_mermaid(&report),
                        200,
                        "text/plain; charset=utf-8",
                    )
                } else {
                    json_response(&serde_json::to_value(report)?, 200)
                }
            }
        }
        (Method::Get, "/events") => {
            let conn = connect_project_db(project_root)?;
            let limit = parse_query_param(query, "limit")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(25);
            let status = parse_query_param(query, "status");
            let node_id = parse_query_param(query, "node_id");
            let total: i64 = if let Some(status) = status.as_deref() {
                conn.query_row(
                    "SELECT COUNT(*) FROM event_log WHERE status = ?1",
                    [status],
                    |row| row.get(0),
                )?
            } else {
                conn.query_row("SELECT COUNT(*) FROM event_log", [], |row| row.get(0))?
            };
            let events = recent_events(
                project_root,
                &conn,
                limit,
                status.as_deref(),
                node_id.as_deref(),
            )?;
            json_response(
                &json!({
                    "limit": limit,
                    "status": status,
                    "node_id": node_id,
                    "total": total,
                    "events": events
                }),
                200,
            )
        }
        (Method::Get, "/snapshots") => {
            let conn = connect_project_db(project_root)?;
            let limit = parse_query_param(query, "limit")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(25);
            let snapshots = list_snapshots(&conn, limit)?;
            json_response(
                &json!({
                    "limit": limit,
                    "total": snapshot_count(&conn)?,
                    "snapshots": snapshots
                }),
                200,
            )
        }
        (Method::Get, "/adapters") => json_response(&adapters_payload(project_root, query)?, 200),
        (Method::Get, "/memory-cases") => {
            let conn = connect_project_db(project_root)?;
            json_response(&memory_cases_payload(&conn, query)?, 200)
        }
        (Method::Get, "/sessions") => {
            let conn = connect_project_db(project_root)?;
            json_response(&sessions_payload(&conn, query)?, 200)
        }
        (Method::Get, "/embeddings/search") => {
            json_response(&embedding_search_payload(project_root, query)?, 200)
        }
        (Method::Get, "/embeddings") => {
            json_response(&embeddings_payload(project_root, query)?, 200)
        }
        (Method::Get, path) if path.starts_with("/session/") => {
            let conn = connect_project_db(project_root)?;
            let session_id = path.trim_start_matches("/session/");
            let limit = parse_query_param(query, "limit")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(100)
                .clamp(1, 500);
            match session_payload(&conn, session_id, limit)? {
                Some(payload) => json_response(&payload, 200),
                None => json_response(&json!({"error": "session not found"}), 404),
            }
        }
        (Method::Get, path) if path.starts_with("/graph/") => {
            let conn = connect_project_db(project_root)?;
            let node_id = path.trim_start_matches("/graph/");
            let depth = parse_query_param(query, "depth")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1);
            if parse_query_param(query, "format").as_deref() == Some("mermaid") {
                text_response(
                    &graph_subset_mermaid_depth(&conn, node_id, depth)?,
                    200,
                    "text/plain; charset=utf-8",
                )
            } else {
                let payload = graph_subset_json_depth(&conn, node_id, depth)?;
                Response::from_string(payload).with_status_code(StatusCode(200))
            }
        }
        (Method::Get, path) if path.starts_with("/snapshot/") => {
            let conn = connect_project_db(project_root)?;
            let hash = path.trim_start_matches("/snapshot/");
            match snapshot_details(&conn, hash)? {
                Some(details) => json_response(
                    &json!({
                        "snapshot": details.snapshot,
                        "files": details.files
                    }),
                    200,
                ),
                None => json_response(&json!({"error": "snapshot not found"}), 404),
            }
        }
        (Method::Get, "/") => json_response(&json!({"service": "memorycore-api"}), 200),
        _ => json_response(&json!({"error": "not found"}), 404),
    };

    add_cors(&mut response);
    request.respond(response)?;
    Ok(())
}

fn split_url(url: &str) -> (&str, Option<&str>) {
    match url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (url, None),
    }
}

fn parse_query_param(query: Option<&str>, key: &str) -> Option<String> {
    query.and_then(|query| {
        query.split('&').find_map(|part| {
            let (param_key, param_value) = part.split_once('=')?;
            (param_key == key).then(|| decode_query_value(param_value))
        })
    })
}

fn resolve_file_node_id(
    project_root: &Path,
    conn: &Connection,
    path: Option<&str>,
) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let abs_path = std::path::Path::new(path);
    let rel = abs_path
        .strip_prefix(project_root)
        .ok()
        .unwrap_or(abs_path)
        .to_string_lossy()
        .replace('\\', "/");
    let node_id = conn
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
        .ok();
    Ok(node_id)
}

fn adapters_payload(project_root: &Path, query: Option<&str>) -> Result<serde_json::Value> {
    let limit = parse_query_param(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(25)
        .clamp(1, 100);
    let agent_filter = parse_query_param(query, "agent").map(|value| value.to_lowercase());
    let adapters = memorycore_adapters::list_adapters(project_root)?;
    let total = adapters.len();
    let filtered = adapters
        .into_iter()
        .filter(|adapter| {
            let Some(agent_filter) = agent_filter.as_deref() else {
                return true;
            };
            let haystack = format!("{} {}", adapter.agent, adapter.name).to_lowercase();
            haystack.contains(agent_filter)
        })
        .take(limit)
        .collect::<Vec<_>>();
    Ok(json!({
        "limit": limit,
        "agent": agent_filter,
        "total": total,
        "adapters": filtered
    }))
}

fn memory_cases_payload(conn: &Connection, query: Option<&str>) -> Result<serde_json::Value> {
    let limit = parse_query_param(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(25)
        .clamp(1, 100);
    let target_filter = parse_query_param(query, "target").map(|value| value.to_lowercase());
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM memory_cases", [], |row| row.get(0))?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, name, summary, target, created_at, updated_at
        FROM memory_cases
        ORDER BY updated_at DESC, created_at DESC, id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "name": row.get::<_, String>(1)?,
            "summary": row.get::<_, Option<String>>(2)?,
            "target": row.get::<_, Option<String>>(3)?,
            "created_at": row.get::<_, i64>(4)?,
            "updated_at": row.get::<_, i64>(5)?,
        }))
    })?;
    let mut cases = Vec::new();
    for row in rows {
        let case = row?;
        if let Some(target_filter) = target_filter.as_deref() {
            let haystack = format!(
                "{} {} {} {}",
                case["id"].as_str().unwrap_or(""),
                case["name"].as_str().unwrap_or(""),
                case["summary"].as_str().unwrap_or(""),
                case["target"].as_str().unwrap_or("")
            )
            .to_lowercase();
            if !haystack.contains(target_filter) {
                continue;
            }
        }
        cases.push(case);
        if cases.len() >= limit {
            break;
        }
    }
    Ok(json!({
        "limit": limit,
        "target": target_filter,
        "total": total,
        "memory_cases": cases
    }))
}

fn sessions_payload(conn: &Connection, query: Option<&str>) -> Result<serde_json::Value> {
    let limit = parse_query_param(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(25)
        .clamp(1, 100);
    let agent_filter = parse_query_param(query, "agent").map(|value| value.to_lowercase());
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, agent, started_at, ended_at, token_count, message_count
        FROM sessions
        ORDER BY started_at DESC, id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "agent": row.get::<_, String>(1)?,
            "started_at": row.get::<_, i64>(2)?,
            "ended_at": row.get::<_, Option<i64>>(3)?,
            "token_count": row.get::<_, i64>(4)?,
            "message_count": row.get::<_, i64>(5)?,
        }))
    })?;
    let mut sessions = Vec::new();
    for row in rows {
        let session = row?;
        if let Some(agent_filter) = agent_filter.as_deref() {
            let haystack = format!(
                "{} {}",
                session["id"].as_str().unwrap_or(""),
                session["agent"].as_str().unwrap_or("")
            )
            .to_lowercase();
            if !haystack.contains(agent_filter) {
                continue;
            }
        }
        sessions.push(session);
        if sessions.len() >= limit {
            break;
        }
    }
    Ok(json!({
        "limit": limit,
        "agent": agent_filter,
        "total": total,
        "sessions": sessions
    }))
}

fn session_payload(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> Result<Option<serde_json::Value>> {
    let session = conn
        .query_row(
            r#"
            SELECT id, agent, started_at, ended_at, token_count, message_count
            FROM sessions
            WHERE id = ?1
            "#,
            [session_id],
            |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "agent": row.get::<_, String>(1)?,
                    "started_at": row.get::<_, i64>(2)?,
                    "ended_at": row.get::<_, Option<i64>>(3)?,
                    "token_count": row.get::<_, i64>(4)?,
                    "message_count": row.get::<_, i64>(5)?,
                }))
            },
        )
        .ok();
    let Some(session) = session else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        r#"
        SELECT id, role, content, timestamp, metadata
        FROM messages
        WHERE session_id = ?1
        ORDER BY timestamp, id
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map((session_id, limit as i64), |row| {
        Ok(json!({
            "id": row.get::<_, i64>(0)?,
            "role": row.get::<_, String>(1)?,
            "content": row.get::<_, String>(2)?,
            "timestamp": row.get::<_, i64>(3)?,
            "metadata": serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(4)?)
                .unwrap_or_else(|_| json!({})),
        }))
    })?;
    let messages = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(json!({
        "session": session,
        "limit": limit,
        "messages": messages
    })))
}

fn embeddings_payload(project_root: &Path, query: Option<&str>) -> Result<serde_json::Value> {
    let limit = parse_query_param(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(25)
        .clamp(1, 100);
    let chunk_type_filter =
        parse_query_param(query, "chunk_type").map(|value| value.to_lowercase());
    let records = memorycore_embeddings::list_embeddings(project_root)?;
    let total = records.len();
    let filtered = records
        .into_iter()
        .filter(|record| {
            let Some(chunk_type_filter) = chunk_type_filter.as_deref() else {
                return true;
            };
            record["chunk_type"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains(chunk_type_filter)
        })
        .take(limit)
        .collect::<Vec<_>>();
    Ok(json!({
        "limit": limit,
        "chunk_type": chunk_type_filter,
        "total": total,
        "path": memorycore_embeddings::embeddings_path(project_root).display().to_string(),
        "embeddings": filtered
    }))
}

fn embedding_search_payload(project_root: &Path, query: Option<&str>) -> Result<serde_json::Value> {
    let query_text = parse_query_param(query, "q").unwrap_or_default();
    let limit = parse_query_param(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(10)
        .clamp(1, 100);
    if query_text.trim().is_empty() {
        return Ok(json!({
            "query": query_text,
            "limit": limit,
            "hits": []
        }));
    }
    let hits = memorycore_embeddings::search_embeddings(project_root, &query_text, limit)?;
    Ok(json!({
        "query": query_text,
        "limit": limit,
        "hits": hits
    }))
}

fn resolve_graph_node_id(conn: &Connection, node_id: Option<&str>) -> Result<Option<String>> {
    let Some(node_id) = node_id else {
        return Ok(None);
    };
    let resolved = conn
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
        .ok();
    Ok(resolved)
}

fn recent_events(
    project_root: &Path,
    conn: &Connection,
    limit: usize,
    status: Option<&str>,
    node_id_filter: Option<&str>,
) -> Result<Vec<serde_json::Value>> {
    let events = if let Some(status) = status {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, timestamp, source, event_type, event_data, status, attempts, error
            FROM event_log
            WHERE status = ?1
            ORDER BY timestamp DESC, id DESC
            LIMIT ?2
            "#,
        )?;
        let mut rows = stmt.query((status, limit as i64))?;
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
            if let Some(filter) = node_id_filter {
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
        events
    } else {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, timestamp, source, event_type, event_data, status, attempts, error
            FROM event_log
            ORDER BY timestamp DESC, id DESC
            LIMIT ?1
            "#,
        )?;
        let mut rows = stmt.query((limit as i64,))?;
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
            if let Some(filter) = node_id_filter {
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
        events
    };
    Ok(events)
}

fn resolve_event_node_id(
    project_root: &Path,
    conn: &Connection,
    event_data: &serde_json::Value,
) -> Result<Option<String>> {
    if let Some(node_id) = event_data.get("id").and_then(serde_json::Value::as_str) {
        if let Some(resolved) = resolve_graph_node_id(conn, Some(node_id))? {
            return Ok(Some(resolved));
        }
    }
    if let Some(path) = event_data.get("path").and_then(serde_json::Value::as_str) {
        if let Some(resolved) = resolve_file_node_id(project_root, conn, Some(path))? {
            return Ok(Some(resolved));
        }
    }
    Ok(None)
}

fn decode_query_value(value: &str) -> String {
    let mut output = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = &value[index + 1..index + 3];
                if let Ok(decoded) = u8::from_str_radix(hex, 16) {
                    output.push(decoded);
                    index += 3;
                } else {
                    output.push(b'%');
                    index += 1;
                }
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memorycore_core::create_snapshot;
    use memorycore_graph::scan_file;
    use memorycore_graph::scanner::{remove_file_content_tree_index, remove_file_graph_index};

    #[test]
    fn decode_query_value_handles_spaces() {
        assert_eq!(decode_query_value("hello+world"), "hello world");
    }

    #[test]
    fn decode_query_value_handles_utf8() {
        assert_eq!(decode_query_value("%E2%9C%93"), "✓");
    }

    #[test]
    fn parse_query_param_reads_depth_values() {
        let query = Some("depth=2&format=mermaid");
        assert_eq!(parse_query_param(query, "depth").as_deref(), Some("2"));
        assert_eq!(
            parse_query_param(query, "format").as_deref(),
            Some("mermaid")
        );
    }

    #[test]
    fn recent_events_returns_latest_rows_first() {
        let temp_dir =
            std::env::temp_dir().join(format!("memorycore-api-events-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src")).unwrap();
        std::fs::write(temp_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        memorycore_graph::scan_file(&conn, &temp_dir, &temp_dir.join("src/main.rs")).unwrap();
        memorycore_core::append_event(
            &conn,
            "graph",
            "file_changed",
            &json!({"path": "src/main.rs"}),
        )
        .unwrap();
        memorycore_core::append_event(&conn, "daemon", "snapshot_created", &json!({"hash": "abc"}))
            .unwrap();
        let events = recent_events(&temp_dir, &conn, 2, None, None).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event_type"], "snapshot_created");
        assert_eq!(events[1]["event_type"], "file_changed");
        assert_eq!(events[0]["event_data"]["hash"], "abc");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn recent_events_resolves_node_ids_from_paths() {
        let temp_dir =
            std::env::temp_dir().join(format!("memorycore-api-events-node-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src")).unwrap();
        std::fs::write(temp_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        memorycore_graph::scan_file(&conn, &temp_dir, &temp_dir.join("src/main.rs")).unwrap();
        memorycore_core::append_event(
            &conn,
            "memorycore-graph",
            "graph_file_scanned",
            &json!({"path": "src/main.rs"}),
        )
        .unwrap();
        let events = recent_events(&temp_dir, &conn, 1, None, None).unwrap();
        assert_eq!(events[0]["node_id"], "file:src/main.rs");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn recent_events_filters_by_node_id() {
        let temp_dir = std::env::temp_dir().join(format!(
            "memorycore-api-events-filter-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src")).unwrap();
        std::fs::write(temp_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        memorycore_graph::scan_file(&conn, &temp_dir, &temp_dir.join("src/main.rs")).unwrap();
        memorycore_core::append_event(
            &conn,
            "memorycore-daemon",
            "snapshot_created",
            &json!({"hash": "abc"}),
        )
        .unwrap();
        let events = recent_events(&temp_dir, &conn, 10, None, Some("file:src/main.rs")).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["node_id"], "file:src/main.rs");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn snapshots_list_and_details_are_available() {
        let temp_dir =
            std::env::temp_dir().join(format!("memorycore-api-snapshots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src")).unwrap();
        std::fs::write(temp_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        let outcome =
            memorycore_core::create_snapshot(&temp_dir, &conn, "api snapshot", "memorycore-test")
                .unwrap();

        let snapshots = memorycore_core::list_snapshots(&conn, 5).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].hash, outcome.record.hash);
        let hits = memorycore_core::search_hits(&conn, "api snapshot", 10, None).unwrap();
        assert!(hits.iter().any(|hit| hit.kind == "Snapshot"));
        assert!(hits.iter().any(|hit| hit
            .node_id
            .as_deref()
            .is_some_and(|id| id.starts_with("snapshot:"))));

        let details = snapshot_details(&conn, &outcome.record.hash)
            .unwrap()
            .unwrap();
        assert_eq!(details.snapshot.hash, outcome.record.hash);
        assert_eq!(details.files.len(), 1);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn search_hits_include_snapshots() {
        let temp_dir = std::env::temp_dir().join(format!(
            "memorycore-api-search-snapshots-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src")).unwrap();
        std::fs::write(temp_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        memorycore_core::create_snapshot(&temp_dir, &conn, "checkpoint beta", "memorycore-test")
            .unwrap();

        let hits = memorycore_core::search_hits(&conn, "checkpoint", 10, None).unwrap();
        assert!(hits.iter().any(|hit| hit.kind == "Snapshot"));
        assert!(hits.iter().any(|hit| hit
            .snapshot_hash
            .as_deref()
            .is_some_and(|hash| !hash.is_empty())));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn search_hits_can_be_filtered_by_kind() {
        let temp_dir =
            std::env::temp_dir().join(format!("memorycore-api-search-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src")).unwrap();
        std::fs::write(
            temp_dir.join("src/main.rs"),
            "fn main() { println!(\"filter token\"); }\n",
        )
        .unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        scan_file(&conn, &temp_dir, &temp_dir.join("src/main.rs")).unwrap();
        create_snapshot(&temp_dir, &conn, "filter token snapshot", "memorycore-test").unwrap();

        let snapshot_hits =
            memorycore_core::search_hits(&conn, "filter token", 10, Some("Snapshot")).unwrap();
        assert!(snapshot_hits.iter().all(|hit| hit.kind == "Snapshot"));
        assert!(snapshot_hits
            .iter()
            .any(|hit| hit.title.contains("filter token snapshot")));

        let file_hits =
            memorycore_core::search_hits(&conn, "filter token", 10, Some("File")).unwrap();
        assert!(file_hits.iter().all(|hit| hit.kind == "File"));
        assert!(file_hits.iter().any(|hit| hit
            .path
            .as_deref()
            .is_some_and(|path| path.ends_with("src/main.rs"))));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn search_hits_are_sorted_with_files_first() {
        let temp_dir = std::env::temp_dir().join(format!(
            "memorycore-api-search-order-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src")).unwrap();
        std::fs::write(
            temp_dir.join("src/main.rs"),
            "fn main() { println!(\"order token\"); }\n",
        )
        .unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        scan_file(&conn, &temp_dir, &temp_dir.join("src/main.rs")).unwrap();
        create_snapshot(&temp_dir, &conn, "order token snapshot", "memorycore-test").unwrap();

        let hits = memorycore_core::search_hits(&conn, "order token", 10, None).unwrap();
        let file_pos = hits.iter().position(|hit| hit.kind == "File");
        let snapshot_pos = hits.iter().position(|hit| hit.kind == "Snapshot");
        assert!(file_pos.is_some());
        assert!(snapshot_pos.is_some());
        assert!(file_pos.unwrap() < snapshot_pos.unwrap());
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn search_surface_counts_include_current_query_scope() {
        let temp_dir = std::env::temp_dir().join(format!(
            "memorycore-api-search-surfaces-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src")).unwrap();
        std::fs::write(
            temp_dir.join("src/main.rs"),
            "fn main() { println!(\"surface token\"); }\n",
        )
        .unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        scan_file(&conn, &temp_dir, &temp_dir.join("src/main.rs")).unwrap();
        create_snapshot(
            &temp_dir,
            &conn,
            "surface token snapshot",
            "memorycore-test",
        )
        .unwrap();

        let counts = memorycore_core::search_surface_counts(&conn, "surface token").unwrap();
        assert!(counts.iter().any(|facet| facet.surface == "graph"));
        assert!(counts.iter().any(|facet| facet.surface == "files"));
        assert!(counts.iter().any(|facet| facet.surface == "skills"));
        assert!(counts.iter().any(|facet| facet.surface == "plugins"));
        assert!(counts.iter().any(|facet| facet.surface == "adapters"));
        assert!(counts.iter().any(|facet| facet.surface == "memory"));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn adapters_payload_lists_and_filters_registered_adapters() {
        let temp_dir =
            std::env::temp_dir().join(format!("memorycore-api-adapters-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        memorycore_core::init_project(&temp_dir).unwrap();
        memorycore_adapters::register_adapter(
            &temp_dir,
            "codex",
            Some("Codex CLI"),
            Some(&temp_dir.join(".memorycore/sessions/codex")),
            Some("codex"),
        )
        .unwrap();
        memorycore_adapters::register_adapter(
            &temp_dir,
            "claude",
            Some("Claude Code"),
            Some(&temp_dir.join(".memorycore/sessions/claude")),
            Some("claude"),
        )
        .unwrap();

        let payload = adapters_payload(&temp_dir, Some("agent=codex&limit=10")).unwrap();
        assert_eq!(payload["total"], 2);
        assert_eq!(payload["agent"], "codex");
        assert_eq!(payload["adapters"].as_array().unwrap().len(), 1);
        assert_eq!(payload["adapters"][0]["id"], "codex");
        assert_eq!(payload["adapters"][0]["name"], "Codex CLI");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn memory_cases_payload_lists_and_filters_cases() {
        let temp_dir = std::env::temp_dir().join(format!(
            "memorycore-api-memory-cases-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        conn.execute(
            r#"
            INSERT INTO memory_cases (id, name, summary, target, created_at, updated_at)
            VALUES
                ('memory:auth', 'Auth refactor', 'auth notes', 'src/auth.rs', 1, 2),
                ('memory:graph', 'Graph refactor', 'graph notes', 'src/graph.rs', 1, 3)
            "#,
            [],
        )
        .unwrap();

        let payload = memory_cases_payload(&conn, Some("target=auth&limit=10")).unwrap();
        assert_eq!(payload["total"], 2);
        assert_eq!(payload["target"], "auth");
        assert_eq!(payload["memory_cases"].as_array().unwrap().len(), 1);
        assert_eq!(payload["memory_cases"][0]["id"], "memory:auth");
        assert_eq!(payload["memory_cases"][0]["name"], "Auth refactor");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn sessions_payload_lists_and_shows_messages() {
        let temp_dir =
            std::env::temp_dir().join(format!("memorycore-api-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        conn.execute(
            r#"
            INSERT INTO sessions (id, agent, started_at, ended_at, token_count, message_count)
            VALUES ('demo', 'codex', 10, NULL, 0, 2)
            "#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT INTO messages (session_id, role, content, timestamp, metadata)
            VALUES
                ('demo', 'user', 'hello session', 10, '{}'),
                ('demo', 'assistant', 'hi back', 11, '{}')
            "#,
            [],
        )
        .unwrap();

        let list = sessions_payload(&conn, Some("agent=codex&limit=5")).unwrap();
        assert_eq!(list["total"], 1);
        assert_eq!(list["sessions"][0]["id"], "demo");
        assert_eq!(list["sessions"][0]["agent"], "codex");

        let detail = session_payload(&conn, "demo", 10).unwrap().unwrap();
        assert_eq!(detail["session"]["id"], "demo");
        assert_eq!(detail["messages"].as_array().unwrap().len(), 2);
        assert_eq!(detail["messages"][0]["content"], "hello session");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn embeddings_payload_lists_metadata_and_path() {
        let temp_dir =
            std::env::temp_dir().join(format!("memorycore-api-embeddings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        conn.execute(
            r#"
            INSERT INTO sessions (id, agent, started_at, ended_at, token_count, message_count)
            VALUES ('demo', 'codex', 10, NULL, 0, 1)
            "#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT INTO messages (session_id, role, content, timestamp, metadata)
            VALUES ('demo', 'user', 'hello embeddings', 10, '{}')
            "#,
            [],
        )
        .unwrap();
        memorycore_embeddings::build_message_embeddings_with_conn(&temp_dir, &conn).unwrap();

        let payload = embeddings_payload(&temp_dir, Some("chunk_type=message&limit=5")).unwrap();
        assert_eq!(payload["total"], 1);
        assert_eq!(payload["chunk_type"], "message");
        assert!(payload["path"]
            .as_str()
            .is_some_and(|path| path.ends_with(".memorycore/embeddings/chunks.bin")));
        assert_eq!(payload["embeddings"].as_array().unwrap().len(), 1);
        assert_eq!(payload["embeddings"][0]["chunk_type"], "message");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn embedding_search_payload_returns_ranked_hits() {
        let temp_dir = std::env::temp_dir().join(format!(
            "memorycore-api-embedding-search-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        conn.execute(
            r#"
            INSERT INTO sessions (id, agent, started_at, ended_at, token_count, message_count)
            VALUES ('demo', 'codex', 10, NULL, 0, 2)
            "#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT INTO messages (session_id, role, content, timestamp, metadata)
            VALUES
                ('demo', 'user', 'rust graph imports', 10, '{}'),
                ('demo', 'assistant', 'dashboard canvas', 11, '{}')
            "#,
            [],
        )
        .unwrap();
        memorycore_embeddings::build_message_embeddings_with_conn(&temp_dir, &conn).unwrap();

        let payload = embedding_search_payload(&temp_dir, Some("q=rust+imports&limit=5")).unwrap();
        assert_eq!(payload["query"], "rust imports");
        assert_eq!(payload["hits"].as_array().unwrap().len(), 1);
        assert_eq!(payload["hits"][0]["chunk_type"], "message");
        assert_eq!(payload["hits"][0]["snippet"], "rust graph imports");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn status_includes_daemon_state_when_pid_is_not_memorycore_daemon() {
        let temp_dir = std::env::temp_dir().join(format!(
            "memorycore-api-daemon-status-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join(".memorycore")).unwrap();
        let payload = serde_json::json!({
            "pid": std::process::id(),
            "started_at": 123,
            "project_root": temp_dir.display().to_string(),
            "last_activity_at": 456
        });
        std::fs::write(
            temp_dir.join(".memorycore").join("daemon.json"),
            serde_json::to_string_pretty(&payload).unwrap(),
        )
        .unwrap();
        let daemon_state = daemon_status_state(&temp_dir);
        assert_eq!(daemon_state["alive"], false);
        assert_eq!(daemon_state["status"]["last_activity_at"], 456);
        assert_eq!(daemon_state["error"], "daemon pid is not running");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn search_hits_disappear_after_file_cleanup() {
        let temp_dir = std::env::temp_dir().join(format!(
            "memorycore-api-search-cleanup-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src")).unwrap();
        let file = temp_dir.join("src/main.rs");
        std::fs::write(&file, "fn main() { println!(\"hello cleanup\"); }\n").unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        scan_file(&conn, &temp_dir, &file).unwrap();

        let before = search_hits(&conn, "cleanup", 10, None).unwrap();
        assert!(before.iter().any(|hit| hit.kind == "File"));

        remove_file_content_tree_index(&conn, &file).unwrap();
        remove_file_graph_index(&conn, &temp_dir, &file).unwrap();

        let after = search_hits(&conn, "cleanup", 10, None).unwrap();
        assert!(after.iter().all(|hit| hit.kind != "File"));
        assert!(after.iter().all(|hit| {
            hit.path
                .as_deref()
                .is_none_or(|path| path != file.to_string_lossy())
        }));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn search_hits_disappear_after_folder_cleanup() {
        let temp_dir = std::env::temp_dir().join(format!(
            "memorycore-api-search-folder-cleanup-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("src/nested")).unwrap();
        let folder = temp_dir.join("src");
        let file = temp_dir.join("src/nested/main.rs");
        std::fs::write(&file, "fn main() { println!(\"folder cleanup\"); }\n").unwrap();
        memorycore_core::init_project(&temp_dir).unwrap();
        let conn = connect_project_db(&temp_dir).unwrap();
        memorycore_graph::scan_folder(&conn, &temp_dir, &folder).unwrap();

        let before = search_hits(&conn, "folder cleanup", 10, None).unwrap();
        assert!(before.iter().any(|hit| hit.kind == "File"));

        remove_file_content_tree_index(&conn, &folder).unwrap();
        remove_file_graph_index(&conn, &temp_dir, &folder).unwrap();

        let after = search_hits(&conn, "folder cleanup", 10, None).unwrap();
        assert!(after.iter().all(|hit| hit.kind != "File"));
        assert!(after.iter().all(|hit| {
            hit.path
                .as_deref()
                .is_none_or(|path| !path.starts_with("src"))
        }));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

fn json_response(value: &serde_json::Value, status: u16) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(value.to_string()).with_status_code(StatusCode(status))
}

fn text_response(
    body: &str,
    status: u16,
    content_type: &str,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(body.to_string()).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes(b"Content-Type".as_slice(), content_type.as_bytes()) {
        response.add_header(header);
    }
    response
}

fn add_cors(response: &mut Response<std::io::Cursor<Vec<u8>>>) {
    let headers = [
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Headers", "*"),
        ("Access-Control-Allow-Methods", "GET, OPTIONS"),
    ];
    for (name, value) in headers {
        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response.add_header(header);
        }
    }
}

fn daemon_status_state(project_root: &Path) -> serde_json::Value {
    let path = project_root.join(".memorycore").join("daemon.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return json!({ "alive": false, "status": null, "error": "daemon status missing" });
    };
    let status: serde_json::Value = match serde_json::from_str(&text) {
        Ok(status) => status,
        Err(_) => {
            return json!({ "alive": false, "status": null, "error": "daemon status malformed" });
        }
    };
    let pid = status
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32);
    let Some(pid) = pid else {
        return json!({ "alive": false, "status": status, "error": "daemon status missing pid" });
    };
    if !daemon_process_alive(pid, project_root) {
        return json!({ "alive": false, "status": status, "error": "daemon pid is not running" });
    }
    json!({ "alive": true, "status": status, "error": null })
}

fn daemon_process_alive(pid: u32, project_root: &Path) -> bool {
    let cmdline_path = std::path::Path::new("/proc")
        .join(pid.to_string())
        .join("cmdline");
    let Ok(bytes) = std::fs::read(cmdline_path) else {
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
