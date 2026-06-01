use crate::model::{GraphEdge, GraphNode};
use crate::parser::{
    extract_rust_call_sites, extract_rust_imports, extract_rust_module_decls, parse_rust_symbols,
};
use crate::store::{upsert_edge, upsert_node};
use anyhow::{bail, Context, Result};
use memorycore_core::append_event;
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Clone, Default)]
pub struct ScanSummary {
    pub folders: usize,
    pub files: usize,
    pub edges: usize,
}

pub fn scan_file(conn: &Connection, project_root: &Path, path: &Path) -> Result<ScanSummary> {
    if !path.is_file() {
        bail!("{} is not a file", path.display());
    }
    let mut summary = ScanSummary::default();
    let project_node = project_node(project_root);
    let file_node = file_node(project_root, path)?;
    let mut rust_imports = Vec::new();
    let mut rust_symbols = Vec::new();
    if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
        rust_imports = extract_rust_imports(project_root, path)?;
        rust_symbols = parse_rust_symbols(project_root, path)?;
    }

    prune_file_graph_for_scan(conn, &file_node, &rust_imports, &rust_symbols)?;

    upsert_node(conn, &project_node)?;
    upsert_node(conn, &file_node)?;
    let edge = contains_edge(&project_node.id, &file_node.id);
    upsert_edge(conn, &edge)?;
    index_file_content(conn, path, file_node.hash.as_deref())?;
    summary.files = 1;
    summary.edges = 1;
    if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
        for symbol in rust_symbols {
            upsert_node(conn, &symbol.node)?;
            upsert_edge(conn, &symbol.defines_edge)?;
            for edge in symbol.extra_edges {
                upsert_edge(conn, &edge)?;
                summary.edges += 1;
            }
            summary.edges += 1;
        }
        for import in &rust_imports {
            upsert_node(conn, &import.node)?;
            upsert_edge(conn, &import.import_edge)?;
            summary.edges += 1;
        }
        summary.edges += resolve_rust_import_edges(conn, &[path.to_path_buf()], rust_imports)?;
        summary.edges += resolve_rust_call_edges(conn, project_root, &[path.to_path_buf()])?;
        summary.edges += resolve_rust_module_edges(conn, project_root, &[path.to_path_buf()])?;
    }
    append_event(
        conn,
        "memorycore-graph",
        "graph_file_scanned",
        &json!({
            "path": file_node.path,
            "files": summary.files,
            "folders": summary.folders,
            "edges": summary.edges
        }),
    )?;
    Ok(summary)
}

pub fn remove_file_content_index(conn: &Connection, path: &Path) -> Result<()> {
    let path_text = path.to_string_lossy().to_string();
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM file_contents WHERE path = ?1",
            [&path_text],
            |row| row.get(0),
        )
        .ok();
    if let Some(id) = id {
        conn.execute("DELETE FROM file_contents_fts WHERE rowid = ?1", [id])?;
        conn.execute("DELETE FROM file_contents WHERE id = ?1", [id])?;
    }
    Ok(())
}

pub fn remove_file_content_tree_index(conn: &Connection, path: &Path) -> Result<()> {
    let path_text = path.to_string_lossy().replace('\\', "/");
    let prefix = format!("{path_text}/%");
    let mut ids = Vec::new();
    let mut stmt = conn.prepare(
        r#"
        SELECT id
        FROM file_contents
        WHERE path = ?1 OR path LIKE ?2
        "#,
    )?;
    let rows = stmt.query_map([&path_text, &prefix], |row| row.get::<_, i64>(0))?;
    for row in rows {
        ids.push(row?);
    }
    for id in ids {
        conn.execute("DELETE FROM file_contents_fts WHERE rowid = ?1", [id])?;
        conn.execute("DELETE FROM file_contents WHERE id = ?1", [id])?;
    }
    Ok(())
}

pub fn remove_file_graph_index(conn: &Connection, project_root: &Path, path: &Path) -> Result<()> {
    let rel_path = relative_path_for_cleanup(project_root, path)?;
    let rel_prefix = format!("{rel_path}/%");
    let mut ids = Vec::new();
    let mut stmt = conn.prepare(
        r#"
        SELECT id
        FROM graph_nodes
        WHERE path = ?1 OR path LIKE ?2
        "#,
    )?;
    let rows = stmt.query_map([&rel_path, &rel_prefix], |row| row.get::<_, String>(0))?;
    for row in rows {
        ids.push(row?);
    }
    if ids.is_empty() {
        return Ok(());
    }

    for id in &ids {
        conn.execute(
            "DELETE FROM graph_edges WHERE source_id = ?1 OR target_id = ?1",
            [id],
        )?;
    }
    for id in ids {
        conn.execute("DELETE FROM graph_nodes WHERE id = ?1", [id])?;
    }
    Ok(())
}

