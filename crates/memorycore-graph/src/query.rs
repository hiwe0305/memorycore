use anyhow::{bail, Result};
use rusqlite::{params_from_iter, Connection};
use serde_json::json;
use std::collections::HashSet;

struct GraphSubset {
    focus: Option<serde_json::Value>,
    nodes: Vec<serde_json::Value>,
    edges: Vec<serde_json::Value>,
}

pub fn resolve_graph_target(conn: &Connection, target: &str) -> Result<Option<String>> {
    let like = format!("%{target}%");
    for (sql, param) in [
        ("SELECT id FROM graph_nodes WHERE id = ?1 LIMIT 1", target),
        ("SELECT id FROM graph_nodes WHERE path = ?1 LIMIT 1", target),
        ("SELECT id FROM graph_nodes WHERE name = ?1 LIMIT 1", target),
        (
            r#"
            SELECT id
            FROM graph_nodes
            WHERE id LIKE ?1 OR name LIKE ?1 OR path LIKE ?1
            ORDER BY kind, path, name
            LIMIT 1
            "#,
            like.as_str(),
        ),
    ] {
        let result = conn.query_row(sql, [param], |row| row.get(0));
        if let Ok(target_id) = result {
            return Ok(Some(target_id));
        }
    }
    Ok(None)
}

pub fn graph_subset_json(conn: &Connection, node_id: &str) -> Result<String> {
    graph_subset_json_depth(conn, node_id, 1)
}

pub fn graph_subset_json_depth(conn: &Connection, node_id: &str, depth: usize) -> Result<String> {
    let subset = load_graph_subset(conn, node_id, depth)?;
    Ok(serde_json::to_string_pretty(&json!({
        "focus": subset.focus,
        "nodes": subset.nodes,
        "edges": subset.edges
    }))?)
}

pub fn graph_subset_mermaid(conn: &Connection, node_id: &str) -> Result<String> {
    graph_subset_mermaid_depth(conn, node_id, 1)
}

pub fn graph_subset_mermaid_depth(
    conn: &Connection,
    node_id: &str,
    depth: usize,
) -> Result<String> {
    let subset = load_graph_subset(conn, node_id, depth)?;
    Ok(render_subset_mermaid(&subset))
}

pub fn graph_target_json(conn: &Connection, target: &str) -> Result<String> {
    graph_target_json_depth(conn, target, 1)
}

pub fn graph_target_json_depth(conn: &Connection, target: &str, depth: usize) -> Result<String> {
    let Some(node_id) = resolve_graph_target(conn, target)? else {
        bail!("No graph node found for target {target}");
    };
    graph_subset_json_depth(conn, &node_id, depth)
}

pub fn graph_target_mermaid(conn: &Connection, target: &str) -> Result<String> {
    graph_target_mermaid_depth(conn, target, 1)
}

pub fn graph_target_mermaid_depth(conn: &Connection, target: &str, depth: usize) -> Result<String> {
    let Some(node_id) = resolve_graph_target(conn, target)? else {
        bail!("No graph node found for target {target}");
    };
    graph_subset_mermaid_depth(conn, &node_id, depth)
}

fn load_graph_subset(conn: &Connection, node_id: &str, depth: usize) -> Result<GraphSubset> {
    let focus = load_graph_node(conn, node_id)?;
    let related_ids = collect_neighborhood(conn, node_id, depth)?;
    let edges = load_edges_by_ids(conn, &related_ids)?;
    let mut expanded_ids = related_ids.clone();
    for edge in &edges {
        if let Some(source) = edge.get("source").and_then(serde_json::Value::as_str) {
            expanded_ids.insert(source.to_string());
        }
        if let Some(target) = edge.get("target").and_then(serde_json::Value::as_str) {
            expanded_ids.insert(target.to_string());
        }
    }
    let nodes = load_nodes_by_ids(conn, &expanded_ids)?;
    Ok(GraphSubset {
        focus,
        nodes,
        edges,
    })
}

fn collect_neighborhood(conn: &Connection, node_id: &str, depth: usize) -> Result<HashSet<String>> {
    let mut visited = HashSet::new();
    visited.insert(node_id.to_string());
    let mut frontier = vec![node_id.to_string()];

    for _ in 0..depth {
        if frontier.is_empty() {
            break;
        }
        let neighbors = load_neighbor_ids(conn, &frontier)?;
        let mut next_frontier = Vec::new();
        for neighbor in neighbors {
            if visited.insert(neighbor.clone()) {
                next_frontier.push(neighbor);
            }
        }
        frontier = next_frontier;
    }

    Ok(visited)
}

fn load_neighbor_ids(conn: &Connection, node_ids: &[String]) -> Result<Vec<String>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; node_ids.len()].join(",");
    let sql = format!(
        r#"
        SELECT source_id, target_id
        FROM graph_edges
        WHERE source_id IN ({0}) OR target_id IN ({0})
        "#,
        placeholders
    );
    let mut params: Vec<&str> = node_ids.iter().map(|value| value.as_str()).collect();
    params.extend(node_ids.iter().map(|value| value.as_str()));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut neighbors = Vec::new();
    for row in rows {
        let (source, target) = row?;
        neighbors.push(source);
        neighbors.push(target);
    }
    Ok(neighbors)
}

