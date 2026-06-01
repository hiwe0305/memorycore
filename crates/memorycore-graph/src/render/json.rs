use anyhow::Result;
use rusqlite::Connection;
use serde_json::json;

pub fn render_json(conn: &Connection) -> Result<String> {
    let mut node_stmt = conn.prepare(
        r#"
        SELECT id, kind, name, COALESCE(path, ''), span_start, span_end, COALESCE(metadata, '{}')
        FROM graph_nodes
        ORDER BY kind, path, name
        "#,
    )?;
    let nodes = node_stmt
        .query_map([], |row| {
            let metadata_text: String = row.get(6)?;
            let metadata = serde_json::from_str(&metadata_text).unwrap_or_else(|_| json!({}));
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "kind": row.get::<_, String>(1)?,
                "name": row.get::<_, String>(2)?,
                "path": row.get::<_, String>(3)?,
                "span_start": row.get::<_, Option<i64>>(4)?,
                "span_end": row.get::<_, Option<i64>>(5)?,
                "metadata": metadata
            }))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut edge_stmt = conn.prepare(
        r#"
        SELECT source_id, target_id, kind, weight, confidence
        FROM graph_edges
        ORDER BY source_id, target_id, kind
        "#,
    )?;
    let edges = edge_stmt
        .query_map([], |row| {
            Ok(json!({
                "source": row.get::<_, String>(0)?,
                "target": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
                "weight": row.get::<_, f64>(3)?,
                "confidence": row.get::<_, f64>(4)?
            }))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(serde_json::to_string_pretty(&json!({
        "nodes": nodes,
        "edges": edges
    }))?)
}
