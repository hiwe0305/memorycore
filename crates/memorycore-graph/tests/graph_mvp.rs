use memorycore_core::{connect_project_db, init_project};
use memorycore_graph::impact::{find_impact, find_impact_with_depth};
use memorycore_graph::parser::parse_rust_symbols;
use memorycore_graph::query::{
    graph_subset_json, graph_subset_mermaid, graph_target_json, resolve_graph_target,
};
use memorycore_graph::render::json::render_json;
use memorycore_graph::render::mermaid::render_mermaid;
use memorycore_graph::{scan_file, scan_folder};
use std::fs;
use tempfile::tempdir;

#[test]
fn scans_file_and_exports_mermaid() {
    let temp = tempdir().expect("create temp project");
    fs::write(temp.path().join("main.rs"), "fn main() {}\n").expect("write test file");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    let summary = scan_file(&conn, temp.path(), &temp.path().join("main.rs")).expect("scan file");
    assert_eq!(summary.files, 1);
    assert_eq!(summary.edges, 2);

    let node_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM graph_nodes", [], |row| row.get(0))
        .expect("count graph nodes");
    let edge_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))
        .expect("count graph edges");
    let event_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE event_type = 'graph_file_scanned'",
            [],
            |row| row.get(0),
        )
        .expect("count graph scan events");
    assert_eq!(node_count, 3);
    assert_eq!(edge_count, 2);
    assert_eq!(event_count, 1);

    let mermaid = render_mermaid(&conn).expect("render mermaid");
    assert!(mermaid.starts_with("flowchart TD"));
    assert!(mermaid.contains("Project:"));
    assert!(mermaid.contains("File: main.rs"));
    assert!(mermaid.contains("-->|contains|"));

    let graph_json = render_json(&conn).expect("render graph json");
    assert!(graph_json.contains("\"nodes\""));
    assert!(graph_json.contains("\"edges\""));
    assert!(graph_json.contains("file:main.rs"));

    let resolved = resolve_graph_target(&conn, "main.rs").expect("resolve graph target");
    assert_eq!(resolved.as_deref(), Some("file:main.rs"));

    let subset = graph_subset_json(&conn, "file:main.rs").expect("render subset json");
    assert!(subset.contains("\"focus\""));
    assert!(subset.contains("\"nodes\""));
    assert!(subset.contains("file:main.rs"));
    assert!(subset.contains("\"edges\""));
    assert!(subset.contains("\"span_start\""));
    assert!(subset.contains("\"span_end\""));
    assert!(subset.contains("\"hash\""));

    let target_subset = graph_target_json(&conn, "main.rs").expect("render target subset json");
    assert!(target_subset.contains("file:main.rs"));

    let subset_mermaid =
        graph_subset_mermaid(&conn, "file:main.rs").expect("render subset mermaid");
    assert!(subset_mermaid.starts_with("flowchart TD"));
    assert!(subset_mermaid.contains("File: main.rs"));

    let impact = find_impact(&conn, "main.rs", 25).expect("find impact");
    assert!(impact.contains("impact for file:main.rs"));
    assert!(impact.contains("- project:"));
    assert!(impact.contains("-contains-> file:main.rs"));
    assert!(impact.contains("- file:main.rs -defines-> symbol:main.rs#main"));
}

