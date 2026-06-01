use crate::model::{GraphEdge, GraphNode};
use anyhow::{Context, Result};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone)]
pub struct ParsedSymbol {
    pub node: GraphNode,
    pub defines_edge: GraphEdge,
    pub extra_edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone)]
pub struct ParsedImport {
    pub node: GraphNode,
    pub import_edge: GraphEdge,
    pub import_path: String,
}

#[derive(Debug, Clone)]
pub struct RustModuleDecl {
    pub module_id: String,
    pub source_file_id: String,
    pub module_name: String,
    pub target_file_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RustCallSite {
    pub caller_id: String,
    pub caller_namespace: Vec<String>,
    pub callee_name: String,
}

#[derive(Debug, Clone)]
struct SymbolInfo {
    node: GraphNode,
    defines_edge: GraphEdge,
    leaf_name: String,
    qualified_name: String,
    is_callable: bool,
    extra_edges: Vec<GraphEdge>,
}

pub fn parse_rust_symbols(project_root: &Path, path: &Path) -> Result<Vec<ParsedSymbol>> {
    let (symbols, unresolved_calls) = analyze_rust_source(project_root, path)?;
    Ok(resolve_rust_symbols(symbols, unresolved_calls))
}

pub fn extract_rust_call_sites(project_root: &Path, path: &Path) -> Result<Vec<RustCallSite>> {
    let (_, unresolved_calls) = analyze_rust_source(project_root, path)?;
    Ok(unresolved_calls)
}

pub fn extract_rust_imports(project_root: &Path, path: &Path) -> Result<Vec<ParsedImport>> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut parser = Parser::new();
    parser
        .set_language(tree_sitter_rust::language())
        .context("load tree-sitter-rust language")?;
    let tree = parser.parse(&source, None).context("parse rust source")?;
    let root = tree.root_node();
    let file_rel = relative_path(project_root, path)?;
    let mut imports = Vec::new();
    collect_imports(root, &source, &file_rel, &mut imports)?;
    Ok(imports)
}

pub fn extract_rust_module_decls(project_root: &Path, path: &Path) -> Result<Vec<RustModuleDecl>> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut parser = Parser::new();
    parser
        .set_language(tree_sitter_rust::language())
        .context("load tree-sitter-rust language")?;
    let tree = parser.parse(&source, None).context("parse rust source")?;
    let root = tree.root_node();
    let file_rel = relative_path(project_root, path)?;
    let mut decls = Vec::new();
    collect_module_decls(root, &source, &file_rel, &mut decls)?;
    Ok(decls)
}

fn analyze_rust_source(
    project_root: &Path,
    path: &Path,
) -> Result<(Vec<SymbolInfo>, Vec<RustCallSite>)> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut parser = Parser::new();
    parser
        .set_language(tree_sitter_rust::language())
        .context("load tree-sitter-rust language")?;
    let tree = parser.parse(&source, None).context("parse rust source")?;
    let root = tree.root_node();
    let file_rel = relative_path(project_root, path)?;
    let mut symbols = Vec::new();
    let mut unresolved_calls = Vec::new();
    walk_rust_items(
        root,
        &source,
        &file_rel,
        &mut Vec::new(),
        &mut symbols,
        &mut unresolved_calls,
    )?;
    Ok((symbols, unresolved_calls))
}

fn resolve_rust_symbols(
    symbols: Vec<SymbolInfo>,
    unresolved_calls: Vec<RustCallSite>,
) -> Vec<ParsedSymbol> {
    let symbol_index = build_symbol_index(&symbols);
    let mut edges_by_caller: HashMap<String, Vec<GraphEdge>> = HashMap::new();
    for edge in resolve_call_edges(&unresolved_calls, &symbol_index) {
        edges_by_caller
            .entry(edge.source_id.clone())
            .or_default()
            .push(edge);
    }
    symbols
        .into_iter()
        .map(|symbol| {
            let extra_edges = edges_by_caller.remove(&symbol.node.id).unwrap_or_default();
            let mut combined_edges = symbol.extra_edges;
            combined_edges.extend(extra_edges);
            ParsedSymbol {
                node: symbol.node,
                defines_edge: symbol.defines_edge,
                extra_edges: combined_edges,
            }
        })
        .collect()
}

fn walk_rust_items(
    node: Node,
    source: &str,
    file_rel: &str,
    namespace: &mut Vec<String>,
    output: &mut Vec<SymbolInfo>,
    unresolved_calls: &mut Vec<RustCallSite>,
) -> Result<()> {
    if let Some(item) = classify_item(node, source, file_rel, namespace)? {
        if item.is_callable {
            collect_call_edges(node, source, namespace, &item.node.id, unresolved_calls)?;
        }
        output.push(item);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let pushed = enter_namespace(child, source, namespace)?;
        walk_rust_items(child, source, file_rel, namespace, output, unresolved_calls)?;
        if pushed {
            namespace.pop();
        }
    }
    Ok(())
}