fn load_nodes_by_ids(
    conn: &Connection,
    node_ids: &HashSet<String>,
) -> Result<Vec<serde_json::Value>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<&str> = node_ids.iter().map(|value| value.as_str()).collect();
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        r#"
        SELECT id, kind, name, COALESCE(path, ''), COALESCE(metadata, '{{}}')
        , span_start, span_end, hash
        FROM graph_nodes
        WHERE id IN ({})
        ORDER BY kind, path, name, id
        "#,
        placeholders
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(ids), |row| {
        let metadata_text: String = row.get(4)?;
        let metadata = serde_json::from_str(&metadata_text).unwrap_or_else(|_| json!({}));
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "kind": row.get::<_, String>(1)?,
            "name": row.get::<_, String>(2)?,
            "path": row.get::<_, String>(3)?,
            "span_start": row.get::<_, Option<i64>>(5)?,
            "span_end": row.get::<_, Option<i64>>(6)?,
            "hash": row.get::<_, Option<String>>(7)?,
            "metadata": metadata
        }))
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn load_edges_by_ids(
    conn: &Connection,
    node_ids: &HashSet<String>,
) -> Result<Vec<serde_json::Value>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<&str> = node_ids.iter().map(|value| value.as_str()).collect();
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        r#"
        SELECT source_id, target_id, kind, weight, confidence
        FROM graph_edges
        WHERE source_id IN ({0}) OR target_id IN ({0})
        ORDER BY source_id, target_id, kind
        "#,
        placeholders
    );
    let mut stmt = conn.prepare(&sql)?;
    let params = ids.iter().copied().chain(ids.iter().copied());
    let rows = stmt.query_map(params_from_iter(params), |row| {
        Ok(json!({
            "source": row.get::<_, String>(0)?,
            "target": row.get::<_, String>(1)?,
            "kind": row.get::<_, String>(2)?,
            "weight": row.get::<_, f64>(3)?,
            "confidence": row.get::<_, f64>(4)?
        }))
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn load_graph_node(conn: &Connection, node_id: &str) -> Result<Option<serde_json::Value>> {
    Ok(conn
        .query_row(
            r#"
            SELECT id, kind, name, COALESCE(path, ''), COALESCE(metadata, '{}')
            , span_start, span_end, hash
            FROM graph_nodes
            WHERE id = ?1
            "#,
            [node_id],
            |row| {
                let metadata_text: String = row.get(4)?;
                let metadata = serde_json::from_str(&metadata_text).unwrap_or_else(|_| json!({}));
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "kind": row.get::<_, String>(1)?,
                    "name": row.get::<_, String>(2)?,
                    "path": row.get::<_, String>(3)?,
                    "span_start": row.get::<_, Option<i64>>(5)?,
                    "span_end": row.get::<_, Option<i64>>(6)?,
                    "hash": row.get::<_, Option<String>>(7)?,
                    "metadata": metadata
                }))
            },
        )
        .ok())
}

fn render_subset_mermaid(subset: &GraphSubset) -> String {
    let mut out = String::from("flowchart TD\n");
    let focus_id = subset
        .focus
        .as_ref()
        .and_then(|focus| focus.get("id"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if let Some(focus) = subset.focus.as_ref() {
        let focus_id = focus
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("focus");
        let focus_kind = focus
            .get("kind")
            .and_then(|value| value.as_str())
            .unwrap_or("Node");
        let focus_name = focus
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or(focus_id);
        out.push_str(&format!(
            "  {}[\"{}: {}\"]\n",
            mermaid_id(focus_id),
            escape_label(focus_kind),
            escape_label(focus_name)
        ));
    }

    for node in &subset.nodes {
        let node_id = node
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let node_kind = node
            .get("kind")
            .and_then(|value| value.as_str())
            .unwrap_or("Node");
        let node_name = node
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or(node_id);
        if !node_id.is_empty() && node_id != focus_id {
            out.push_str(&format!(
                "  {}[\"{}: {}\"]\n",
                mermaid_id(node_id),
                escape_label(node_kind),
                escape_label(node_name)
            ));
        }
    }

    for edge in &subset.edges {
        let source = edge
            .get("source")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let target = edge
            .get("target")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let kind = edge
            .get("kind")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if !source.is_empty() && !target.is_empty() {
            out.push_str(&format!(
                "  {} -->|{}| {}\n",
                mermaid_id(source),
                escape_label(kind),
                mermaid_id(target)
            ));
        }
    }
    out
}

fn mermaid_id(id: &str) -> String {
    let mut output = String::from("n_");
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    output
}

fn escape_label(label: &str) -> String {
    label.replace('"', "'")
}
