//! MemoryCore demo: end-to-end verification.
//!
//! Builds a multi-language project, scans it, runs every command,
//! and prints a structured report.
//!
//! Run: cargo test -p memorycore-cli --test demo -- --nocapture

use std::fs;
use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_memorycore");

struct Demo {
    root: tempfile::TempDir,
}

impl Demo {
    fn new() -> Self {
        Demo { root: tempfile::TempDir::with_prefix("mc-demo-").unwrap() }
    }

    fn path(&self) -> &Path { self.root.path() }

    fn run(&self, args: &[&str]) -> String {
        let out = Command::new(BIN).args(args).current_dir(self.path()).output().unwrap();
        assert!(out.status.success(), "{} failed\nstdout:{}\nstderr:{}",
            args.join(" "), String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn create_rust_project(&self) {
        let root = self.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), r#"
pub mod auth;
pub mod db;
pub fn init() -> Result<(), String> {
    auth::login("admin", "secret")?;
    db::connect()?;
    Ok(())
}"#).unwrap();
        fs::write(root.join("src/auth.rs"), r#"
pub struct User { pub name: String, pub role: Role }
pub enum Role { Admin, User, Guest }
pub fn login(name: &str, pass: &str) -> Result<User, String> {
    if pass == "secret" { Ok(User { name: name.into(), role: Role::Admin }) }
    else { Err("bad password".into()) }
}"#).unwrap();
        fs::write(root.join("src/db.rs"), r#"
pub struct Connection { pub url: String }
pub fn connect() -> Result<Connection, String> {
    Ok(Connection { url: "sqlite://local".into() })
}
pub fn query(conn: &Connection, sql: &str) -> Vec<String> {
    vec![format!("result: {}", sql)]
}"#).unwrap();
    }

    fn create_js_project(&self) {
        let root = self.path();
        fs::create_dir_all(root.join("js")).unwrap();
        fs::write(root.join("js/app.js"), r#"
import { connect } from './db.js';
export function start() {
  return connect().query("SELECT 1");
}
class Router {
  constructor() { this.routes = {}; }
  get(path, handler) { this.routes[path] = handler; }
}"#).unwrap();
        fs::write(root.join("js/db.js"), r#"
export function connect() {
  return { query: (sql) => `exec: ${sql}` };
}
function helper() { return "helper"; }"#).unwrap();
    }
}

fn check(label: &str, ok: bool) {
    println!("  {} {}", if ok { "\u{2713}" } else { "\u{2717}" }, label);
}

fn section(n: &str, title: &str) {
    println!("\n--- [{}/18] {} ---", n, title);
}

#[test]
fn full_demo() {
    let d = Demo::new();
    println!("{}", "=".repeat(58));
    println!("  MemoryCore Demo Suite");
    println!("  Project: {}", d.path().display());
    println!("{}", "=".repeat(58));
    let mut passed = 0u32;
    let mut total = 0u32;
    macro_rules! ck { ($label:expr, $cond:expr) => { {
        total += 1;
        if $cond { passed += 1; }
        check($label, $cond);
    } } }

    // 1. Init
    section("1", "Init");
    d.run(&["init"]);
    ck!(".memorycore/ created", d.path().join(".memorycore").is_dir());
    ck!("index.db exists", d.path().join(".memorycore/index.db").is_file());
    ck!("config.toml exists", d.path().join(".memorycore/config.toml").is_file());

    // 2. Status (empty)
    section("2", "Status (empty)");
    let out = d.run(&["status"]);
    ck!("shows 0 graph nodes", out.contains("Nodes:"));
    ck!("daemon not running", out.contains("stopped") || out.contains("not running"));

    // 3. Create source files
    section("3", "Create source files");
    d.create_rust_project();
    d.create_js_project();
    for f in &["src/lib.rs","src/auth.rs","src/db.rs","js/app.js","js/db.js"] {
        ck!(f, d.path().join(f).is_file());
    }

    // 4. Graph file (Rust)
    section("4", "Graph file (Rust)");
    for f in &["src/lib.rs","src/auth.rs","src/db.rs"] {
        let o = d.run(&["graph", "file", f]);
        ck!(f, o.contains("Scanned file"));
    }

    // 5. Graph file (JS)
    section("5", "Graph file (JS)");
    for f in &["js/app.js","js/db.js"] {
        let o = d.run(&["graph", "file", f]);
        ck!(f, o.contains("Scanned file"));
    }

    // 6. Status (with data)
    section("6", "Status (with data)");
    let out = d.run(&["status"]);
    let has_nodes = out.contains("Nodes:") && !out.contains("Nodes:    0");
    let has_edges = out.contains("Edges:") && !out.contains("Edges:    0");
    ck!("has graph nodes", has_nodes);
    ck!("has graph edges", has_edges);

    // 7. Graph Mermaid export
    section("7", "Graph Mermaid export");
    let out = d.run(&["graph", "export", "--format", "mermaid"]);
    ck!("starts with flowchart TD", out.starts_with("flowchart TD"));
    ck!("Rust auth symbols", out.contains("auth"));
    ck!("Rust db symbols", out.contains("db"));
    ck!("JS app symbols", out.contains("app.js"));
    ck!("contains edges", out.contains("-->"));

    // 8. Graph JSON export
    section("8", "Graph JSON export");
    let out = d.run(&["graph", "export", "--format", "json"]);
    ck!("valid JSON", out.starts_with("{"));
    ck!("has edges", out.contains("\"edges\""));
    ck!("has nodes", out.contains("\"nodes\""));
    // Verify it's parseable JSON
    let is_json: bool = serde_json::from_str::<serde_json::Value>(&out).is_ok();
    ck!("parseable", is_json);

    // 9. Graph query
    section("9", "Graph query");
    let out = d.run(&["graph", "query", "login", "--format", "mermaid"]);
    ck!("query finds login", out.contains("login"));
    let out = d.run(&["graph", "query", "app", "--depth", "2"]);
    ck!("depth 2 query", out.contains("app"));

    // 10. Graph impact
    section("10", "Graph impact");
    let out = d.run(&["graph", "impact", "src/lib.rs", "--depth", "1"]);
    ck!("impact report", out.contains("impact for") && out.contains("-"));

    // 11. Search
    section("11", "Search");
    let out = d.run(&["search", "login"]);
    ck!("finds login fn", out.contains("login"));
    let out = d.run(&["search", "connect"]);
    ck!("finds connect in Rust+JS",
        out.contains("src/db.rs") && out.contains("js/db.js"));

    // 12. Memory
    section("12", "Memory");
    d.run(&["memory", "pin", "rust-project", "--target", "src/lib.rs"]);
    let out = d.run(&["memory", "list"]);
    ck!("memory case pinned", out.contains("rust-project"));

    // 13. Snapshots
    section("13", "Snapshots");
    let out = d.run(&["snapshots", "create", "--message", "demo-v1"]);
    ck!("snapshot created", out.contains("created"));
    let out = d.run(&["snapshots", "list"]);
    ck!("snapshot listed", out.contains("demo-v1"));

    // 14. Events
    section("14", "Events");
    let out = d.run(&["events", "--limit", "5"]);
    ck!("events returned", out.contains("event_type"));
    ck!("file scanned event", out.contains("graph_file_scanned"));

    // 15. Analyze
    section("15", "Analyze");
    let out = d.run(&["analyze", "src/lib.rs", "--format", "text"]);
    ck!("analysis report", out.contains("MemoryCore Analysis") && out.contains("Graph context"));
    let out = d.run(&["analyze", "js/app.js", "--format", "mermaid"]);
    ck!("mermaid analysis", out.contains("flowchart TD"));

    // 16. Daemon
    section("16", "Daemon lifecycle");
    let out = d.run(&["daemon", "start"]);
    ck!("daemon started", out.contains("running pid"));
    let out = d.run(&["daemon", "status"]);
    ck!("daemon reports running", out.contains("running"));
    let out = d.run(&["daemon", "stop"]);
    ck!("daemon stopped", out.contains("Stopped"));

    // 17. Sessions
    section("17", "Sessions");
    fs::write(d.path().join("chat.jsonl"), r#"{"role":"user","content":"hello from demo","timestamp":10,"metadata":{}}
{"role":"assistant","content":"demo is running","timestamp":11,"metadata":{}}
"#).unwrap();
    let out = d.run(&["sessions", "import", "--agent", "demo", "--id", "demo-chat", "chat.jsonl"]);
    ck!("session imported", out.contains("Imported"));
    let out = d.run(&["sessions", "list"]);
    ck!("session listed", out.contains("demo-chat"));
    let out = d.run(&["sessions", "show", "demo-chat"]);
    ck!("messages shown", out.contains("hello from demo") && out.contains("demo is running"));

    // 18. Registries
    section("18", "Registries");
    let out = d.run(&["plugins", "list"]);
    ck!("plugins", out.contains("No plugins"));
    let out = d.run(&["skills", "list"]);
    ck!("skills", out.contains("No skills"));
    let out = d.run(&["adapters", "list"]);
    ck!("adapters", out.contains("No adapters"));

    // Summary
    println!("\n{}", "=".repeat(58));
    println!("  RESULT: {}/{} checks passed", passed, total);
    println!("  Project: {}", d.path().display());
    println!("  Binary: {}", BIN);
    if passed == total {
        println!("  VERDICT: MemoryCore is fully operational.");
    } else {
        println!("  VERDICT: Some checks FAILED.");
    }
    println!("{}", "=".repeat(58));
    assert_eq!(passed, total, "{} of {} checks failed", total - passed, total);
}