#[test]
fn scans_rust_call_edges() {
    let temp = tempdir().expect("create temp project");
    fs::write(
        temp.path().join("main.rs"),
        "struct Demo;\nimpl Demo {\n    fn helper(&self) {}\n    fn run(&self) { self.helper(); }\n}\nfn main() { let demo = Demo; demo.run(); }\n",
    )
    .expect("write test file");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    let parsed = parse_rust_symbols(temp.path(), &temp.path().join("main.rs")).expect("parse rust");
    let parsed_call_edges: usize = parsed.iter().map(|symbol| symbol.extra_edges.len()).sum();
    assert_eq!(parsed_call_edges, 2);

    let summary = scan_file(&conn, temp.path(), &temp.path().join("main.rs")).expect("scan file");
    assert_eq!(summary.files, 1);
    assert_eq!(summary.edges, 7);

    let edge_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))
        .expect("count graph edges");
    assert_eq!(edge_count, 7);

    let call_edge_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'calls'",
            [],
            |row| row.get(0),
        )
        .expect("count call edges");
    assert_eq!(call_edge_count, 2);

    let impact = find_impact(&conn, "symbol:main.rs#impl Demo::run", 25).expect("find impact");
    assert!(impact
        .contains("- symbol:main.rs#impl Demo::run -calls-> symbol:main.rs#impl Demo::helper"));
    let main_impact = find_impact(&conn, "symbol:main.rs#main", 25).expect("find impact");
    assert!(main_impact.contains("- symbol:main.rs#main -calls-> symbol:main.rs#impl Demo::run"));
}

#[test]
fn rescanning_file_prunes_removed_symbols_and_stale_calls() {
    let temp = tempdir().expect("create temp project");
    let path = temp.path().join("main.rs");
    fs::write(
        &path,
        "fn helper() {}\nfn other() {}\nfn main() { helper(); }\n",
    )
    .expect("write initial file");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    scan_file(&conn, temp.path(), &path).expect("scan initial file");
    let helper_calls: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'calls' AND target_id = 'symbol:main.rs#helper'",
            [],
            |row| row.get(0),
        )
        .expect("count helper calls");
    assert_eq!(helper_calls, 1);

    fs::write(&path, "fn other() {}\nfn main() { other(); }\n").expect("rewrite file");
    scan_file(&conn, temp.path(), &path).expect("rescan file");

    let helper_nodes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE id = 'symbol:main.rs#helper'",
            [],
            |row| row.get(0),
        )
        .expect("count helper nodes");
    assert_eq!(helper_nodes, 0);

    let stale_helper_calls: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'calls' AND target_id = 'symbol:main.rs#helper'",
            [],
            |row| row.get(0),
        )
        .expect("count stale helper calls");
    assert_eq!(stale_helper_calls, 0);

    let other_calls: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'calls' AND target_id = 'symbol:main.rs#other'",
            [],
            |row| row.get(0),
        )
        .expect("count other calls");
    assert_eq!(other_calls, 1);
}

#[test]
fn impact_depth_expands_beyond_direct_edges() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src")).expect("create src folder");
    fs::write(
        temp.path().join("src/lib.rs"),
        "mod helper;\npub fn helper() {}\n",
    )
    .expect("write lib");
    fs::write(
        temp.path().join("src/main.rs"),
        "use crate::helper::helper;\nfn main() { helper(); }\n",
    )
    .expect("write main");
    fs::write(temp.path().join("src/helper.rs"), "pub fn helper() {}\n").expect("write helper");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan folder");

    let shallow = find_impact(&conn, "file:src/main.rs", 25).expect("find shallow impact");
    assert!(
        shallow.contains("- file:src/main.rs -imports-> import:src/main.rs#crate::helper::helper")
    );
    assert!(!shallow.contains("symbol:src/helper.rs#helper"));

    let deep = find_impact_with_depth(&conn, "file:src/main.rs", 25, 2).expect("find deep impact");
    assert!(deep.contains("symbol:src/helper.rs#helper"));
    assert!(deep.contains("- import:src/main.rs#crate::helper::helper -resolves_import_symbol-> symbol:src/helper.rs#helper"));
}

