use crate::model::{GraphEdge, GraphNode};
use anyhow::{Context, Result};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone)]
pub struct ParsedJsSymbol {
    pub node: GraphNode,
    pub defines_edge: GraphEdge,
    pub extra_edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone)]
pub struct ParsedJsImport {
    pub node: GraphNode,
    pub import_edge: GraphEdge,
    pub import_path: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct JsCallSite {
    pub caller_id: String,
    pub callee_name: String,
}

fn js_language() -> tree_sitter::Language {
    tree_sitter_javascript::language()
}

pub fn parse_js_symbols(project_root: &Path, path: &Path) -> Result<Vec<ParsedJsSymbol>> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut parser = Parser::new();
    parser
        .set_language(js_language())
        .context("load tree-sitter-javascript language")?;
    let tree = parser
        .parse(&source, None)
        .context("parse javascript source")?;
    let root = tree.root_node();
    let file_rel = relative_path(project_root, path)?;

    let mut symbols = Vec::new();
    let mut call_sites = Vec::new();
    walk_js_items(
        root,
        &source,
        &file_rel,
        &mut Vec::new(),
        &mut symbols,
        &mut call_sites,
    )?;

    let symbol_index = build_js_symbol_index(&symbols);
    let mut edges_by_caller: HashMap<String, Vec<GraphEdge>> = HashMap::new();
    for edge in resolve_js_call_edges(&call_sites, &symbol_index) {
        edges_by_caller
            .entry(edge.source_id.clone())
            .or_default()
            .push(edge);
    }

    Ok(symbols
        .into_iter()
        .map(|sym| {
            let extra = edges_by_caller.remove(&sym.node.id).unwrap_or_default();
            ParsedJsSymbol {
                node: sym.node,
                defines_edge: sym.defines_edge,
                extra_edges: extra,
            }
        })
        .collect())
}

pub fn extract_js_call_sites(project_root: &Path, path: &Path) -> Result<Vec<JsCallSite>> {
    let source = fs::read_to_string(path)?;
    let mut parser = Parser::new();
    parser.set_language(js_language())?;
    let tree = parser.parse(&source, None).context("parse javascript")?;
    let root = tree.root_node();
    let file_rel = relative_path(project_root, path)?;
    let mut call_sites = Vec::new();
    collect_js_calls(root, &source, &file_rel, &mut call_sites);
    Ok(call_sites)
}

pub fn extract_js_imports(project_root: &Path, path: &Path) -> Result<Vec<ParsedJsImport>> {
    let source = fs::read_to_string(path)?;
    let mut parser = Parser::new();
    parser.set_language(js_language())?;
    let tree = parser.parse(&source, None).context("parse javascript")?;
    let root = tree.root_node();
    let file_rel = relative_path(project_root, path)?;
    let mut imports = Vec::new();
    collect_js_imports(root, &source, &file_rel, &mut imports)?;
    Ok(imports)
}

struct JsItem {
    node: GraphNode,
    defines_edge: GraphEdge,
    is_callable: bool,
}

fn walk_js_items(
    node: Node,
    source: &str,
    file_rel: &str,
    namespace: &mut Vec<String>,
    output: &mut Vec<JsItem>,
    call_sites: &mut Vec<JsCallSite>,
) -> Result<()> {
    if let Some(item) = classify_js_item(node, source, file_rel, namespace) {
        if item.is_callable {
            collect_js_calls_in_node(node, source, &item.node.id, call_sites);
        }
        output.push(item);
    }

    // Enter class bodies as namespace for methods
    let push_ns = node.kind() == "class_declaration" || node.kind() == "object";

    if push_ns {
        if let Some(name) = js_node_name(node, source) {
            namespace.push(name);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_js_items(child, source, file_rel, namespace, output, call_sites)?;
    }

    if push_ns && !namespace.is_empty() {
        namespace.pop();
    }
    Ok(())
}

fn classify_js_item(
    node: Node,
    source: &str,
    file_rel: &str,
    namespace: &[String],
) -> Option<JsItem> {
    let kind = match node.kind() {
        "function_declaration" => Some("Function"),
        "method_definition" => Some("Method"),
        "class_declaration" => Some("Class"),
        "lexical_declaration" | "variable_declaration" => {
            return extract_named_var(node, source, file_rel, namespace);
        }
        _ => None,
    }?;

    let leaf_name = js_node_name(node, source)?;
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
        metadata: json!({
            "language": "javascript",
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
        metadata: json!({ "language": "javascript" }),
    };

    Some(JsItem {
        node: graph_node,
        defines_edge,
        is_callable: matches!(kind, "Function" | "Method"),
    })
}

fn extract_named_var(
    node: Node,
    source: &str,
    file_rel: &str,
    namespace: &[String],
) -> Option<JsItem> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let name_node = child.child_by_field_name("name")?;
        let leaf_name = js_node_text(name_node, source)?;
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
            metadata: json!({
                "language": "javascript",
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
            metadata: json!({ "language": "javascript" }),
        };
        let is_callable = child
            .child_by_field_name("value")
            .map(|v| matches!(v.kind(), "arrow_function" | "function"))
            .unwrap_or(false);
        return Some(JsItem {
            node: graph_node,
            defines_edge,
            is_callable,
        });
    }
    None
}

fn js_node_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "function_declaration" | "function" => node
            .child_by_field_name("name")
            .and_then(|n| js_node_text(n, source)),
        "class_declaration" => node
            .child_by_field_name("name")
            .and_then(|n| js_node_text(n, source)),
        "method_definition" => {
            let name = node.child_by_field_name("name")?;
            js_node_text(name, source)
        }
        // For arrow functions, we look up to the variable declaration
        "arrow_function" => None,
        _ => None,
    }
}

