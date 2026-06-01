/// Simple ANSI color helpers for CLI output.
/// Respects NO_COLOR environment variable.

use std::sync::OnceLock;

static NO_COLOR: OnceLock<bool> = OnceLock::new();

fn no_color() -> bool {
    *NO_COLOR.get_or_init(|| std::env::var("NO_COLOR").is_ok())
}

pub fn red(s: &str) -> String {
    if no_color() { s.to_string() } else { format!("[31m{}[0m", s) }
}

pub fn green(s: &str) -> String {
    if no_color() { s.to_string() } else { format!("[32m{}[0m", s) }
}

pub fn yellow(s: &str) -> String {
    if no_color() { s.to_string() } else { format!("[33m{}[0m", s) }
}

pub fn blue(s: &str) -> String {
    if no_color() { s.to_string() } else { format!("[34m{}[0m", s) }
}

pub fn magenta(s: &str) -> String {
    if no_color() { s.to_string() } else { format!("[35m{}[0m", s) }
}

pub fn cyan(s: &str) -> String {
    if no_color() { s.to_string() } else { format!("[36m{}[0m", s) }
}

pub fn bold(s: &str) -> String {
    if no_color() { s.to_string() } else { format!("[1m{}[0m", s) }
}

pub fn dim(s: &str) -> String {
    if no_color() { s.to_string() } else { format!("[2m{}[0m", s) }
}

pub fn header(s: &str) -> String {
    format!("{}", bold(&format!("
[44;97m {} [0m", s)))
}

pub fn ok(s: &str) -> String {
    format!("{} {}", green("✓"), s)
}

pub fn fail(s: &str) -> String {
    format!("{} {}", red("✗"), s)
}

pub fn label(kind: &str) -> String {
    match kind {
        "File" | "Function" | "Method" => cyan(kind),
        "Struct" | "Class" | "Interface" | "Enum" | "TypeAlias" => yellow(kind),
        "Variable" | "Import" => dim(kind),
        "Folder" | "Project" => blue(kind),
        "Plugin" | "Skill" | "Adapter" | "Agent" => magenta(kind),
        "OK" | "enabled" | "running" | "active" => green(kind),
        "FAIL" | "disabled" | "error" | "offline" => red(kind),
        _ => dim(kind),
    }
}
