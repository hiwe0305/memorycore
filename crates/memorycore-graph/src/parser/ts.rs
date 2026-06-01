use crate::model::{GraphEdge, GraphNode};
use anyhow::{Context, Result};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tree_sitter::Parser;

#[derive(Debug, Clone)]
pub struct ParsedTsSymbol {
    pub node: GraphNode,
    pub defines_edge: GraphEdge,
    pub extra_edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone)]
pub struct ParsedTsImport {
    pub node: GraphNode,
    pub import_edge: GraphEdge,
    pub import_path: String,
}

#[derive(Debug, Clone)]
pub struct TsCallSite {
    pub caller_id: String,
    pub callee_name: String,
}

fn tsx_language() -> tree_sitter::Language {
    tree_sitter_typescript::language_tsx()
}

fn ts_language() -> tree_sitter::Language {
    tree_sitter_typescript::language_typescript()
}

fn select_language(path: &Path) -> tree_sitter::Language {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".tsx") { tsx_language() } else { ts_language() }
}

fn relative_path(project_root: &Path, path: &Path) -> Result<String> {
    let abs_root = project_root.canonicalize()?;
    let abs_path = path.canonicalize()?;
    let rel = abs_path.strip_prefix(&abs_root).unwrap_or(&abs_path);
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

fn node_name(node: tree_sitter::Node, source: &str) -> Option<String> {
    if let Some(name_node) = node.child_by_field_name("name") {
        return Some(name_node.utf8_text(source.as_bytes()).ok()?.to_string());
    }
    // fallback: try first child that looks like an identifier
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "property_identifier" {
            return Some(child.utf8_text(source.as_bytes()).ok()?.to_string());
        }
    }
    None
}

pub fn parse_ts_symbols(project_root: &Path, path: &Path) -> Result<Vec<ParsedTsSymbol>> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut parser = Parser::new();
    parser.set_language(select_language(path))
        .context("load tree-sitter-typescript language")?;
    let tree = parser.parse(&source, None)
        .context("parse typescript source")?;
    let root = tree.root_node();
    let file_rel = relative_path(project_root, path)?;

    let mut symbols = Vec::new();
    let mut call_sites = Vec::new();
    walk_ts_items(root, &source, &file_rel, &mut Vec::new(), &mut symbols, &mut call_sites)?;

    let symbol_index = build_ts_symbol_index(&symbols);
    let mut edges_by_caller: HashMap<String, Vec<GraphEdge>> = HashMap::new();
    for edge in resolve_ts_call_edges(&call_sites, &symbol_index) {
        edges_by_caller.entry(edge.source_id.clone()).or_default().push(edge);
    }

    Ok(symbols.into_iter().map(|sym| {
        let extra = edges_by_caller.remove(&sym.node.id).unwrap_or_default();
        ParsedTsSymbol { node: sym.node, defines_edge: sym.defines_edge, extra_edges: extra }
    }).collect())
}

pub fn extract_ts_imports(project_root: &Path, path: &Path) -> Result<Vec<ParsedTsImport>> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut parser = Parser::new();
    parser.set_language(select_language(path))
        .context("load tree-sitter-typescript language")?;
    let tree = parser.parse(&source, None)
        .context("parse typescript source")?;
    let root = tree.root_node();
    let file_rel = relative_path(project_root, path)?;

    let mut imports = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        collect_ts_import(child, &source, &file_rel, &mut imports);
    }
    Ok(imports)
}

pub fn extract_ts_call_sites(project_root: &Path, path: &Path) -> Result<Vec<TsCallSite>> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut parser = Parser::new();
    parser.set_language(select_language(path))
        .context("load tree-sitter-typescript language")?;
    let tree = parser.parse(&source, None)
        .context("parse typescript source")?;
    let root = tree.root_node();
    let file_rel = relative_path(project_root, path)?;

    let mut symbols = Vec::new();
    let mut call_sites = Vec::new();
    walk_ts_items(root, &source, &file_rel, &mut Vec::new(), &mut symbols, &mut call_sites)?;
    Ok(call_sites)
}