fn relative_path_for_cleanup(project_root: &Path, path: &Path) -> Result<String> {
    let abs_root = canonicalize_existing(project_root)?;
    let candidate = path
        .strip_prefix(&abs_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if candidate.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(candidate)
    }
}

pub fn scan_folder(conn: &Connection, project_root: &Path, folder: &Path) -> Result<ScanSummary> {
    if !folder.is_dir() {
        bail!("{} is not a folder", folder.display());
    }

    let mut summary = ScanSummary::default();
    let project_node = project_node(project_root);
    upsert_node(conn, &project_node)?;

    let root_folder = folder_node(project_root, folder)?;
    upsert_node(conn, &root_folder)?;
    upsert_edge(conn, &contains_edge(&project_node.id, &root_folder.id))?;
    summary.folders += 1;
    summary.edges += 1;

    let mut rust_files = Vec::new();
    let mut rust_imports = Vec::new();
    for entry in WalkDir::new(folder)
        .into_iter()
        .filter_entry(|entry| !is_ignored(entry))
    {
        let entry = entry?;
        let path = entry.path();
        if path == folder {
            continue;
        }

        let parent_id = path
            .parent()
            .map(|parent| {
                let kind = if parent.is_dir() { "folder" } else { "file" };
                node_id(project_root, parent, kind)
            })
            .unwrap_or_else(|| project_node.id.clone());

        if path.is_dir() {
            let node = folder_node(project_root, path)?;
            upsert_node(conn, &node)?;
            upsert_edge(conn, &contains_edge(&parent_id, &node.id))?;
            summary.folders += 1;
            summary.edges += 1;
        } else if path.is_file() {
            if is_probably_binary(path) {
                continue;
            }
            let node = file_node(project_root, path)?;
            let mut imports = Vec::new();
            let mut symbols = Vec::new();
            if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                imports = extract_rust_imports(project_root, path)?;
                symbols = parse_rust_symbols(project_root, path)?;
            }
            prune_file_graph_for_scan(conn, &node, &imports, &symbols)?;
            upsert_node(conn, &node)?;
            upsert_edge(conn, &contains_edge(&parent_id, &node.id))?;
            index_file_content(conn, path, node.hash.as_deref())?;
            summary.files += 1;
            summary.edges += 1;
            if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                rust_files.push(path.to_path_buf());
                for symbol in symbols {
                    upsert_node(conn, &symbol.node)?;
                    upsert_edge(conn, &symbol.defines_edge)?;
                    for edge in symbol.extra_edges {
                        upsert_edge(conn, &edge)?;
                        summary.edges += 1;
                    }
                    summary.edges += 1;
                }
                for import in &imports {
                    upsert_node(conn, &import.node)?;
                    upsert_edge(conn, &import.import_edge)?;
                    summary.edges += 1;
                }
                rust_imports.extend(imports);
            }
        }
    }

    summary.edges += resolve_rust_import_edges(conn, &rust_files, rust_imports)?;
    summary.edges += resolve_rust_call_edges(conn, project_root, &rust_files)?;
    summary.edges += resolve_rust_module_edges(conn, project_root, &rust_files)?;

    append_event(
        conn,
        "memorycore-graph",
        "graph_folder_scanned",
        &json!({
            "path": root_folder.path,
            "files": summary.files,
            "folders": summary.folders,
            "edges": summary.edges
        }),
    )?;
    Ok(summary)
}

fn project_node(project_root: &Path) -> GraphNode {
    let name = project_root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| project_root.display().to_string());
    GraphNode {
        id: "project:root".to_string(),
        kind: "Project".to_string(),
        name,
        path: Some(".".to_string()),
        span_start: None,
        span_end: None,
        hash: None,
        metadata: json!({}),
    }
}

fn folder_node(project_root: &Path, path: &Path) -> Result<GraphNode> {
    let rel = relative_path(project_root, path)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    Ok(GraphNode {
        id: format!("folder:{rel}"),
        kind: "Folder".to_string(),
        name,
        path: Some(rel),
        span_start: None,
        span_end: None,
        hash: None,
        metadata: json!({}),
    })
}

