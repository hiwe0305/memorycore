use anyhow::{Context, Result};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ProjectLayout {
    pub root: PathBuf,
    pub memorycore: PathBuf,
    pub index_db: PathBuf,
    pub sessions: PathBuf,
    pub snapshots: PathBuf,
    pub snapshot_objects: PathBuf,
    pub snapshot_refs: PathBuf,
    pub embeddings: PathBuf,
    pub plugins: PathBuf,
    pub skills: PathBuf,
    pub events: PathBuf,
    pub logs: PathBuf,
}

pub fn memorycore_dir(root: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join(".memorycore")
}

pub fn project_db_path(root: impl AsRef<Path>) -> PathBuf {
    memorycore_dir(root).join("index.db")
}

impl ProjectLayout {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let memorycore = memorycore_dir(&root);
        let snapshots = memorycore.join("snapshots");
        Self {
            root,
            index_db: memorycore.join("index.db"),
            sessions: memorycore.join("sessions"),
            snapshot_objects: snapshots.join("objects"),
            snapshot_refs: snapshots.join("refs"),
            snapshots,
            embeddings: memorycore.join("embeddings"),
            plugins: memorycore.join("plugins"),
            skills: memorycore.join("skills"),
            events: memorycore.join("events"),
            logs: memorycore.join("logs"),
            memorycore,
        }
    }
}

pub fn init_project(root: impl AsRef<Path>) -> Result<ProjectLayout> {
    let layout = ProjectLayout::new(root);
    fs::create_dir_all(&layout.sessions).context("create sessions directory")?;
    for agent in ["codex", "claude", "cursor", "antigravity"] {
        fs::create_dir_all(layout.sessions.join(agent))
            .with_context(|| format!("create session directory for {agent}"))?;
    }
    fs::create_dir_all(&layout.snapshot_objects).context("create snapshot objects directory")?;
    fs::create_dir_all(&layout.snapshot_refs).context("create snapshot refs directory")?;
    fs::create_dir_all(&layout.embeddings).context("create embeddings directory")?;
    fs::create_dir_all(&layout.plugins).context("create plugins directory")?;
    fs::create_dir_all(&layout.skills).context("create skills directory")?;
    fs::create_dir_all(&layout.events).context("create events directory")?;
    fs::create_dir_all(&layout.logs).context("create logs directory")?;

    let config_path = layout.memorycore.join("config.toml");
    if !config_path.exists() {
        fs::write(
            &config_path,
            "version = 1\napi_addr = \"127.0.0.1:7330\"\ndashboard_addr = \"127.0.0.1:7331\"\n",
        )
        .context("write default config")?;
    }

    let conn = connect_project_db(&layout.root)?;
    migrate(&conn)?;
    conn.execute(
        "INSERT OR REPLACE INTO project_info (key, value, updated_at) VALUES (?1, ?2, ?3)",
        (
            "root",
            layout.root.to_string_lossy().to_string(),
            now_unix(),
        ),
    )?;
    Ok(layout)
}