fn classify_item(
    node: Node,
    source: &str,
    file_rel: &str,
    namespace: &[String],
) -> Result<Option<SymbolInfo>> {
    let Some(kind) = (match node.kind() {
        "function_item" => Some("Function"),
        "struct_item" => Some("Struct"),
        "enum_item" => Some("Enum"),
        "trait_item" => Some("Trait"),
        "type_item" => Some("TypeAlias"),
        "mod_item" => Some("Module"),
        "function_signature_item" => Some("Function"),
        _ => None,
    }) else {
        return Ok(None);
    };

    let Some(name_node) = first_named_child_by_kind(node, &["identifier", "type_identifier"])
    else {
        return Ok(None);
    };
    let leaf_name = node_text(name_node, source).unwrap_or_else(|| "anonymous".to_string());
    let qualified_name = if namespace.is_empty() {
        leaf_name.clone()
    } else {
        format!("{}::{leaf_name}", namespace.join("::"))
    };
    let symbol_id = format!("symbol:{file_rel}#{qualified_name}");
    let ast_node = node;
    let graph_node = GraphNode {
        id: symbol_id.clone(),
        kind: kind.to_string(),
        name: qualified_name.clone(),
        path: Some(file_rel.to_string()),
        span_start: Some(ast_node.start_position().row as i64),
        span_end: Some(ast_node.end_position().row as i64),
        hash: None,
        metadata: json!({
            "language": "rust",
            "symbol_kind": ast_node.kind(),
            "leaf_name": leaf_name
        }),
    };
    let defines_edge = GraphEdge {
        id: format!("edge:file:{file_rel}:defines:{symbol_id}"),
        source_id: format!("file:{file_rel}"),
        target_id: symbol_id,
        kind: "defines".to_string(),
        weight: 1.0,
        confidence: 1.0,
        metadata: json!({ "language": "rust" }),
    };
    Ok(Some(SymbolInfo {
        node: graph_node,
        defines_edge,
        leaf_name: leaf_name.clone(),
        qualified_name,
        is_callable: matches!(kind, "Function"),
        extra_edges: Vec::new(),
    }))
}