fn file_node(project_root: &Path, path: &Path) -> Result<GraphNode> {
    let rel = relative_path(project_root, path)?;
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| rel.clone());
    Ok(GraphNode {
        id: format!("file:{rel}"),
        kind: "File".to_string(),
        name,
        path: Some(rel),
        span_start: None,
        span_end: None,
        hash: Some(hash),
        metadata: json!({ "size": bytes.len() }),
    })
}

fn contains_edge(source_id: &str, target_id: &str) -> GraphEdge {
    GraphEdge {
        id: format!("edge:{source_id}:contains:{target_id}"),
        source_id: source_id.to_string(),
        target_id: target_id.to_string(),
        kind: "contains".to_string(),
        weight: 1.0,
        confidence: 1.0,
        metadata: json!({}),
    }
}

fn prune_file_graph_for_scan(
    conn: &Connection,
    file_node: &GraphNode,
    imports: &[crate::parser::ParsedImport],
    symbols: &[crate::parser::ParsedSymbol],
) -> Result<()> {
    let Some(path) = file_node.path.as_deref() else {
        return Ok(());
    };

    let mut desired_ids = HashSet::new();
    desired_ids.insert(file_node.id.clone());
    for import in imports {
        desired_ids.insert(import.node.id.clone());
    }
    for symbol in symbols {
        desired_ids.insert(symbol.node.id.clone());
    }

    let mut existing_ids = Vec::new();
    let mut stmt = conn.prepare(
        r#"
        SELECT id
        FROM graph_nodes
        WHERE path = ?1
        "#,
    )?;
    let rows = stmt.query_map([path], |row| row.get::<_, String>(0))?;
    for row in rows {
        existing_ids.push(row?);
    }

    for id in existing_ids {
        if desired_ids.contains(&id) {
            continue;
        }
        conn.execute(
            "DELETE FROM graph_edges WHERE source_id = ?1 OR target_id = ?1",
            [&id],
        )?;
        conn.execute("DELETE FROM graph_nodes WHERE id = ?1", [&id])?;
    }

    for id in desired_ids {
        conn.execute("DELETE FROM graph_edges WHERE source_id = ?1", [&id])?;
    }

    Ok(())
}

fn resolve_rust_call_edges(
    conn: &Connection,
    project_root: &Path,
    rust_files: &[PathBuf],
) -> Result<usize> {
    if rust_files.is_empty() {
        return Ok(0);
    }

    let mut call_sites = Vec::new();
    for path in rust_files {
        call_sites.extend(extract_rust_call_sites(project_root, path)?);
    }

    let symbol_index = load_rust_symbol_index(conn)?;
    let mut existing_call_edges = load_existing_call_edge_ids(conn)?;
    let mut inserted = 0usize;

    for call in call_sites {
        let qualified = if call.caller_namespace.is_empty() {
            call.callee_name.clone()
        } else {
            format!("{}::{}", call.caller_namespace.join("::"), call.callee_name)
        };
        let targets = symbol_index
            .get(&qualified)
            .or_else(|| symbol_index.get(&call.callee_name));
        let Some(targets) = targets else {
            continue;
        };
        for target_id in targets {
            if target_id == &call.caller_id {
                continue;
            }
            let edge_id = format!(
                "edge:{}:calls:{target_id}:{}",
                call.caller_id, call.callee_name
            );
            if existing_call_edges.contains(&edge_id) {
                continue;
            }
            let edge = GraphEdge {
                id: edge_id.clone(),
                source_id: call.caller_id.clone(),
                target_id: target_id.clone(),
                kind: "calls".to_string(),
                weight: 1.0,
                confidence: 0.8,
                metadata: json!({
                    "language": "rust",
                    "callee": call.callee_name,
                    "resolved": target_id
                }),
            };
            upsert_edge(conn, &edge)?;
            existing_call_edges.insert(edge_id);
            inserted += 1;
        }
    }

    Ok(inserted)
}

