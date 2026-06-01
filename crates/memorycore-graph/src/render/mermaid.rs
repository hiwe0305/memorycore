use anyhow::Result;
use rusqlite::Connection;

pub fn render_mermaid(conn: &Connection) -> Result<String> {
    let mut out = String::from("flowchart TD\n");
    let mut stmt = conn.prepare(
        r#"
        SELECT id, kind, name
        FROM graph_nodes
        ORDER BY kind, path, name
        "#,
    )?;
    let nodes = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    for node in nodes {
        let (id, kind, name) = node?;
        out.push_str(&format!(
            "  {}[\"{}: {}\"]\n",
            mermaid_id(&id),
            escape_label(&kind),
            escape_label(&name)
        ));
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT source_id, target_id, kind
        FROM graph_edges
        ORDER BY source_id, target_id, kind
        "#,
    )?;
    let edges = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    for edge in edges {
        let (source, target, kind) = edge?;
        out.push_str(&format!(
            "  {} -->|{}| {}\n",
            mermaid_id(&source),
            escape_label(&kind),
            mermaid_id(&target)
        ));
    }
    Ok(out)
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