struct TsItem {
    node: GraphNode,
    defines_edge: GraphEdge,
    is_callable: bool,
}

fn walk_ts_items(
    node: tree_sitter::Node,
    source: &str,
    file_rel: &str,
    namespace: &mut Vec<String>,
    output: &mut Vec<TsItem>,
    call_sites: &mut Vec<TsCallSite>,
) -> Result<()> {
    if let Some(item) = classify_ts_item(node, source, file_rel, namespace) {
        if item.is_callable {
            collect_ts_calls_in_node(node, source, &item.node.id, call_sites);
        }
        output.push(item);
    }

    let push_ns = matches!(node.kind(), "class_declaration" | "object");

    if push_ns {
        if let Some(name) = node_name(node, source) {
            namespace.push(name);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_ts_items(child, source, file_rel, namespace, output, call_sites)?;
    }

    if push_ns && !namespace.is_empty() {
        namespace.pop();
    }
    Ok(())
}

fn classify_ts_item(
    node: tree_sitter::Node,
    source: &str,
    file_rel: &str,
    namespace: &[String],
) -> Option<TsItem> {
    let kind = match node.kind() {
        "function_declaration" | "arrow_function" => Some("Function"),
        "method_definition" => Some("Method"),
        "class_declaration" => Some("Class"),
        "interface_declaration" => Some("Interface"),
        "type_alias_declaration" => Some("TypeAlias"),
        "enum_declaration" => Some("Enum"),
        "lexical_declaration" | "variable_declaration" => {
            return extract_ts_named_var(node, source, file_rel, namespace);
        }
        _ => None,
    }?;

    let leaf_name = node_name(node, source)?;
    let qualified = if namespace.is_empty() {
        leaf_name.clone()
    } else {
        format!("{}::{}", namespace.join("::"), leaf_name)
    };
    let symbol_id = format!("symbol:{file_rel}#{qualified}");

    let graph_node = GraphNode {
        id: symbol_id.clone(),
        kind: kind.to_string(),
        name: qualified.clone(),
        path: Some(file_rel.to_string()),
        span_start: Some(node.start_position().row as i64),
        span_end: Some(node.end_position().row as i64),
        hash: None,
        metadata: json!({ "language": "typescript", "leaf_name": leaf_name }),
    };

    let defines_edge = GraphEdge {
        id: format!("edge:file:{file_rel}:defines:{symbol_id}"),
        source_id: format!("file:{file_rel}"),
        target_id: symbol_id,
        kind: "defines".to_string(),
        weight: 1.0,
        confidence: 1.0,
        metadata: json!({ "language": "typescript" }),
    };

    Some(TsItem { node: graph_node, defines_edge, is_callable: matches!(kind, "Function" | "Method") })
}

fn extract_ts_named_var(
    node: tree_sitter::Node,
    source: &str,
    file_rel: &str,
    namespace: &[String],
) -> Option<TsItem> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let leaf_name = node_name(child, source)?;
            let qualified = if namespace.is_empty() {
                leaf_name.clone()
            } else {
                format!("{}::{}", namespace.join("::"), leaf_name)
            };
            let symbol_id = format!("symbol:{file_rel}#{qualified}");
            let graph_node = GraphNode {
                id: symbol_id.clone(),
                kind: "Variable".to_string(),
                name: qualified.clone(),
                path: Some(file_rel.to_string()),
                span_start: Some(node.start_position().row as i64),
                span_end: Some(node.end_position().row as i64),
                hash: None,
                metadata: json!({ "language": "typescript", "leaf_name": leaf_name }),
            };
            let defines_edge = GraphEdge {
                id: format!("edge:file:{file_rel}:defines:{symbol_id}"),
                source_id: format!("file:{file_rel}"),
                target_id: symbol_id,
                kind: "defines".to_string(),
                weight: 1.0,
                confidence: 1.0,
                metadata: json!({ "language": "typescript" }),
            };
            let is_callable = child.child_by_field_name("value")
                .map(|v| v.kind() == "arrow_function")
                .unwrap_or(false);
            return Some(TsItem { node: graph_node, defines_edge, is_callable });
        }
    }
    None
}