fn resolve_rust_module_edges(
    conn: &Connection,
    project_root: &Path,
    rust_files: &[PathBuf],
) -> Result<usize> {
    if rust_files.is_empty() {
        return Ok(0);
    }

    let mut inserted = 0usize;
    let mut existing = load_existing_module_edge_ids(conn)?;
    let existing_nodes = load_graph_node_ids(conn)?;
    for path in rust_files {
        for decl in extract_rust_module_decls(project_root, path)? {
            let Some(target_file_id) = decl
                .target_file_ids
                .iter()
                .find(|candidate| existing_nodes.contains(*candidate))
                .cloned()
            else {
                continue;
            };
            let edge_id = format!(
                "edge:{}:declares_module:{}",
                decl.source_file_id, target_file_id
            );
            if existing.contains(&edge_id) {
                continue;
            }
            let edge = GraphEdge {
                id: edge_id.clone(),
                source_id: decl.source_file_id.clone(),
                target_id: target_file_id.clone(),
                kind: "declares_module".to_string(),
                weight: 1.0,
                confidence: 0.75,
                metadata: json!({
                    "language": "rust",
                    "module": decl.module_name,
                    "module_node": decl.module_id,
                    "target_candidates": decl.target_file_ids,
                    "source_file": decl.source_file_id
                }),
            };
            upsert_edge(conn, &edge)?;
            existing.insert(edge_id);
            inserted += 1;
        }
    }

    Ok(inserted)
}

fn resolve_rust_import_edges(
    conn: &Connection,
    _rust_files: &[PathBuf],
    imports: Vec<crate::parser::ParsedImport>,
) -> Result<usize> {
    if imports.is_empty() {
        return Ok(0);
    }

    let existing_nodes = load_graph_node_paths(conn)?;
    let mut existing_edges = load_existing_import_resolution_edge_ids(conn)?;
    let existing_symbol_nodes = load_graph_symbol_nodes_by_file(conn)?;
    let mut existing_symbol_edges = load_existing_import_symbol_resolution_edge_ids(conn)?;
    let mut inserted = 0usize;

    for import in imports {
        let Some(source_file_rel) = import.node.path.as_deref() else {
            continue;
        };
        let Some(target_path) =
            resolve_local_import_target(source_file_rel, &import.import_path, &existing_nodes)
        else {
            continue;
        };
        let target_file_id = format!("file:{target_path}");
        let symbol_candidates = import_symbol_candidates(&import.import_path);
        let edge_id = format!("edge:{}:resolves_import:{}", import.node.id, target_file_id);
        if existing_edges.contains(&edge_id) {
            // Preserve file-level import resolution even if this edge already exists.
        } else {
            let edge = GraphEdge {
                id: edge_id.clone(),
                source_id: import.node.id.clone(),
                target_id: target_file_id.clone(),
                kind: "resolves_import".to_string(),
                weight: 1.0,
                confidence: 0.7,
                metadata: json!({
                    "language": "rust",
                    "import_path": import.import_path,
                }),
            };
            upsert_edge(conn, &edge)?;
            existing_edges.insert(edge_id);
            inserted += 1;
        }

        if let Some(symbol_records) = existing_symbol_nodes.get(&target_path) {
            for symbol in symbol_records {
                if !symbol_matches_import_candidates(&symbol.name, &symbol_candidates) {
                    continue;
                }
                let edge_id = format!(
                    "edge:{}:resolves_import_symbol:{}",
                    import.node.id, symbol.id
                );
                if existing_symbol_edges.contains(&edge_id) {
                    continue;
                }
                let edge = GraphEdge {
                    id: edge_id.clone(),
                    source_id: import.node.id.clone(),
                    target_id: symbol.id.clone(),
                    kind: "resolves_import_symbol".to_string(),
                    weight: 1.0,
                    confidence: 0.7,
                    metadata: json!({
                        "language": "rust",
                        "import_path": import.import_path,
                        "symbol_name": symbol.name,
                        "symbol_kind": symbol.kind,
                        "resolved_file": target_file_id,
                    }),
                };
                upsert_edge(conn, &edge)?;
                existing_symbol_edges.insert(edge_id);
                inserted += 1;
            }
        }
    }

    Ok(inserted)
}

fn load_rust_symbol_index(conn: &Connection) -> Result<HashMap<String, Vec<String>>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn.prepare(
        r#"
        SELECT id, name
        FROM graph_nodes
        WHERE kind = 'Function'
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, name) = row?;
        index.entry(name.clone()).or_default().push(id.clone());
        if let Some(leaf) = name.rsplit("::").next() {
            index.entry(leaf.to_string()).or_default().push(id.clone());
        }
    }
    Ok(index)
}

fn load_existing_call_edge_ids(conn: &Connection) -> Result<HashSet<String>> {
    let mut edges = HashSet::new();
    let mut stmt = conn.prepare("SELECT id FROM graph_edges WHERE kind = 'calls'")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        edges.insert(row?);
    }
    Ok(edges)
}