#[test]
fn scans_rust_import_edges() {
    let temp = tempdir().expect("create temp project");
    fs::write(
        temp.path().join("main.rs"),
        "use std::collections::HashMap;\nfn main() {}\n",
    )
    .expect("write test file");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    let summary = scan_file(&conn, temp.path(), &temp.path().join("main.rs")).expect("scan file");
    assert_eq!(summary.files, 1);
    assert_eq!(summary.edges, 3);

    let import_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE kind = 'Import'",
            [],
            |row| row.get(0),
        )
        .expect("count import nodes");
    assert_eq!(import_count, 1);

    let import_edge_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'imports'",
            [],
            |row| row.get(0),
        )
        .expect("count import edges");
    assert_eq!(import_edge_count, 1);

    let impact = find_impact(&conn, "main.rs", 25).expect("find impact");
    assert!(impact.contains("- file:main.rs -imports-> import:main.rs#std::collections::HashMap"));
}

#[test]
fn resolves_local_rust_imports_to_files() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src")).expect("create src folder");
    fs::write(temp.path().join("src/lib.rs"), "mod helper;\n").expect("write lib");
    fs::write(
        temp.path().join("src/main.rs"),
        "use crate::helper::helper;\nfn main() { helper(); }\n",
    )
    .expect("write main");
    fs::write(temp.path().join("src/helper.rs"), "pub fn helper() {}\n").expect("write helper");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    let summary = scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan folder");
    assert_eq!(summary.files, 3);

    let resolves_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'resolves_import'",
            [],
            |row| row.get(0),
        )
        .expect("count resolved imports");
    assert_eq!(resolves_count, 1);

    let impact = find_impact(&conn, "import:src/main.rs#crate::helper::helper", 25)
        .expect("find import impact");
    assert!(impact.contains(
        "- import:src/main.rs#crate::helper::helper -resolves_import-> file:src/helper.rs"
    ));
}

#[test]
fn scan_file_resolves_import_symbols_when_target_is_indexed() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src")).expect("create src folder");
    fs::write(temp.path().join("src/lib.rs"), "mod helper;\n").expect("write lib");
    fs::write(temp.path().join("src/helper.rs"), "pub fn helper() {}\n").expect("write helper");
    fs::write(
        temp.path().join("src/main.rs"),
        "use crate::helper::helper;\nfn main() { helper(); }\n",
    )
    .expect("write main");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    scan_file(&conn, temp.path(), &temp.path().join("src/helper.rs")).expect("scan helper file");
    scan_file(&conn, temp.path(), &temp.path().join("src/main.rs")).expect("scan main file");

    let resolves_symbol_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'resolves_import_symbol'",
            [],
            |row| row.get(0),
        )
        .expect("count resolved import symbols");
    assert_eq!(resolves_symbol_count, 1);

    let impact = find_impact(&conn, "import:src/main.rs#crate::helper::helper", 25)
        .expect("find import symbol impact");
    assert!(impact.contains(
        "- import:src/main.rs#crate::helper::helper -resolves_import_symbol-> symbol:src/helper.rs#helper"
    ));
}

#[test]
fn resolves_local_rust_imports_to_symbols() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src")).expect("create src folder");
    fs::write(temp.path().join("src/lib.rs"), "mod helper;\n").expect("write lib");
    fs::write(
        temp.path().join("src/main.rs"),
        "use crate::helper::helper;\nfn main() { helper(); }\n",
    )
    .expect("write main");
    fs::write(temp.path().join("src/helper.rs"), "pub fn helper() {}\n").expect("write helper");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan folder");

    let resolves_symbol_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'resolves_import_symbol'",
            [],
            |row| row.get(0),
        )
        .expect("count resolved import symbols");
    assert_eq!(resolves_symbol_count, 1);

    let impact = find_impact(&conn, "import:src/main.rs#crate::helper::helper", 25)
        .expect("find import symbol impact");
    assert!(impact.contains(
        "- import:src/main.rs#crate::helper::helper -resolves_import_symbol-> symbol:src/helper.rs#helper"
    ));
}