pub fn connect_project_db(root: impl AsRef<Path>) -> Result<Connection> {
    let db_path = project_db_path(root);
    let conn = Connection::open(&db_path)
        .with_context(|| format!("open SQLite database at {}", db_path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn append_event(
    conn: &Connection,
    source: &str,
    event_type: &str,
    event_data: &serde_json::Value,
) -> Result<i64> {
    conn.execute(
        r#"
        INSERT INTO event_log (timestamp, source, event_type, event_data)
        VALUES (?1, ?2, ?3, ?4)
        "#,
        (now_unix(), source, event_type, event_data.to_string()),
    )?;
    Ok(conn.last_insert_rowid())
}

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS project_info (
            key TEXT PRIMARY KEY,
            value TEXT,
            updated_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            agent TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            ended_at INTEGER,
            token_count INTEGER NOT NULL DEFAULT 0,
            message_count INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY,
            session_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            metadata TEXT NOT NULL DEFAULT '{}',
            FOREIGN KEY (session_id) REFERENCES sessions(id)
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
            session_id UNINDEXED,
            role,
            content,
            timestamp UNINDEXED,
            tokenize='porter unicode61'
        );

        CREATE TABLE IF NOT EXISTS file_contents (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            content TEXT NOT NULL,
            hash TEXT,
            updated_at INTEGER NOT NULL
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS file_contents_fts USING fts5(
            path UNINDEXED,
            content,
            hash UNINDEXED,
            tokenize='porter unicode61'
        );

        CREATE TABLE IF NOT EXISTS snapshots (
            hash TEXT PRIMARY KEY,
            parent_hash TEXT,
            timestamp INTEGER NOT NULL,
            message TEXT,
            file_count INTEGER,
            total_size INTEGER
        );

        CREATE TABLE IF NOT EXISTS snapshot_files (
            snapshot_hash TEXT NOT NULL,
            path TEXT NOT NULL,
            object_hash TEXT NOT NULL,
            mode INTEGER,
            size INTEGER,
            PRIMARY KEY (snapshot_hash, path),
            FOREIGN KEY (snapshot_hash) REFERENCES snapshots(hash)
        );

        CREATE TABLE IF NOT EXISTS embeddings (
            id INTEGER PRIMARY KEY,
            chunk_type TEXT NOT NULL,
            chunk_id TEXT NOT NULL,
            embedding_offset INTEGER NOT NULL,
            metadata TEXT NOT NULL DEFAULT '{}'
        );

        CREATE TABLE IF NOT EXISTS graph_nodes (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            path TEXT,
            span_start INTEGER,
            span_end INTEGER,
            hash TEXT,
            metadata TEXT NOT NULL DEFAULT '{}',
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS graph_edges (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            weight REAL NOT NULL DEFAULT 1.0,
            confidence REAL NOT NULL DEFAULT 1.0,
            metadata TEXT NOT NULL DEFAULT '{}',
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (source_id) REFERENCES graph_nodes(id),
            FOREIGN KEY (target_id) REFERENCES graph_nodes(id)
        );

        CREATE TABLE IF NOT EXISTS event_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            source TEXT NOT NULL,
            event_type TEXT NOT NULL,
            event_data TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            attempts INTEGER NOT NULL DEFAULT 0,
            error TEXT,
            vector_clock TEXT
        );

        CREATE TABLE IF NOT EXISTS plugins (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            entry TEXT NOT NULL,
            manifest_path TEXT NOT NULL,
            capabilities TEXT NOT NULL DEFAULT '[]',
            hooks TEXT NOT NULL DEFAULT '[]',
            enabled INTEGER NOT NULL DEFAULT 1,
            installed_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            skill_path TEXT NOT NULL,
            description TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            registered_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS adapters (
            id TEXT PRIMARY KEY,
            agent TEXT NOT NULL,
            name TEXT NOT NULL,
            session_dir TEXT,
            command TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            registered_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS memory_cases (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            summary TEXT,
            target TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_embeddings_type ON embeddings(chunk_type);
        CREATE INDEX IF NOT EXISTS idx_graph_nodes_kind ON graph_nodes(kind);
        CREATE INDEX IF NOT EXISTS idx_graph_nodes_path ON graph_nodes(path);
        CREATE INDEX IF NOT EXISTS idx_graph_edges_source ON graph_edges(source_id, kind);
        CREATE INDEX IF NOT EXISTS idx_graph_edges_target ON graph_edges(target_id, kind);
        CREATE INDEX IF NOT EXISTS idx_graph_edges_kind ON graph_edges(kind);
        CREATE INDEX IF NOT EXISTS idx_event_log_status ON event_log(status, id);
        CREATE INDEX IF NOT EXISTS idx_event_log_type ON event_log(event_type, id);
        CREATE INDEX IF NOT EXISTS idx_file_contents_path ON file_contents(path);
        CREATE INDEX IF NOT EXISTS idx_plugins_enabled ON plugins(enabled, id);
        CREATE INDEX IF NOT EXISTS idx_skills_enabled ON skills(enabled, id);
        CREATE INDEX IF NOT EXISTS idx_adapters_enabled ON adapters(enabled, id);
        CREATE INDEX IF NOT EXISTS idx_adapters_agent ON adapters(agent, id);
        CREATE INDEX IF NOT EXISTS idx_memory_cases_created ON memory_cases(created_at, id);
        "#,
    )?;
    Ok(())
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