fn enter_namespace(node: Node, source: &str, namespace: &mut Vec<String>) -> Result<bool> {
    match node.kind() {
        "impl_item" => {
            if let Some(target) = first_named_child_by_kind(node, &["type_identifier"]) {
                if let Some(name) = node_text(target, source) {
                    namespace.push(format!("impl {name}"));
                    return Ok(true);
                }
            }
            namespace.push("impl".to_string());
            Ok(true)
        }
        "mod_item" | "trait_item" => {
            if let Some(name_node) = first_named_child_by_kind(node, &["identifier"]) {
                if let Some(name) = node_text(name_node, source) {
                    namespace.push(name);
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn first_named_child_by_kind<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let child = node
        .named_children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()));
    child
}

fn collect_call_edges(
    node: Node,
    source: &str,
    namespace: &[String],
    caller_id: &str,
    unresolved_calls: &mut Vec<RustCallSite>,
) -> Result<()> {
    if matches!(node.kind(), "call_expression" | "method_call_expression") {
        if let Some(callee_name) = call_target_name(node, source) {
            unresolved_calls.push(RustCallSite {
                caller_id: caller_id.to_string(),
                caller_namespace: namespace.to_vec(),
                callee_name,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_call_edges(child, source, namespace, caller_id, unresolved_calls)?;
    }
    Ok(())
}

fn collect_imports(
    node: Node,
    source: &str,
    file_rel: &str,
    imports: &mut Vec<ParsedImport>,
) -> Result<()> {
    if node.kind() == "use_declaration" {
        if let Some(import_path) = import_path_text(node, source) {
            for expanded_path in expand_import_paths(&import_path) {
                let import_id = format!("import:{file_rel}#{expanded_path}");
                let node = GraphNode {
                    id: import_id.clone(),
                    kind: "Import".to_string(),
                    name: expanded_path.clone(),
                    path: Some(file_rel.to_string()),
                    span_start: Some(node.start_position().row as i64),
                    span_end: Some(node.end_position().row as i64),
                    hash: None,
                    metadata: json!({
                        "language": "rust",
                        "source": node.kind(),
                        "import_path": expanded_path
                    }),
                };
                let import_edge = GraphEdge {
                    id: format!("edge:file:{file_rel}:imports:{import_id}"),
                    source_id: format!("file:{file_rel}"),
                    target_id: import_id,
                    kind: "imports".to_string(),
                    weight: 1.0,
                    confidence: 0.9,
                    metadata: json!({ "language": "rust" }),
                };
                imports.push(ParsedImport {
                    node,
                    import_edge,
                    import_path: expanded_path,
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_imports(child, source, file_rel, imports)?;
    }
    Ok(())
}

fn collect_module_decls(
    node: Node,
    source: &str,
    file_rel: &str,
    decls: &mut Vec<RustModuleDecl>,
) -> Result<()> {
    if node.kind() == "mod_item" && node.child_by_field_name("body").is_none() {
        if let Some(name_node) = first_named_child_by_kind(node, &["identifier"]) {
            if let Some(module_name) = node_text(name_node, source) {
                let parent = Path::new(file_rel)
                    .parent()
                    .unwrap_or_else(|| Path::new(""));
                let module_name_rs = module_name.clone();
                let module_name_mod = module_name.clone();
                decls.push(RustModuleDecl {
                    module_id: format!("symbol:{file_rel}#{module_name}"),
                    source_file_id: format!("file:{file_rel}"),
                    module_name,
                    target_file_ids: vec![
                        format!(
                            "file:{}",
                            parent
                                .join(format!("{module_name_rs}.rs"))
                                .to_string_lossy()
                                .replace('\\', "/")
                        ),
                        format!(
                            "file:{}",
                            parent
                                .join(module_name_mod)
                                .join("mod.rs")
                                .to_string_lossy()
                                .replace('\\', "/")
                        ),
                    ],
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_module_decls(child, source, file_rel, decls)?;
    }
    Ok(())
}

fn import_path_text(node: Node, source: &str) -> Option<String> {
    let raw = node_text(node, source)?;
    let trimmed = raw.trim();
    let use_index = trimmed.find("use ")?;
    let trimmed = &trimmed[use_index + 4..];
    let trimmed = trimmed.trim_end_matches(';').trim();
    Some(trimmed.to_string())
}

fn expand_import_paths(import_path: &str) -> Vec<String> {
    let trimmed = import_path.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if let Some((prefix, suffixes)) = split_first_brace_group(trimmed) {
        let mut expanded = Vec::new();
        for suffix in suffixes {
            for nested in expand_import_paths(&format!("{prefix}{suffix}")) {
                expanded.push(nested);
            }
        }
        return expanded;
    }

    split_top_level_commas(trimmed)
        .into_iter()
        .flat_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                Vec::new()
            } else {
                vec![part.to_string()]
            }
        })
        .collect()
}

fn split_first_brace_group(value: &str) -> Option<(String, Vec<String>)> {
    let mut depth = 0isize;
    let mut start = None;
    let mut end = None;
    for (idx, ch) in value.char_indices() {
        match ch {
            '{' => {
                if depth == 0 {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(idx);
                    break;
                }
            }
            _ => {}
        }
    }
    let (Some(start), Some(end)) = (start, end) else {
        return None;
    };
    let prefix = value[..start].to_string();
    let inner = &value[start + 1..end];
    let suffix = value[end + 1..].to_string();
    let suffixes = split_top_level_commas(inner)
        .into_iter()
        .map(|part| {
            let trimmed = part.trim();
            format!("{trimmed}{suffix}")
        })
        .collect();
    Some((prefix, suffixes))
}

fn split_top_level_commas(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0isize;
    let mut start = 0usize;
    for (idx, ch) in value.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(value[start..idx].to_string());
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(value[start..].to_string());
    parts
}

fn call_target_name(node: Node, source: &str) -> Option<String> {
    let function = match node.kind() {
        "method_call_expression" => node
            .child_by_field_name("method")
            .or_else(|| first_named_child_by_kind(node, &["identifier"])),
        _ => node.child_by_field_name("function").or_else(|| {
            first_named_child_by_kind(
                node,
                &[
                    "identifier",
                    "field_expression",
                    "scoped_identifier",
                    "generic_function",
                ],
            )
        }),
    };
    function.and_then(|function| terminal_identifier(function, source))
}

fn terminal_identifier(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" | "type_identifier" | "field_identifier" => node_text(node, source),
        _ => {
            let mut cursor = node.walk();
            let mut last = None;
            for child in node.named_children(&mut cursor) {
                if let Some(name) = terminal_identifier(child, source) {
                    last = Some(name);
                }
            }
            last
        }
    }
}

fn build_symbol_index(symbols: &[SymbolInfo]) -> HashMap<String, Vec<String>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();
    for symbol in symbols {
        index
            .entry(symbol.qualified_name.clone())
            .or_default()
            .push(symbol.node.id.clone());
        index
            .entry(symbol.leaf_name.clone())
            .or_default()
            .push(symbol.node.id.clone());
    }
    index
}

fn resolve_call_edges(
    calls: &[RustCallSite],
    index: &HashMap<String, Vec<String>>,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    for call in calls {
        let qualified = if call.caller_namespace.is_empty() {
            call.callee_name.clone()
        } else {
            format!("{}::{}", call.caller_namespace.join("::"), call.callee_name)
        };
        let targets = index
            .get(&qualified)
            .or_else(|| index.get(&call.callee_name));
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
            if !seen.insert(edge_id.clone()) {
                continue;
            }
            edges.push(GraphEdge {
                id: edge_id,
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
            });
        }
    }
    edges
}

fn node_text(node: Node, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes())
        .ok()
        .map(|s| s.trim().to_string())
}

fn relative_path(project_root: &Path, path: &Path) -> Result<String> {
    let abs_root = project_root.canonicalize()?;
    let abs_path = path.canonicalize()?;
    let rel = abs_path.strip_prefix(&abs_root).unwrap_or(&abs_path);
    Ok(rel.to_string_lossy().replace('\\', "/"))
}