#[test]
fn resolves_alias_and_glob_rust_imports_to_files() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src")).expect("create src folder");
    fs::write(
        temp.path().join("src/helper.rs"),
        "pub fn alpha() {}\npub fn beta() {}\n",
    )
    .expect("write helper");
    fs::write(
        temp.path().join("src/main.rs"),
        "use crate::helper::{alpha as a, beta as b};\nuse crate::helper::*;\nfn main() { a(); b(); alpha(); beta(); }\n",
    )
    .expect("write main");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan folder");

    let resolves_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'resolves_import'",
            [],
            |row| row.get(0),
        )
        .expect("count resolved imports");
    assert_eq!(resolves_count, 3);

    let alias_impact = find_impact(&conn, "import:src/main.rs#crate::helper::alpha as a", 25)
        .expect("find alias impact");
    assert!(alias_impact.contains(
        "- import:src/main.rs#crate::helper::alpha as a -resolves_import-> file:src/helper.rs"
    ));

    let glob_impact =
        find_impact(&conn, "import:src/main.rs#crate::helper::*", 25).expect("find glob impact");
    assert!(glob_impact
        .contains("- import:src/main.rs#crate::helper::* -resolves_import-> file:src/helper.rs"));
}

#[test]
fn resolves_nested_module_imports_to_crate_root_files() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src/nested")).expect("create nested folder");
    fs::write(temp.path().join("src/helper.rs"), "pub fn alpha() {}\n").expect("write helper");
    fs::write(
        temp.path().join("src/nested/mod.rs"),
        "use crate::helper::alpha;\npub fn call() { alpha(); }\n",
    )
    .expect("write nested mod");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan folder");

    let resolves_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'resolves_import'",
            [],
            |row| row.get(0),
        )
        .expect("count resolved imports");
    assert_eq!(resolves_count, 1);

    let impact = find_impact(&conn, "import:src/nested/mod.rs#crate::helper::alpha", 25)
        .expect("find nested import impact");
    assert!(impact.contains(
        "- import:src/nested/mod.rs#crate::helper::alpha -resolves_import-> file:src/helper.rs"
    ));
}

#[test]
fn resolves_imports_to_the_closest_matching_file_when_names_repeat() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src/a")).expect("create a folder");
    fs::create_dir_all(temp.path().join("src/b")).expect("create b folder");
    fs::write(temp.path().join("src/a/helper.rs"), "pub fn alpha() {}\n").expect("write a helper");
    fs::write(temp.path().join("src/b/helper.rs"), "pub fn alpha() {}\n").expect("write b helper");
    fs::write(
        temp.path().join("src/b/mod.rs"),
        "use crate::helper::alpha;\npub fn call() { alpha(); }\n",
    )
    .expect("write b mod");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan folder");

    let impact = find_impact(&conn, "import:src/b/mod.rs#crate::helper::alpha", 25)
        .expect("find ambiguous import impact");
    assert!(impact.contains(
        "- import:src/b/mod.rs#crate::helper::alpha -resolves_import-> file:src/b/helper.rs"
    ));
    assert!(!impact.contains(
        "- import:src/b/mod.rs#crate::helper::alpha -resolves_import-> file:src/a/helper.rs"
    ));
}

#[test]
fn parses_pub_use_imports() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src")).expect("create src folder");
    fs::write(
        temp.path().join("src/lib.rs"),
        "pub mod helper;\npub use crate::helper::alpha;\n",
    )
    .expect("write lib");
    fs::write(temp.path().join("src/helper.rs"), "pub fn alpha() {}\n").expect("write helper");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    let summary = scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan folder");
    assert_eq!(summary.files, 2);

    let import_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE kind = 'Import'",
            [],
            |row| row.get(0),
        )
        .expect("count import nodes");
    assert_eq!(import_count, 1);

    let resolves_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'resolves_import'",
            [],
            |row| row.get(0),
        )
        .expect("count resolved imports");
    assert_eq!(resolves_count, 1);

    let impact = find_impact(&conn, "import:src/lib.rs#crate::helper::alpha", 25)
        .expect("find pub use impact");
    assert!(impact.contains(
        "- import:src/lib.rs#crate::helper::alpha -resolves_import-> file:src/helper.rs"
    ));
}