fn js_node_text(node: Node, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes())
        .ok()
        .map(|s| s.to_string())
}

fn collect_js_calls(node: Node, source: &str, file_rel: &str, call_sites: &mut Vec<JsCallSite>) {
    if matches!(node.kind(), "call_expression") {
        if let Some(name) = js_call_target_name(node, source) {
            if let Some(caller) = find_enclosing_function_name(node, source, file_rel) {
                call_sites.push(JsCallSite {
                    caller_id: caller,
                    callee_name: name,
                });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_js_calls(child, source, file_rel, call_sites);
    }
}

fn collect_js_calls_in_node(
    node: Node,
    source: &str,
    caller_id: &str,
    call_sites: &mut Vec<JsCallSite>,
) {
    if matches!(node.kind(), "call_expression") {
        if let Some(name) = js_call_target_name(node, source) {
            call_sites.push(JsCallSite {
                caller_id: caller_id.to_string(),
                callee_name: name,
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_js_calls_in_node(child, source, caller_id, call_sites);
    }
}

fn js_call_target_name(node: Node, source: &str) -> Option<String> {
    let func = node.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => js_node_text(func, source),
        "member_expression" => {
            // Get property: obj.method -> "method"
            func.child_by_field_name("property")
                .and_then(|p| js_node_text(p, source))
        }
        _ => {
            let mut cursor = func.walk();
            let mut last = None;
            for child in func.named_children(&mut cursor) {
                if child.kind() == "identifier" {
                    last = js_node_text(child, source);
                }
            }
            last
        }
    }
}

fn find_enclosing_function_name(node: Node, source: &str, file_rel: &str) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        let name = match n.kind() {
            "function_declaration" => n
                .child_by_field_name("name")
                .and_then(|c| js_node_text(c, source)),
            "method_definition" => n
                .child_by_field_name("name")
                .and_then(|c| js_node_text(c, source)),
            "arrow_function" => n
                .parent()
                .filter(|p| p.kind() == "variable_declarator")
                .and_then(|p| p.child_by_field_name("name"))
                .and_then(|c| js_node_text(c, source)),
            _ => None,
        };
        if let Some(n) = name {
            return Some(format!("symbol:{file_rel}#{n}"));
        }
        current = n.parent();
    }
    None
}

fn collect_js_imports(
    node: Node,
    source: &str,
    file_rel: &str,
    imports: &mut Vec<ParsedJsImport>,
) -> Result<()> {
    if node.kind() == "import_statement" {
        let source_node = node.child_by_field_name("source");
        let module_path = source_node
            .and_then(|s| js_node_text(s, source))
            .map(|s| s.trim_matches('\'').trim_matches('"').to_string())
            .unwrap_or_default();

        // Debug: print all children of import_statement

        // Handle default import: `import def from 'path'`
        // The default import name is a direct child named_children before the source string.
        // We look for a child whose kind is "identifier" while excluding the source (string).
        // This is fragile but works for tree-sitter-javascript 0.20 grammar where
        // import_statement has: [identifier [,]] [named_imports|namespace_import] "from" string
        if let Some(default_name) = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "identifier")
            .and_then(|c| js_node_text(c, source))
        {
            imports.push(make_js_import(file_rel, node, &default_name, &module_path));
            // Don't return early - there might be named imports too, and we still need
            // to recurse for OTHER import statements. But skip specifiers for this node
            // since we already handled the default import.
        }

        // Handle default import: `import def from 'path'`
        // The default import name is inside import_clause node, not an identifier.
        let default_name = (0..node.child_count())
            .filter_map(|i| node.child(i))
            .find(|c| c.kind() == "import_clause")
            .and_then(|c| js_node_text(c, source))
            .filter(|text| !text.starts_with("{") && !text.contains("from"))
            .map(|text| text.trim().to_string());
        if let Some(ref name) = default_name {
            imports.push(make_js_import(file_rel, node, name, &module_path));
        }

        // Handle specifiers (import {x}, import * as x)
        let specifiers = node.child_by_field_name("specifiers");
        if let Some(spec) = specifiers {
            let mut sc = spec.walk();
            for child in spec.named_children(&mut sc) {
                let name = child
                    .child_by_field_name("name")
                    .and_then(|n| js_node_text(n, source))
                    .or_else(|| {
                        child
                            .child_by_field_name("local")
                            .and_then(|n| js_node_text(n, source))
                    })
                    .or_else(|| {
                        child
                            .child_by_field_name("alias")
                            .and_then(|n| js_node_text(n, source))
                    });
                if let Some(name) = name {
                    imports.push(make_js_import(file_rel, node, &name, &module_path));
                }
            }
        } else if default_name.is_none() {
            // Side-effect import: `import 'module'`
            imports.push(make_js_import(file_rel, node, &module_path, &module_path));
        }
    }

    // Handle require() calls
    if node.kind() == "call_expression" {
        let func = node.child_by_field_name("function");
        let is_require = func
            .and_then(|f| js_node_text(f, source))
            .map(|t| t == "require")
            .unwrap_or(false);
        if is_require {
            if let Some(arg) = node.child_by_field_name("arguments") {
                let mut cursor = arg.walk();
                for child in arg.named_children(&mut cursor) {
                    if let Some(module_path) = js_node_text(child, source) {
                        let path = module_path.trim_matches('\'').trim_matches('"').to_string();
                        // Find the variable this require is assigned to
                        let parent_decl = node.parent().and_then(|p| {
                            if p.kind() == "variable_declarator" {
                                Some(p)
                            } else {
                                p.parent().filter(|pp| pp.kind() == "variable_declarator")
                            }
                        });
                        let name = parent_decl
                            .and_then(|d| d.child_by_field_name("name"))
                            .and_then(|n| js_node_text(n, source))
                            .unwrap_or_else(|| format!("require_{}", path.replace('/', "_")));
                        imports.push(make_js_import(file_rel, node, &name, &path));
                    }
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_js_imports(child, source, file_rel, imports)?;
    }
    Ok(())
}

fn make_js_import(file_rel: &str, node: Node, name: &str, module_path: &str) -> ParsedJsImport {
    let import_id = format!("import:{file_rel}#{name}");
    let import_node = GraphNode {
        id: import_id.clone(),
        kind: "Import".to_string(),
        name: name.to_string(),
        path: Some(file_rel.to_string()),
        span_start: Some(node.start_position().row as i64),
        span_end: Some(node.end_position().row as i64),
        hash: None,
        metadata: json!({
            "language": "javascript",
            "module_path": module_path,
        }),
    };
    let import_edge = GraphEdge {
        id: format!("edge:file:{file_rel}:imports:{import_id}"),
        source_id: format!("file:{file_rel}"),
        target_id: import_id,
        kind: "imports".to_string(),
        weight: 1.0,
        confidence: 0.9,
        metadata: json!({ "language": "javascript" }),
    };
    ParsedJsImport {
        node: import_node,
        import_edge,
        import_path: name.to_string(),
        source: module_path.to_string(),
    }
}

fn build_js_symbol_index(symbols: &[JsItem]) -> HashMap<String, Vec<String>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();
    for sym in symbols {
        index
            .entry(sym.node.name.clone())
            .or_default()
            .push(sym.node.id.clone());
        if let Some(leaf) = sym.node.name.rsplit("::").next() {
            index
                .entry(leaf.to_string())
                .or_default()
                .push(sym.node.id.clone());
        }
    }
    index
}

fn resolve_js_call_edges(
    calls: &[JsCallSite],
    index: &HashMap<String, Vec<String>>,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    for call in calls {
        let targets = index.get(&call.callee_name);
        if let Some(targets) = targets {
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
                        "language": "javascript",
                        "callee": call.callee_name,
                        "resolved": target_id
                    }),
                });
            }
        }
    }
    edges
}