fn collect_ts_import(
    node: tree_sitter::Node,
    source: &str,
    file_rel: &str,
    output: &mut Vec<ParsedTsImport>,
) {
    let (module, source_clause) = match node.kind() {
        "import_statement" => {
            let src = node.child_by_field_name("source")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(|s| s.trim_matches(&['"', '\''][..]).to_string());
            (None, src)
        }
        "import_require" => {
            let src = node.child_by_field_name("source")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(|s| s.trim_matches(&['"', '\''][..]).to_string());
            (None, src)
        }
        _ => return,
    };

    let import_path = source_clause.clone().unwrap_or_default();
    let source_clause = source_clause.unwrap_or_default();
    let module_name = module.clone().unwrap_or_else(|| {
        source_clause.rsplit('/').next().unwrap_or(&source_clause)
            .split('.').next().unwrap_or(&source_clause)
            .to_string()
    });

    let import_id = format!("import:{file_rel}#{module_name}");
    let import_node = GraphNode {
        id: import_id.clone(),
        kind: "Import".to_string(),
        name: module_name.clone(),
        path: Some(file_rel.to_string()),
        span_start: Some(node.start_position().row as i64),
        span_end: Some(node.end_position().row as i64),
        hash: None,
        metadata: json!({ "language": "typescript", "import_path": import_path }),
    };

    let import_edge = GraphEdge {
        id: format!("edge:file:{file_rel}:imports:{import_id}"),
        source_id: format!("file:{file_rel}"),
        target_id: import_id,
        kind: "imports".to_string(),
        weight: 1.0,
        confidence: 1.0,
        metadata: json!({ "language": "typescript" }),
    };

    output.push(ParsedTsImport { node: import_node, import_edge, import_path });
}

fn collect_ts_calls_in_node(
    node: tree_sitter::Node,
    source: &str,
    caller_id: &str,
    output: &mut Vec<TsCallSite>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression" {
            if let Some(func) = child.child_by_field_name("function") {
                let name = func.utf8_text(source.as_bytes()).ok()
                    .map(|s| s.to_string());
                if let Some(name) = name {
                    output.push(TsCallSite { caller_id: caller_id.to_string(), callee_name: name });
                }
            }
        }
        collect_ts_calls_in_node(child, source, caller_id, output);
    }
}

fn build_ts_symbol_index(symbols: &[TsItem]) -> HashMap<String, String> {
    let mut index = HashMap::new();
    for sym in symbols {
        index.insert(sym.node.name.clone(), sym.node.id.clone());
    }
    index
}

fn resolve_ts_call_edges(
    call_sites: &[TsCallSite],
    symbol_index: &HashMap<String, String>,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    for site in call_sites {
        let callee_id = symbol_index.get(&site.callee_name).cloned()
            .or_else(|| {
                // try matching last segment (e.g. "auth::login" -> match "login")
                let candidates: Vec<&String> = symbol_index.keys()
                    .filter(|k| k.ends_with(&format!("::{}", site.callee_name)) || **k == site.callee_name)
                    .collect();
                candidates.first().and_then(|k| symbol_index.get(*k).cloned())
            });
        if let Some(callee_id) = callee_id {
            edges.push(GraphEdge {
                id: format!("edge:{}:calls:{}", &site.caller_id, &callee_id),
                source_id: site.caller_id.clone(),
                target_id: callee_id,
                kind: "calls".to_string(),
                weight: 1.0,
                confidence: 0.9,
                metadata: json!({ "language": "typescript" }),
            });
        }
    }
    edges
}
