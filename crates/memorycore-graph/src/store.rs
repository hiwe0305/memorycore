use crate::model::{GraphEdge, GraphNode};
use anyhow::Result;
use memorycore_core::now_unix;
use rusqlite::{params, Connection};

pub fn upsert_node(conn: &Connection, node: &GraphNode) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO graph_nodes
            (id, kind, name, path, span_start, span_end, hash, metadata, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(id) DO UPDATE SET
            kind=excluded.kind,
            name=excluded.name,
            path=excluded.path,
            span_start=excluded.span_start,
            span_end=excluded.span_end,
            hash=excluded.hash,
            metadata=excluded.metadata,
            updated_at=excluded.updated_at
        "#,
        params![
            &node.id,
            &node.kind,
            &node.name,
            &node.path,
            &node.span_start,
            &node.span_end,
            &node.hash,
            node.metadata.to_string(),
            now_unix()
        ],
    )?;
    Ok(())
}

pub fn upsert_edge(conn: &Connection, edge: &GraphEdge) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO graph_edges
            (id, source_id, target_id, kind, weight, confidence, metadata, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(id) DO UPDATE SET
            source_id=excluded.source_id,
            target_id=excluded.target_id,
            kind=excluded.kind,
            weight=excluded.weight,
            confidence=excluded.confidence,
            metadata=excluded.metadata,
            updated_at=excluded.updated_at
        "#,
        params![
            &edge.id,
            &edge.source_id,
            &edge.target_id,
            &edge.kind,
            edge.weight,
            edge.confidence,
            edge.metadata.to_string(),
            now_unix()
        ],
    )?;
    Ok(())
}