fn relative_path(project_root: &Path, path: &Path) -> Result<String> {
    let abs_root = project_root.canonicalize()?;
    let abs_path = path.canonicalize()?;
    let rel = abs_path.strip_prefix(&abs_root).unwrap_or(&abs_path);
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

pub fn resolve_js_import_target(
    source_file_rel: &str,
    import_source: &str,
    existing_files: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if !import_source.starts_with('.') {
        return None;
    }
    let source_dir = Path::new(source_file_rel)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let import_clean = import_source.strip_prefix("./").unwrap_or(import_source);
    let resolved = source_dir.join(import_clean);
    for ext in &["js", "jsx"] {
        for candidate in &[
            resolved.with_extension(ext),
            resolved.join("index").with_extension(ext),
        ] {
            let rel_str = candidate.to_string_lossy().replace('\\', "/");
            if existing_files.contains_key(&rel_str) {
                return Some(rel_str);
            }
            if let Some(stripped) = rel_str.strip_prefix('/') {
                if existing_files.contains_key(stripped) {
                    return Some(stripped.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn parse_js_symbols_from(source: &str) -> Vec<ParsedJsSymbol> {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("test.js");
        fs::write(&path, source).expect("write");
        parse_js_symbols(dir.path(), &path).expect("parse_js_symbols")
    }

    #[test]
    fn parses_function_declarations() {
        let symbols = parse_js_symbols_from("function hello() { return 1; }");
        assert!(symbols
            .iter()
            .any(|s| s.node.name == "hello" && s.node.kind == "Function"));
    }

    #[test]
    fn parses_class_and_methods() {
        let symbols = parse_js_symbols_from("class MyClass { constructor() {} method() {} }");
        assert!(symbols
            .iter()
            .any(|s| s.node.name == "MyClass" && s.node.kind == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.node.name == "MyClass::method" && s.node.kind == "Method"));
    }

    #[test]
    fn parses_arrow_function_as_var() {
        let symbols = parse_js_symbols_from("const greet = (name) => `Hello ${name}`;");
        // arrow functions inside const declarations are Variables with callable flag
        assert!(symbols
            .iter()
            .any(|s| s.node.name == "greet" && s.node.kind == "Variable"));
    }

    #[test]
    fn parses_const_variable() {
        let symbols = parse_js_symbols_from("const x = 42;");
        assert!(symbols
            .iter()
            .any(|s| s.node.name == "x" && s.node.kind == "Variable"));
    }

    #[test]
    fn finds_intra_file_call_edges() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("test.js");
        fs::write(
            &path,
            "\
function helper() { return 1; }
function main() { return helper(); }
        ",
        )
        .expect("write");
        let symbols = parse_js_symbols(dir.path(), &path).expect("parse");
        let main = symbols
            .iter()
            .find(|s| s.node.name == "main")
            .expect("main");
        let has_call = main
            .extra_edges
            .iter()
            .any(|e| e.kind == "calls" && e.target_id.contains("helper"));
        assert!(has_call, "main should have a calls edge to helper");
    }

    #[test]
    fn extracts_es_module_imports() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("main.js");
        let source = r#"
            import { readFile } from 'fs';
            import { useState } from 'react';
            import def from './local-module';
        "#;
        fs::write(&path, source).expect("write");
        let imports = extract_js_imports(dir.path(), &path).expect("extract_js_imports");
        assert_eq!(imports.len(), 3);
        // import { readFile } from 'fs' -> import_path is "readFile"
        // Print debug for diagnosis
        for imp in &imports {
            eprintln!("  import: path={} source={}", imp.import_path, imp.source);
        }
        let fs_imports: Vec<_> = imports.iter().filter(|i| i.source == "fs").collect();
        assert!(
            !fs_imports.is_empty(),
            "Expected imports from 'fs', got none; all imports: {:?}",
            imports
                .iter()
                .map(|i| format!("{}/{}", i.source, i.import_path))
                .collect::<Vec<_>>()
        );
        assert!(imports
            .iter()
            .any(|i| i.source == "./local-module" && i.import_path == "def"));
    }

    #[test]
    fn extracts_side_effect_import() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("main.js");
        fs::write(&path, "import './styles.css';").expect("write");
        let imports = extract_js_imports(dir.path(), &path).expect("extract_js_imports");
        assert_eq!(imports.len(), 1);
        assert!(imports[0].source == "./styles.css");
    }

    #[test]
    fn extracts_require_calls() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("main.js");
        fs::write(
            &path,
            "const fs = require('fs');\nconst path = require('path');",
        )
        .expect("write");
        let imports = extract_js_imports(dir.path(), &path).expect("extract_js_imports");
        assert_eq!(imports.len(), 2);
        assert!(imports.iter().any(|i| i.source == "fs"));
        assert!(imports.iter().any(|i| i.source == "path"));
    }

    #[test]
    fn resolves_relative_js_import() {
        let mut files = std::collections::HashMap::new();
        files.insert("src/utils.js".to_string(), "file:src/utils.js".to_string());
        let result = resolve_js_import_target("src/main.js", "./utils", &files);
        assert_eq!(result, Some("src/utils.js".to_string()));
    }

    #[test]
    fn skips_external_imports() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "node_modules/react/index.js".to_string(),
            "file:...".to_string(),
        );
        let result = resolve_js_import_target("src/main.js", "react", &files);
        assert_eq!(result, None);
    }
}
