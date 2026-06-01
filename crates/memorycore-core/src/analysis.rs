use crate::{search_hits, SearchHit};
use anyhow::Result;
use rusqlite::{params, params_from_iter, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisNode {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub path: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisMemoryCase {
    pub id: String,
    pub name: String,
    pub summary: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisFileContext {
    pub path: String,
    pub hash: Option<String>,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub target: String,
    pub resolved_node: Option<AnalysisNode>,
    pub graph_nodes: Vec<AnalysisNode>,
    pub graph_edges: Vec<AnalysisEdge>,
    pub search_hits: Vec<SearchHit>,
    pub memory_cases: Vec<AnalysisMemoryCase>,
    pub file_contexts: Vec<AnalysisFileContext>,
}

pub fn analyze_target(
    conn: &Connection,
    target: &str,
    depth: usize,
    limit: usize,
) -> Result<AnalysisReport> {
    let limit = limit.clamp(1, 100);
    let resolved_id = resolve_analysis_target(conn, target)?;
    let resolved_node = resolved_id
        .as_deref()
        .map(|id| load_analysis_node(conn, id))
        .transpose()?
        .flatten();
    let (graph_nodes, graph_edges) = if let Some(node_id) = resolved_id.as_deref() {
        load_analysis_graph(conn, node_id, depth.min(4), limit)?
    } else {
        (Vec::new(), Vec::new())
    };
    let search_hits = search_hits(conn, target, limit, None)?;
    let memory_cases = related_memory_cases(conn, target, resolved_id.as_deref(), limit)?;
    let file_contexts = related_file_contexts(
        conn,
        target,
        resolved_node.as_ref(),
        graph_nodes.as_slice(),
        limit,
    )?;
    Ok(AnalysisReport {
        target: target.to_string(),
        resolved_node,
        graph_nodes,
        graph_edges,
        search_hits,
        memory_cases,
        file_contexts,
    })
}

pub fn format_analysis_report(report: &AnalysisReport) -> String {
    let mut output = format!("# MemoryCore Analysis: {}\n\n", report.target);
    if let Some(node) = &report.resolved_node {
        output.push_str(&format!(
            "Resolved node: {} [{}] {}\n\n",
            node.id,
            node.kind,
            node.path.as_deref().unwrap_or("")
        ));
    } else {
        output.push_str(&format!(
            "⚠ Target '{}' not found in the graph.\n  File may not exist, not indexed, or use an unsupported language.\n  Run `memorycore search \"{}\" --kind File` to find existing files.\n\n",
            report.target,
            report.target.rsplit(|c: char| c == '/' || c == '\\' || c == '.').last().unwrap_or(&report.target)
        ));
    }

    output.push_str(&format!(
        "Graph context: {} nodes, {} edges\n",
        report.graph_nodes.len(),
        report.graph_edges.len()
    ));
    for edge in &report.graph_edges {
        output.push_str(&format!(
            "- {} -{}-> {}\n",
            edge.source, edge.kind, edge.target
        ));
    }
    if report.graph_edges.is_empty() {
        output.push_str("- No graph edges found.\n");
    }

    output.push_str("\nSearch hits:\n");
    for hit in &report.search_hits {
        output.push_str(&format!(
            "- [{}] {} {}\n",
            hit.kind,
            hit.title,
            hit.path.as_deref().unwrap_or("")
        ));
    }
    if report.search_hits.is_empty() {
        output.push_str("- No search hits found.\n");
    }

    output.push_str("\nFile context:\n");
    for context in &report.file_contexts {
        output.push_str(&format!(
            "- {} hash={}\n  {}\n",
            context.path,
            context.hash.as_deref().unwrap_or(""),
            context.snippet.replace('\n', "\n  ")
        ));
    }
    if report.file_contexts.is_empty() {
        output.push_str("- No file context found.\n");
    }

    output.push_str("\nMemory cases:\n");
    for case in &report.memory_cases {
        output.push_str(&format!(
            "- {} name={} target={} summary={}\n",
            case.id,
            case.name,
            case.target.as_deref().unwrap_or(""),
            case.summary.as_deref().unwrap_or("")
        ));
    }
    if report.memory_cases.is_empty() {
        output.push_str("- No related memory cases found.\n");
    }
    output
}

pub fn render_analysis_mermaid(report: &AnalysisReport) -> String {
    let mut output = String::from("flowchart TD\n");
    let target_id = mermaid_id(&format!("target:{}", report.target));
    output.push_str(&format!(
        "  {}[\"Target: {}\"]\n",
        target_id,
        escape_mermaid_label(&report.target)
    ));

    for node in &report.graph_nodes {
        output.push_str(&format!(
            "  {}[\"{}: {}\"]\n",
            mermaid_id(&node.id),
            escape_mermaid_label(&node.kind),
            escape_mermaid_label(&node.name)
        ));
    }
    if let Some(node) = &report.resolved_node {
        output.push_str(&format!(
            "  {} -->|resolves| {}\n",
            target_id,
            mermaid_id(&node.id)
        ));
    }
    for edge in &report.graph_edges {
        output.push_str(&format!(
            "  {} -->|{}| {}\n",
            mermaid_id(&edge.source),
            escape_mermaid_label(&edge.kind),
            mermaid_id(&edge.target)
        ));
    }

    for (index, case) in report.memory_cases.iter().enumerate() {
        let case_id = mermaid_id(&format!("memory:{index}:{}", case.id));
        output.push_str(&format!(
            "  {}[\"Memory: {}\"]\n",
            case_id,
            escape_mermaid_label(&case.name)
        ));
        output.push_str(&format!("  {} -->|memory| {}\n", target_id, case_id));
    }

    if report.graph_edges.is_empty() && report.memory_cases.is_empty() {
        for (index, hit) in report.search_hits.iter().take(8).enumerate() {
            let hit_id = mermaid_id(&format!("hit:{index}:{}:{}", hit.kind, hit.title));
            output.push_str(&format!(
                "  {}[\"{}: {}\"]\n",
                hit_id,
                escape_mermaid_label(&hit.kind),
                escape_mermaid_label(&hit.title)
            ));
            output.push_str(&format!("  {} -->|search| {}\n", target_id, hit_id));
        }
    }

    output
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

fn escape_mermaid_label(label: &str) -> String {
    label.replace('"', "'")
}

fn resolve_analysis_target(conn: &Connection, target: &str) -> Result<Option<String>> {
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
        if let Ok(id) = conn.query_row(sql, [param], |row| row.get(0)) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn load_analysis_graph(
    conn: &Connection,
    node_id: &str,
    depth: usize,
    limit: usize,
) -> Result<(Vec<AnalysisNode>, Vec<AnalysisEdge>)> {
    let mut ids = HashSet::new();
    ids.insert(node_id.to_string());
    let mut frontier = vec![node_id.to_string()];
    for _ in 0..depth {
        if frontier.is_empty() || ids.len() >= limit {
            break;
        }
        let mut next = Vec::new();
        for id in &frontier {
            for neighbor in neighbor_ids(conn, id, limit)? {
                if ids.insert(neighbor.clone()) {
                    next.push(neighbor);
                }
                if ids.len() >= limit {
                    break;
                }
            }
        }
        frontier = next;
    }

    let mut nodes = Vec::new();
    for id in &ids {
        if let Some(node) = load_analysis_node(conn, id)? {
            nodes.push(node);
        }
    }
    nodes.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.name.cmp(&right.name))
    });

    let edges = edges_between_ids(conn, &ids, limit)?;
    Ok((nodes, edges))
}

fn edges_between_ids(
    conn: &Connection,
    node_ids: &HashSet<String>,
    limit: usize,
) -> Result<Vec<AnalysisEdge>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids = node_ids.iter().map(String::as_str).collect::<Vec<_>>();
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        r#"
        SELECT source_id, target_id, kind
        FROM graph_edges
        WHERE source_id IN ({0}) AND target_id IN ({0})
        ORDER BY source_id, target_id, kind
        LIMIT ?{1}
        "#,
        placeholders,
        ids.len() * 2 + 1
    );
    let mut params = ids.clone();
    params.extend(ids.iter().copied());
    let limit_text = limit.to_string();
    params.push(limit_text.as_str());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        Ok(AnalysisEdge {
            source: row.get(0)?,
            target: row.get(1)?,
            kind: row.get(2)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn neighbor_ids(conn: &Connection, node_id: &str, limit: usize) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT source_id, target_id
        FROM graph_edges
        WHERE source_id = ?1 OR target_id = ?1
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![node_id, limit as i64], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (source, target) = row?;
        if source == node_id {
            out.push(target);
        } else {
            out.push(source);
        }
    }
    Ok(out)
}

fn load_analysis_node(conn: &Connection, id: &str) -> Result<Option<AnalysisNode>> {
    let node = conn
        .query_row(
            r#"
            SELECT id, kind, name, path, COALESCE(metadata, '{}')
            FROM graph_nodes
            WHERE id = ?1
            LIMIT 1
            "#,
            [id],
            |row| {
                let metadata_text: String = row.get(4)?;
                let metadata = serde_json::from_str(&metadata_text).unwrap_or_else(|_| json!({}));
                Ok(AnalysisNode {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    name: row.get(2)?,
                    path: row.get(3)?,
                    metadata,
                })
            },
        )
        .ok();
    Ok(node)
}

fn related_memory_cases(
    conn: &Connection,
    target: &str,
    resolved_id: Option<&str>,
    limit: usize,
) -> Result<Vec<AnalysisMemoryCase>> {
    let like = format!("%{target}%");
    let resolved_like = resolved_id.map(|id| format!("%{id}%"));
    let mut stmt = conn.prepare(
        r#"
        SELECT id, name, summary, target
        FROM memory_cases
        WHERE id LIKE ?1 OR name LIKE ?1 OR summary LIKE ?1 OR target LIKE ?1
           OR (?2 IS NOT NULL AND target LIKE ?2)
        ORDER BY updated_at DESC, created_at DESC, id
        LIMIT ?3
        "#,
    )?;
    let rows = stmt.query_map(params![like, resolved_like, limit as i64], |row| {
        Ok(AnalysisMemoryCase {
            id: row.get(0)?,
            name: row.get(1)?,
            summary: row.get(2)?,
            target: row.get(3)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn related_file_contexts(
    conn: &Connection,
    target: &str,
    resolved_node: Option<&AnalysisNode>,
    graph_nodes: &[AnalysisNode],
    limit: usize,
) -> Result<Vec<AnalysisFileContext>> {
    let mut exact_paths = Vec::new();
    let mut prefixes = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut seen_prefixes = HashSet::new();

    if !target.trim().is_empty() {
        push_unique(&mut exact_paths, &mut seen_paths, target.trim());
        if target.contains('/') {
            push_unique(&mut prefixes, &mut seen_prefixes, target.trim());
        }
    }

    for node in resolved_node.into_iter().chain(graph_nodes.iter()) {
        let Some(path) = node.path.as_deref() else {
            continue;
        };
        if node.kind.eq_ignore_ascii_case("Folder") {
            push_unique(&mut prefixes, &mut seen_prefixes, path);
        } else {
            push_unique(&mut exact_paths, &mut seen_paths, path);
        }
    }

    let mut contexts = Vec::new();
    let mut emitted = HashSet::new();
    for path in exact_paths {
        if contexts.len() >= limit {
            break;
        }
        if let Some(context) = load_file_context(conn, &path)? {
            if emitted.insert(context.path.clone()) {
                contexts.push(context);
            }
        }
    }

    for prefix in prefixes {
        if contexts.len() >= limit {
            break;
        }
        let normalized = prefix.trim_end_matches('/');
        if normalized.is_empty() {
            continue;
        }
        let like = format!("{normalized}/%");
        let remaining = limit.saturating_sub(contexts.len());
        let mut stmt = conn.prepare(
            r#"
            SELECT path, hash, content
            FROM file_contents
            WHERE path = ?1 OR path LIKE ?2
            ORDER BY path
            LIMIT ?3
            "#,
        )?;
        let rows = stmt.query_map(params![normalized, like, remaining as i64], |row| {
            Ok(file_context_from_row(
                row.get(0)?,
                row.get(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let context = row?;
            if emitted.insert(context.path.clone()) {
                contexts.push(context);
            }
            if contexts.len() >= limit {
                break;
            }
        }
    }

    Ok(contexts)
}

fn push_unique(values: &mut Vec<String>, seen: &mut HashSet<String>, value: &str) {
    let trimmed = value.trim().trim_start_matches("./").trim_end_matches('/');
    if trimmed.is_empty() {
        return;
    }
    if seen.insert(trimmed.to_string()) {
        values.push(trimmed.to_string());
    }
}

fn load_file_context(conn: &Connection, path: &str) -> Result<Option<AnalysisFileContext>> {
    let context = conn
        .query_row(
            r#"
            SELECT path, hash, content
            FROM file_contents
            WHERE path = ?1
            LIMIT 1
            "#,
            [path],
            |row| {
                Ok(file_context_from_row(
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .ok();
    Ok(context)
}

fn file_context_from_row(
    path: String,
    hash: Option<String>,
    content: String,
) -> AnalysisFileContext {
    AnalysisFileContext {
        path,
        hash,
        snippet: truncate_snippet(&content, 600),
    }
}

fn truncate_snippet(content: &str, max_chars: usize) -> String {
    let trimmed = content.trim();
    let mut snippet = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars {
        snippet.push_str("...");
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connect_project_db, init_project};
    use tempfile::tempdir;

    #[test]
    fn analyzes_target_with_graph_search_and_memory_cases() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        conn.execute(
            r#"
            INSERT INTO graph_nodes (id, kind, name, path, metadata, updated_at)
            VALUES
                ('file:src/main.rs', 'File', 'main.rs', 'src/main.rs', '{}', 1),
                ('symbol:src/main.rs#main', 'Function', 'main', 'src/main.rs', '{}', 1),
                ('symbol:src/main.rs#helper', 'Function', 'helper', 'src/main.rs', '{}', 1)
            "#,
            [],
        )
        .expect("insert nodes");
        conn.execute(
            r#"
            INSERT INTO graph_edges (id, source_id, target_id, kind, updated_at)
            VALUES
                ('edge:file:src/main.rs:defines:symbol:src/main.rs#main', 'file:src/main.rs', 'symbol:src/main.rs#main', 'defines', 1),
                ('edge:symbol:src/main.rs#main:calls:symbol:src/main.rs#helper', 'symbol:src/main.rs#main', 'symbol:src/main.rs#helper', 'calls', 1)
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
        conn.execute(
            r#"
            INSERT INTO file_contents (path, content, hash, updated_at)
            VALUES ('src/main.rs', 'fn main() { helper(); }\nfn helper() {}', 'h1', 1)
            "#,
            [],
        )
        .expect("insert file content");

        let report = analyze_target(&conn, "src/main.rs", 2, 10).expect("analyze");
        assert_eq!(
            report.resolved_node.as_ref().unwrap().id,
            "file:src/main.rs"
        );
        assert_eq!(report.graph_edges.len(), 2);
        assert!(report.graph_edges.iter().any(|edge| edge.kind == "calls"));
        assert!(report.search_hits.iter().any(|hit| hit.kind == "File"));
        assert_eq!(report.memory_cases[0].id, "memory:main");
        assert_eq!(report.file_contexts[0].path, "src/main.rs");
        assert!(report.file_contexts[0].snippet.contains("helper"));

        let formatted = format_analysis_report(&report);
        assert!(formatted.contains("MemoryCore Analysis"));
        assert!(formatted.contains("file:src/main.rs"));
        assert!(formatted.contains("File context"));

        let mermaid = render_analysis_mermaid(&report);
        assert!(mermaid.starts_with("flowchart TD"));
        assert!(mermaid.contains("-->|defines|"));
    }
}