fn load_existing_module_edge_ids(conn: &Connection) -> Result<HashSet<String>> {
    let mut edges = HashSet::new();
    let mut stmt = conn.prepare("SELECT id FROM graph_edges WHERE kind = 'declares_module'")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        edges.insert(row?);
    }
    Ok(edges)
}

fn load_graph_node_ids(conn: &Connection) -> Result<HashSet<String>> {
    let mut nodes = HashSet::new();
    let mut stmt = conn.prepare("SELECT id FROM graph_nodes")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        nodes.insert(row?);
    }
    Ok(nodes)
}

fn load_graph_node_paths(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut paths = HashMap::new();
    let mut stmt = conn.prepare("SELECT id, path FROM graph_nodes WHERE kind = 'File'")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (id, path) = row?;
        if let Some(path) = path {
            paths.insert(path, id);
        }
    }
    Ok(paths)
}

fn load_existing_import_resolution_edge_ids(conn: &Connection) -> Result<HashSet<String>> {
    let mut edges = HashSet::new();
    let mut stmt = conn.prepare("SELECT id FROM graph_edges WHERE kind = 'resolves_import'")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        edges.insert(row?);
    }
    Ok(edges)
}

fn load_existing_import_symbol_resolution_edge_ids(conn: &Connection) -> Result<HashSet<String>> {
    let mut edges = HashSet::new();
    let mut stmt =
        conn.prepare("SELECT id FROM graph_edges WHERE kind = 'resolves_import_symbol'")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        edges.insert(row?);
    }
    Ok(edges)
}

#[derive(Debug, Clone)]
struct SymbolRecord {
    id: String,
    name: String,
    kind: String,
}

fn load_graph_symbol_nodes_by_file(
    conn: &Connection,
) -> Result<HashMap<String, Vec<SymbolRecord>>> {
    let mut symbols: HashMap<String, Vec<SymbolRecord>> = HashMap::new();
    let mut stmt = conn.prepare(
        r#"
        SELECT id, name, path, kind
        FROM graph_nodes
        WHERE id LIKE 'symbol:%' AND path IS NOT NULL
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, path, name, kind) = row?;
        if let Some(path) = path {
            symbols
                .entry(path)
                .or_default()
                .push(SymbolRecord { id, name, kind });
        }
    }
    Ok(symbols)
}

fn import_symbol_candidates(import_path: &str) -> Vec<String> {
    if import_path.contains("::*") {
        return Vec::new();
    }

    let normalized = normalize_rust_import_path(import_path);
    let normalized = normalized
        .trim_start_matches("crate::")
        .trim_start_matches("self::")
        .trim_start_matches("super::")
        .to_string();
    let parts: Vec<&str> = normalized
        .split("::")
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() < 2 {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    for start in 0..parts.len() {
        let candidate = parts[start..].join("::");
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

fn symbol_matches_import_candidates(symbol_name: &str, candidates: &[String]) -> bool {
    candidates.iter().any(|candidate| {
        symbol_name == candidate || symbol_name.ends_with(&format!("::{candidate}"))
    })
}

fn resolve_local_import_target(
    source_file_rel: &str,
    import_path: &str,
    existing_nodes: &HashMap<String, String>,
) -> Option<String> {
    let normalized = normalize_rust_import_path(import_path);
    let normalized = normalized
        .trim_start_matches("crate::")
        .trim_start_matches("self::")
        .trim_start_matches("super::")
        .replace("::", "/");
    let mut parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let source_dir = Path::new(source_file_rel)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut candidates = Vec::new();
    if parts.len() > 1 {
        candidates.push(parts.join("/"));
        parts.pop();
    }
    candidates.push(parts.join("/"));
    for candidate in candidates {
        for rendered in candidate_file_paths(source_dir, &candidate) {
            if let Some(path) = find_matching_file(existing_nodes, &rendered) {
                return Some(path);
            }
        }
    }
    None
}

fn candidate_file_paths(source_dir: &Path, candidate: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let direct_rs = source_dir.join(candidate).with_extension("rs");
    let direct_mod = source_dir.join(candidate).join("mod.rs");
    let root_rs = Path::new(candidate).with_extension("rs");
    let root_mod = Path::new(candidate).join("mod.rs");
    for path in [
        direct_rs,
        direct_mod,
        root_rs.to_path_buf(),
        root_mod.to_path_buf(),
    ] {
        paths.push(path.to_string_lossy().replace('\\', "/"));
    }
    paths
}

fn find_matching_file(existing_nodes: &HashMap<String, String>, candidate: &str) -> Option<String> {
    if existing_nodes.contains_key(candidate) {
        return Some(candidate.to_string());
    }
    let suffix = format!("/{candidate}");
    let mut matches: Vec<&String> = existing_nodes
        .keys()
        .filter(|path| path.ends_with(&suffix))
        .collect();
    matches.sort_by_key(|path| (path.len(), path.matches('/').count()));
    matches.into_iter().next().cloned()
}

fn normalize_rust_import_path(import_path: &str) -> String {
    let mut normalized = import_path.trim().to_string();
    if let Some((path, _alias)) = normalized.split_once(" as ") {
        normalized = path.trim().to_string();
    }
    if let Some(stripped) = normalized.strip_suffix("::*") {
        normalized = stripped.trim().to_string();
    }
    normalized
}

fn relative_path(project_root: &Path, path: &Path) -> Result<String> {
    let abs_root = canonicalize_existing(project_root)?;
    let abs_path = canonicalize_existing(path)?;
    let rel = abs_path.strip_prefix(&abs_root).unwrap_or(&abs_path);
    let rendered = rel.to_string_lossy().replace('\\', "/");
    Ok(if rendered.is_empty() {
        ".".to_string()
    } else {
        rendered
    })
}

fn canonicalize_existing(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("canonicalize {}", path.display()))
}

fn node_id(project_root: &Path, path: &Path, kind: &str) -> String {
    match relative_path(project_root, path) {
        Ok(rel) => format!("{kind}:{rel}"),
        Err(_) => "project:root".to_string(),
    }
}

fn is_ignored(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    matches!(
        name.as_ref(),
        ".git" | ".memorycore" | "target" | "node_modules" | ".codegraph" | ".codex" | ".agents"
    )
}

fn is_probably_binary(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()).unwrap_or(""),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "pdf" | "zip" | "gz" | "zst" | "db"
    )
}