#[test]
fn scans_rust_module_edges() {
    let temp = tempdir().expect("create temp project");

    let nested = temp.path().join("nested");
    fs::create_dir_all(nested.join("src")).expect("create nested src folder");
    fs::write(nested.join("src/lib.rs"), "mod helper;\n").expect("write nested lib");
    fs::create_dir_all(nested.join("src/helper")).expect("create nested helper folder");
    fs::write(nested.join("src/helper/mod.rs"), "pub fn helper() {}\n")
        .expect("write nested helper");
    init_project(nested.as_path()).expect("init nested project");
    let nested_conn = connect_project_db(nested.as_path()).expect("open nested database");
    scan_folder(&nested_conn, nested.as_path(), &nested.join("src")).expect("scan nested folder");
    let nested_impact =
        find_impact(&nested_conn, "file:src/lib.rs", 25).expect("find nested impact");
    assert!(nested_impact.contains("- file:src/lib.rs -declares_module-> file:src/helper/mod.rs"));

    let flat = temp.path().join("flat");
    fs::create_dir_all(flat.join("src")).expect("create flat src folder");
    fs::write(flat.join("src/lib.rs"), "mod helper;\n").expect("write flat lib");
    fs::write(flat.join("src/helper.rs"), "pub fn helper() {}\n").expect("write flat helper");
    init_project(flat.as_path()).expect("init flat project");
    let flat_conn = connect_project_db(flat.as_path()).expect("open flat database");
    scan_folder(&flat_conn, flat.as_path(), &flat.join("src")).expect("scan flat folder");
    let flat_impact = find_impact(&flat_conn, "file:src/lib.rs", 25).expect("find flat impact");
    assert!(flat_impact.contains("- file:src/lib.rs -declares_module-> file:src/helper.rs"));
}

#[test]
fn scans_folder_with_nested_files() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src/nested")).expect("create test folders");
    fs::write(temp.path().join("src/lib.rs"), "pub fn lib() {}\n").expect("write lib");
    fs::write(
        temp.path().join("src/nested/mod.rs"),
        "pub fn nested() {}\n",
    )
    .expect("write mod");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    let summary = scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan folder");
    assert_eq!(summary.files, 2);
    assert_eq!(summary.folders, 2);
    assert_eq!(summary.edges, 6);

    let src_node_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE id IN ('folder:src', 'folder:src/nested', 'file:src/lib.rs', 'file:src/nested/mod.rs')",
            [],
            |row| row.get(0),
        )
        .expect("count expected graph nodes");
    assert_eq!(src_node_count, 4);

    let symbol_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE kind IN ('Function', 'Method')",
            [],
            |row| row.get(0),
        )
        .expect("count symbol nodes");
    assert_eq!(symbol_count, 2);
}

#[test]
fn scans_folder_and_resolves_cross_file_calls() {
    let temp = tempdir().expect("create temp project");
    fs::create_dir_all(temp.path().join("src")).expect("create src folder");
    fs::write(temp.path().join("src/lib.rs"), "pub fn helper() {}\n").expect("write lib");
    fs::write(temp.path().join("src/main.rs"), "fn main() { helper(); }\n").expect("write main");
    init_project(temp.path()).expect("init project");
    let conn = connect_project_db(temp.path()).expect("open database");

    let summary = scan_folder(&conn, temp.path(), &temp.path().join("src")).expect("scan folder");
    assert_eq!(summary.files, 2);
    assert_eq!(summary.folders, 1);

    let call_edge_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE kind = 'calls'",
            [],
            |row| row.get(0),
        )
        .expect("count call edges");
    assert_eq!(call_edge_count, 1);

    let impact = find_impact(&conn, "symbol:src/main.rs#main", 25).expect("find impact");
    assert!(impact.contains("- symbol:src/main.rs#main -calls-> symbol:src/lib.rs#helper"));
}
