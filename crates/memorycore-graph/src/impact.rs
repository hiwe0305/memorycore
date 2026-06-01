use anyhow::Result;
use rusqlite::{params_from_iter, Connection};
use std::collections::HashSet;

pub fn find_impact(conn: &Connection, target: &str, limit: usize) -> Result<String> {
    find_impact_with_depth(conn, target, limit, 1)
}

pub fn find_impact_with_depth(
    conn: &Connection,
    target: &str,
    limit: usize,
    depth: usize,
) -> Result<String> {
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

    let Some(target_id) = target_id else {
        return Ok(format!("No graph node found for target {target}"));
    };

    let mut output = format!("impact for {target_id}:\n");
    if depth == 0 || limit == 0 {
        return Ok(output);
    }

    let mut visited_nodes = HashSet::new();
    visited_nodes.insert(target_id.clone());
    let mut frontier = vec![target_id.clone()];
    let mut seen_edges = HashSet::new();
    let mut emitted = 0usize;

    for _ in 0..depth {
        if frontier.is_empty() || emitted >= limit {
            break;
        }
        let placeholders = vec!["?"; frontier.len()].join(",");
        let sql = format!(
            r#"
            SELECT source_id, kind, target_id
            FROM graph_edges
            WHERE source_id IN ({0}) OR target_id IN ({0})
            ORDER BY kind, source_id, target_id
            "#,
            placeholders
        );
        let params: Vec<&str> = frontier.iter().map(|node| node.as_str()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let edges = stmt.query_map(
            params_from_iter(params.iter().copied().chain(params.iter().copied())),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;

        let mut next_frontier = Vec::new();
        for edge in edges {
            if emitted >= limit {
                break;
            }
            let (source, kind, target) = edge?;
            let edge_key = format!("{source}:{kind}:{target}");
            if !seen_edges.insert(edge_key) {
                continue;
            }
            output.push_str(&format!("- {source} -{kind}-> {target}\n"));
            emitted += 1;
            for node in [&source, &target] {
                if visited_nodes.insert(node.clone()) {
                    next_frontier.push(node.clone());
                }
            }
        }
        frontier = next_frontier;
    }

    Ok(output)
}