fn index_file_content(conn: &Connection, path: &Path, hash: Option<&str>) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() > 256 * 1024 {
        return Ok(());
    }
    let Ok(content) = String::from_utf8(bytes) else {
        return Ok(());
    };
    let now = memorycore_core::now_unix();
    let path_text = path.to_string_lossy().to_string();
    let hash_text = hash.unwrap_or_default().to_string();
    conn.execute(
        r#"
        INSERT INTO file_contents (path, content, hash, updated_at)
        VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(path) DO UPDATE SET
            content=excluded.content,
            hash=excluded.hash,
            updated_at=excluded.updated_at
        "#,
        (&path_text, &content, &hash_text, now),
    )?;
    let id: i64 = conn.query_row(
        "SELECT id FROM file_contents WHERE path = ?1",
        [&path_text],
        |row| row.get(0),
    )?;
    conn.execute("DELETE FROM file_contents_fts WHERE rowid = ?1", [id])?;
    conn.execute(
        "INSERT INTO file_contents_fts (rowid, path, content, hash) VALUES (?1, ?2, ?3, ?4)",
        (&id, &path_text, &content, &hash_text),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memorycore_core::{connect_project_db, init_project};
    use tempfile::tempdir;

    #[test]
    fn remove_file_content_tree_index_clears_nested_files() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init project");
        let conn = connect_project_db(temp.path()).expect("db");
        let root = temp.path().join("src");
        let nested = root.join("nested");
        let file = nested.join("main.rs");
        fs::create_dir_all(&nested).expect("create nested dir");
        fs::write(&file, "fn main() {}\n").expect("write file");

        scan_folder(&conn, temp.path(), &root).expect("scan folder");
        remove_file_content_tree_index(&conn, &root).expect("cleanup tree");
        remove_file_graph_index(&conn, temp.path(), &root).expect("cleanup graph tree");

        let content_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_contents WHERE path = ?1 OR path LIKE ?2",
                [
                    root.to_string_lossy().to_string(),
                    format!("{}/%", root.to_string_lossy()),
                ],
                |row| row.get(0),
            )
            .expect("count file contents");
        assert_eq!(content_count, 0);

        let rel_root = root
            .strip_prefix(temp.path())
            .expect("relative root")
            .to_string_lossy()
            .to_string();
        let graph_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE path = ?1 OR path LIKE ?2",
                [rel_root.clone(), format!("{rel_root}/%")],
                |row| row.get(0),
            )
            .expect("count graph nodes");
        assert_eq!(graph_count, 0);
    }
}
