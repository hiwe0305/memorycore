use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub kind: String,
    pub title: String,
    pub path: Option<String>,
    pub node_id: Option<String>,
    pub snippet: Option<String>,
    pub snapshot_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSurfaceCount {
    pub surface: String,
    pub count: usize,
}

pub fn search_hits(
    conn: &Connection,
    query: &str,
    limit: usize,
    kind_filter: Option<&str>,
) -> Result<Vec<SearchHit>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let allowed_kinds = parse_kind_filter(kind_filter);
    let like = format!("%{query}%");
    let phrase = format!("\"{}\"", query.replace('"', "\"\""));
    let mut hits = Vec::new();

    let mut stmt = conn.prepare(
        r#"
        SELECT path, snippet(file_contents_fts, 1, '[', ']', '…', 8)
        FROM file_contents_fts
        WHERE file_contents_fts MATCH ?1
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![phrase, limit], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (path, snippet) = row?;
        let hit = SearchHit {
            kind: "File".to_string(),
            title: path.clone(),
            path: Some(path),
            node_id: None,
            snippet: Some(snippet),
            snapshot_hash: None,
        };
        if kind_allowed(&hit.kind, allowed_kinds.as_ref()) {
            hits.push(hit);
        }
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT id, kind, name, COALESCE(path, '')
        FROM graph_nodes
        WHERE name LIKE ?1 OR path LIKE ?1 OR id LIKE ?1
        ORDER BY updated_at DESC
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![like, limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, kind, name, path) = row?;
        let hit = SearchHit {
            kind,
            title: name,
            path: Some(path),
            node_id: Some(id),
            snippet: None,
            snapshot_hash: None,
        };
        if kind_allowed(&hit.kind, allowed_kinds.as_ref()) {
            hits.push(hit);
        }
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT session_id, role, content
        FROM messages_fts
        WHERE messages_fts MATCH ?1
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![phrase, limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (session_id, role, content) = row?;
        let hit = SearchHit {
            kind: "Message".to_string(),
            title: format!("{session_id} {role}"),
            path: Some(session_id),
            node_id: None,
            snippet: Some(content),
            snapshot_hash: None,
        };
        if kind_allowed(&hit.kind, allowed_kinds.as_ref()) {
            hits.push(hit);
        }
    }
    if hits.iter().all(|hit| hit.kind != "Message") {
        let mut stmt = conn.prepare(
            r#"
            SELECT session_id, role, content
            FROM messages
            WHERE session_id LIKE ?1 OR role LIKE ?1 OR content LIKE ?1
            ORDER BY timestamp DESC, id DESC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![like, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (session_id, role, content) = row?;
            let hit = SearchHit {
                kind: "Message".to_string(),
                title: format!("{session_id} {role}"),
                path: Some(session_id),
                node_id: None,
                snippet: Some(content),
                snapshot_hash: None,
            };
            if kind_allowed(&hit.kind, allowed_kinds.as_ref()) {
                hits.push(hit);
            }
        }
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT id, name, COALESCE(summary, ''), COALESCE(target, '')
        FROM memory_cases
        WHERE id LIKE ?1 OR name LIKE ?1 OR summary LIKE ?1 OR target LIKE ?1
        ORDER BY updated_at DESC
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![like, limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, name, summary, target) = row?;
        let hit = SearchHit {
            kind: "MemoryCase".to_string(),
            title: name,
            path: Some(target),
            node_id: Some(id),
            snippet: if summary.is_empty() {
                None
            } else {
                Some(summary)
            },
            snapshot_hash: None,
        };
        if kind_allowed(&hit.kind, allowed_kinds.as_ref()) {
            hits.push(hit);
        }
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT event_type, source, event_data
        FROM event_log
        WHERE event_type LIKE ?1 OR source LIKE ?1 OR event_data LIKE ?1
        ORDER BY id DESC
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![like, limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (event_type, source, event_data) = row?;
        let hit = SearchHit {
            kind: "Event".to_string(),
            title: event_type,
            path: Some(source),
            node_id: None,
            snippet: Some(event_data),
            snapshot_hash: None,
        };
        if kind_allowed(&hit.kind, allowed_kinds.as_ref()) {
            hits.push(hit);
        }
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT id, agent, name, COALESCE(session_dir, ''), COALESCE(command, '')
        FROM adapters
        WHERE id LIKE ?1 OR agent LIKE ?1 OR name LIKE ?1 OR session_dir LIKE ?1 OR command LIKE ?1
        ORDER BY updated_at DESC, id
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![like, limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for row in rows {
        let (id, agent, name, session_dir, command) = row?;
        let hit = SearchHit {
            kind: "Adapter".to_string(),
            title: name,
            path: if session_dir.is_empty() {
                None
            } else {
                Some(session_dir)
            },
            node_id: Some(format!("adapter:{id}")),
            snippet: Some(format!("{agent} {}", command).trim().to_string()),
            snapshot_hash: None,
        };
        if kind_allowed(&hit.kind, allowed_kinds.as_ref()) {
            hits.push(hit);
        }
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT hash, timestamp, COALESCE(message, ''), COALESCE(file_count, 0), COALESCE(total_size, 0)
        FROM snapshots
        WHERE hash LIKE ?1 OR message LIKE ?1
        ORDER BY timestamp DESC, hash DESC
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![like, limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    for row in rows {
        let (hash, _timestamp, message, file_count, total_size) = row?;
        let hit = SearchHit {
            kind: "Snapshot".to_string(),
            title: message.clone(),
            path: Some(hash.clone()),
            node_id: Some(format!("snapshot:{hash}")),
            snippet: Some(format!("{file_count} files, {total_size} bytes")),
            snapshot_hash: Some(hash),
        };
        if kind_allowed(&hit.kind, allowed_kinds.as_ref()) {
            hits.push(hit);
        }
    }

    normalize_hits(&mut hits);
    Ok(hits)
}

pub fn search_surface_counts(conn: &Connection, query: &str) -> Result<Vec<SearchSurfaceCount>> {
    let mut counts = Vec::new();
    let surfaces = [
        ("graph", None),
        ("files", Some("File,Folder,Project")),
        ("plugins", Some("Plugin")),
        ("skills", Some("Skill")),
        ("adapters", Some("Adapter")),
        ("memory", Some("MemoryCase")),
        ("sessions", Some("Session,Message")),
    ];
    for (surface, kind_filter) in surfaces {
        let count = search_hits(conn, query, i64::MAX as usize, kind_filter)?.len();
        counts.push(SearchSurfaceCount {
            surface: surface.to_string(),
            count,
        });
    }
    Ok(counts)
}

fn normalize_hits(hits: &mut Vec<SearchHit>) {
    hits.sort_by(|left, right| {
        search_hit_rank(left)
            .cmp(&search_hit_rank(right))
            .then_with(|| search_hit_key(left).cmp(&search_hit_key(right)))
    });
    hits.dedup_by(|left, right| search_hit_key(left) == search_hit_key(right));
}

fn search_hit_rank(hit: &SearchHit) -> usize {
    match hit.kind.as_str() {
        "File" => 0,
        "Snapshot" => 1,
        "MemoryCase" => 2,
        "Message" => 3,
        "Event" => 4,
        "Adapter" => 5,
        _ => 6,
    }
}

fn search_hit_key(hit: &SearchHit) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        hit.kind,
        hit.title,
        hit.path.as_deref().unwrap_or(""),
        hit.node_id.as_deref().unwrap_or(""),
        hit.snapshot_hash.as_deref().unwrap_or("")
    )
}

fn parse_kind_filter(kind_filter: Option<&str>) -> Option<HashSet<String>> {
    let mut kinds = HashSet::new();
    let filter = kind_filter?.trim();
    if filter.is_empty() {
        return None;
    }
    for kind in filter.split(',') {
        let kind = kind.trim();
        if !kind.is_empty() {
            kinds.insert(kind.to_string());
        }
    }
    if kinds.is_empty() {
        None
    } else {
        Some(kinds)
    }
}

fn kind_allowed(kind: &str, allowed: Option<&HashSet<String>>) -> bool {
    match allowed {
        Some(kinds) => kinds.contains(kind),
        None => true,
    }
}

pub fn format_search_hits(hits: &[SearchHit]) -> String {
    let mut output = String::new();
    for hit in hits {
        match hit.kind.as_str() {
            "File" => {
                output.push_str(&format!(
                    "File: {} {}\n",
                    hit.path.as_deref().unwrap_or(""),
                    hit.snippet.as_deref().unwrap_or("")
                ));
            }
            "Message" => {
                output.push_str(&format!(
                    "Message: {} {}\n",
                    hit.path.as_deref().unwrap_or(""),
                    hit.snippet.as_deref().unwrap_or("")
                ));
            }
            "MemoryCase" => {
                output.push_str(&format!(
                    "MemoryCase: {} [{}] {} {}\n",
                    hit.title,
                    hit.node_id.as_deref().unwrap_or(""),
                    hit.snippet.as_deref().unwrap_or(""),
                    hit.path.as_deref().unwrap_or("")
                ));
            }
            "Event" => {
                output.push_str(&format!(
                    "Event: {} [{}] {}\n",
                    hit.title,
                    hit.path.as_deref().unwrap_or(""),
                    hit.snippet.as_deref().unwrap_or("")
                ));
            }
            "Snapshot" => {
                output.push_str(&format!(
                    "Snapshot: {} [{}] {}\n",
                    hit.title,
                    hit.node_id.as_deref().unwrap_or(""),
                    hit.snippet.as_deref().unwrap_or("")
                ));
            }
            "Adapter" => {
                output.push_str(&format!(
                    "Adapter: {} [{}] {} {}\n",
                    hit.title,
                    hit.node_id.as_deref().unwrap_or(""),
                    hit.snippet.as_deref().unwrap_or(""),
                    hit.path.as_deref().unwrap_or("")
                ));
            }
            _ => {
                output.push_str(&format!(
                    "{}: {} ({})\n",
                    hit.kind,
                    hit.title,
                    hit.path.as_deref().unwrap_or("")
                ));
            }
        }
    }
    if output.is_empty() {
        output.push_str("No MemoryCore matches found.\n");
    }
    output
}
