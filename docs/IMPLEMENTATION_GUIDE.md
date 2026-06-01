# MemoryCore Implementation Guide

## Phase 1: Core Storage Engine (Week 1-2)

### Day 1-2: Project Setup & SQLite Schema

#### 1.1 Initialize Rust Workspace
```bash
cargo new --lib crates/memorycore-core
cargo new --bin crates/memorycore-cli
cargo new --bin crates/memorycore-api
cargo new --lib crates/memorycore-embeddings
cargo new --lib crates/memorycore-adapters
```

#### 1.2 Core Dependencies (`crates/memorycore-core/Cargo.toml`)
```toml
[dependencies]
tokio = { workspace = true }
sqlx = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }

# Storage
zstd = "0.13"
sha2 = "0.10"
hex = "0.4"

# Time
chrono = { version = "0.4", features = ["serde"] }

# Config
toml = "0.8"

# Async
async-trait = "0.1"
futures = "0.3"

[dev-dependencies]
tempfile = "3.8"
tokio-test = "0.4"
```

#### 1.3 Core Types (`crates/memorycore-core/src/types.rs`)
```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub root_path: PathBuf,
    pub created_at: i64,
    pub last_updated: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub agent: Agent,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub token_count: u32,
    pub message_count: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Codex,
    Claude,
    Cursor,
    Antigravity,
}

impl Agent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Agent::Codex => "codex",
            Agent::Claude => "claude",
            Agent::Cursor => "cursor",
            Agent::Antigravity => "antigravity",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Option<i64>,
    pub session_id: String,
    pub role: Role,
    pub content: String,
    pub timestamp: i64,
    pub metadata: Option<MessageMetadata>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMetadata {
    pub agent: Agent,
    pub tokens: Option<u32>,
    pub files_touched: Vec<String>,
    pub commands_run: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub hash: String,
    pub parent_hash: Option<String>,
    pub timestamp: i64,
    pub message: String,
    pub file_count: u32,
    pub total_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub message: Message,
    pub score: f32,
    pub highlights: Vec<String>,
}
```

#### 1.4 SQLite Storage (`crates/memorycore-core/src/storage/sqlite.rs`)
```rust
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use anyhow::Result;
use std::path::Path;

pub struct SqliteStorage {
    pool: SqlitePool,
}

impl SqliteStorage {
    pub async fn new(db_path: &Path) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&format!("sqlite:{}", db_path.display()))
            .await?;
        
        Ok(Self { pool })
    }

    pub async fn initialize(&self) -> Result<()> {
        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS project_info (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                agent TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                token_count INTEGER DEFAULT 0,
                message_count INTEGER DEFAULT 0
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                metadata TEXT,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Create FTS5 virtual table
        sqlx::query(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                session_id UNINDEXED,
                role,
                content,
                timestamp UNINDEXED,
                tokenize='porter unicode61'
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS snapshots (
                hash TEXT PRIMARY KEY,
                parent_hash TEXT,
                timestamp INTEGER NOT NULL,
                message TEXT NOT NULL,
                file_count INTEGER NOT NULL,
                total_size INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS snapshot_files (
                snapshot_hash TEXT NOT NULL,
                path TEXT NOT NULL,
                object_hash TEXT NOT NULL,
                mode INTEGER NOT NULL,
                size INTEGER NOT NULL,
                PRIMARY KEY (snapshot_hash, path),
                FOREIGN KEY (snapshot_hash) REFERENCES snapshots(hash)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS embeddings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chunk_type TEXT NOT NULL,
                chunk_id TEXT NOT NULL,
                embedding_offset INTEGER NOT NULL,
                metadata TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_embeddings_type ON embeddings(chunk_type)")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn add_message(&self, msg: &Message) -> Result<i64> {
        let metadata_json = msg.metadata.as_ref()
            .map(|m| serde_json::to_string(m))
            .transpose()?;

        let result = sqlx::query(
            r#"
            INSERT INTO messages (session_id, role, content, timestamp, metadata)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(&msg.session_id)
        .bind(format!("{:?}", msg.role).to_lowercase())
        .bind(&msg.content)
        .bind(msg.timestamp)
        .bind(metadata_json)
        .execute(&self.pool)
        .await?;

        let id = result.last_insert_rowid();

        // Insert into FTS
        sqlx::query(
            r#"
            INSERT INTO messages_fts (rowid, session_id, role, content, timestamp)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(id)
        .bind(&msg.session_id)
        .bind(format!("{:?}", msg.role).to_lowercase())
        .bind(&msg.content)
        .bind(msg.timestamp)
        .execute(&self.pool)
        .await?;

        Ok(id)
    }

    pub async fn search_messages(&self, query: &str, limit: usize) -> Result<Vec<Message>> {
        let rows = sqlx::query(
            r#"
            SELECT m.id, m.session_id, m.role, m.content, m.timestamp, m.metadata
            FROM messages m
            JOIN messages_fts fts ON m.id = fts.rowid
            WHERE messages_fts MATCH ?
            ORDER BY rank
            LIMIT ?
            "#,
        )
        .bind(query)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let messages = rows
            .into_iter()
            .map(|row| {
                let role_str: String = row.get("role");
                let role = match role_str.as_str() {
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    "system" => Role::System,
                    _ => Role::User,
                };

                let metadata_json: Option<String> = row.get("metadata");
                let metadata = metadata_json
                    .and_then(|json| serde_json::from_str(&json).ok());

                Message {
                    id: Some(row.get("id")),
                    session_id: row.get("session_id"),
                    role,
                    content: row.get("content"),
                    timestamp: row.get("timestamp"),
                    metadata,
                }
            })
            .collect();

        Ok(messages)
    }

    pub async fn create_session(&self, session: &Session) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO sessions (id, agent, started_at, ended_at, token_count, message_count)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&session.id)
        .bind(session.agent.as_str())
        .bind(session.started_at)
        .bind(session.ended_at)
        .bind(session.token_count)
        .bind(session.message_count)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
        let row = sqlx::query(
            r#"
            SELECT id, agent, started_at, ended_at, token_count, message_count
            FROM sessions
            WHERE id = ?
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            let agent_str: String = r.get("agent");
            let agent = match agent_str.as_str() {
                "codex" => Agent::Codex,
                "claude" => Agent::Claude,
                "cursor" => Agent::Cursor,
                "antigravity" => Agent::Antigravity,
                _ => Agent::Codex,
            };

            Session {
                id: r.get("id"),
                agent,
                started_at: r.get("started_at"),
                ended_at: r.get("ended_at"),
                token_count: r.get("token_count"),
                message_count: r.get("message_count"),
            }
        }))
    }
}
```

### Day 3-4: Session Storage with Compression

#### 1.5 Session Storage (`crates/memorycore-core/src/storage/session.rs`)
```rust
use anyhow::Result;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use zstd::stream::write::Encoder;

pub struct SessionStorage {
    base_path: PathBuf,
    compression_level: i32,
}

impl SessionStorage {
    pub fn new(base_path: PathBuf, compression_level: i32) -> Self {
        Self {
            base_path,
            compression_level,
        }
    }

    pub fn init(&self) -> Result<()> {
        for agent in &["codex", "claude", "cursor", "antigravity"] {
            let agent_dir = self.base_path.join(agent);
            fs::create_dir_all(&agent_dir)?;
        }
        Ok(())
    }

    pub fn write_session(&self, agent: &str, session_id: &str, data: &[u8]) -> Result<()> {
        let file_path = self.base_path
            .join(agent)
            .join(format!("{}.jsonl.zst", session_id));

        let file = File::create(file_path)?;
        let buf_writer = BufWriter::new(file);
        let mut encoder = Encoder::new(buf_writer, self.compression_level)?;
        
        encoder.write_all(data)?;
        encoder.finish()?;

        Ok(())
    }

    pub fn read_session(&self, agent: &str, session_id: &str) -> Result<Vec<u8>> {
        let file_path = self.base_path
            .join(agent)
            .join(format!("{}.jsonl.zst", session_id));

        let file = File::open(file_path)?;
        let mut decoder = zstd::stream::read::Decoder::new(file)?;
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut data)?;

        Ok(data)
    }

    pub fn list_sessions(&self, agent: &str) -> Result<Vec<String>> {
        let agent_dir = self.base_path.join(agent);
        let mut sessions = Vec::new();

        for entry in fs::read_dir(agent_dir)? {
            let entry = entry?;
            let path = entry.path();
            
            if let Some(file_name) = path.file_stem() {
                if let Some(name) = file_name.to_str() {
                    // Remove .jsonl extension
                    let session_id = name.trim_end_matches(".jsonl");
                    sessions.push(session_id.to_string());
                }
            }
        }

        Ok(sessions)
    }
}
```

### Day 5-7: Snapshot Engine with Delta Compression

#### 1.6 Snapshot Engine (`crates/memorycore-core/src/storage/snapshot.rs`)
```rust
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zstd::stream::write::Encoder;

pub struct SnapshotEngine {
    objects_dir: PathBuf,
    refs_dir: PathBuf,
}

impl SnapshotEngine {
    pub fn new(base_path: PathBuf) -> Self {
        Self {
            objects_dir: base_path.join("objects"),
            refs_dir: base_path.join("refs"),
        }
    }

    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.objects_dir)?;
        fs::create_dir_all(&self.refs_dir)?;
        Ok(())
    }

    /// Store file content and return its hash
    pub fn store_object(&self, content: &[u8]) -> Result<String> {
        let hash = self.hash_content(content);
        let object_path = self.object_path(&hash);

        // Skip if already exists
        if object_path.exists() {
            return Ok(hash);
        }

        // Create parent directory
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Compress and write
        let file = File::create(object_path)?;
        let mut encoder = Encoder::new(file, 3)?;
        encoder.write_all(content)?;
        encoder.finish()?;

        Ok(hash)
    }

    /// Retrieve object content by hash
    pub fn get_object(&self, hash: &str) -> Result<Vec<u8>> {
        let object_path = self.object_path(hash);
        let file = File::open(object_path)
            .context("Object not found")?;
        
        let mut decoder = zstd::stream::read::Decoder::new(file)?;
        let mut content = Vec::new();
        decoder.read_to_end(&mut content)?;

        Ok(content)
    }

    /// Create snapshot from directory
    pub fn create_snapshot(
        &self,
        root_path: &Path,
        message: &str,
        parent_hash: Option<&str>,
    ) -> Result<String> {
        let mut files = Vec::new();
        let mut total_size = 0u64;

        // Walk directory and store files
        for entry in walkdir::WalkDir::new(root_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !self.should_ignore(e.path()))
        {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                let content = fs::read(path)?;
                let object_hash = self.store_object(&content)?;
                
                let relative_path = path.strip_prefix(root_path)?;
                let metadata = fs::metadata(path)?;
                
                files.push(SnapshotFile {
                    path: relative_path.to_string_lossy().to_string(),
                    object_hash,
                    mode: Self::get_mode(&metadata),
                    size: content.len() as u64,
                });

                total_size += content.len() as u64;
            }
        }

        // Create snapshot metadata
        let snapshot = SnapshotMetadata {
            parent_hash: parent_hash.map(String::from),
            timestamp: chrono::Utc::now().timestamp(),
            message: message.to_string(),
            files,
        };

        let snapshot_json = serde_json::to_string(&snapshot)?;
        let snapshot_hash = self.hash_content(snapshot_json.as_bytes());

        // Store snapshot metadata
        let snapshot_path = self.refs_dir.join(&snapshot_hash);
        fs::write(snapshot_path, snapshot_json)?;

        Ok(snapshot_hash)
    }

    fn hash_content(&self, content: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        hex::encode(hasher.finalize())
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        // Git-style: objects/ab/cdef...
        let (prefix, suffix) = hash.split_at(2);
        self.objects_dir.join(prefix).join(suffix)
    }

    fn should_ignore(&self, path: &Path) -> bool {
        let ignore_patterns = [
            ".git",
            ".memorycore",
            "node_modules",
            "target",
            ".DS_Store",
        ];

        path.components().any(|c| {
            if let Some(s) = c.as_os_str().to_str() {
                ignore_patterns.contains(&s)
            } else {
                false
            }
        })
    }

    #[cfg(unix)]
    fn get_mode(metadata: &fs::Metadata) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    }

    #[cfg(not(unix))]
    fn get_mode(_metadata: &fs::Metadata) -> u32 {
        0o644
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotMetadata {
    parent_hash: Option<String>,
    timestamp: i64,
    message: String,
    files: Vec<SnapshotFile>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotFile {
    path: String,
    object_hash: String,
    mode: u32,
    size: u64,
}
```

### Day 8-10: CLI Implementation

#### 1.7 CLI Main (`crates/memorycore-cli/src/main.rs`)
```rust
use clap::{Parser, Subcommand};
use anyhow::Result;

mod commands;

#[derive(Parser)]
#[command(name = "memorycore")]
#[command(about = "Core memory for coding projects", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize MemoryCore in current directory
    Init,
    
    /// Show status and statistics
    Status,
    
    /// Search through project memory
    Search {
        /// Search query
        query: String,
        
        /// Search type: all, code, chat, semantic
        #[arg(short, long, default_value = "all")]
        r#type: String,
        
        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
        
        /// Filter by agent
        #[arg(short, long)]
        agent: Option<String>,
    },
    
    /// Manage sessions
    Sessions {
        #[command(subcommand)]
        command: SessionCommands,
    },
    
    /// Manage snapshots
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommands,
    },
    
    /// Start MCP server
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List all sessions
    List {
        #[arg(short, long)]
        agent: Option<String>,
    },
    
    /// Show session details
    Show {
        session_id: String,
    },
    
    /// Export session
    Export {
        session_id: String,
        
        #[arg(short, long, default_value = "markdown")]
        format: String,
    },
}

#[derive(Subcommand)]
enum SnapshotCommands {
    /// Create a new snapshot
    Create {
        /// Snapshot message
        message: String,
    },
    
    /// List all snapshots
    List,
    
    /// Show diff between snapshots
    Diff {
        hash1: String,
        hash2: String,
    },
}

#[derive(Subcommand)]
enum McpCommands {
    /// Start MCP server
    Serve {
        #[arg(short, long, default_value = "stdio")]
        transport: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Init => commands::init::run().await,
        Commands::Status => commands::status::run().await,
        Commands::Search { query, r#type, limit, agent } => {
            commands::search::run(&query, &r#type, limit, agent.as_deref()).await
        }
        Commands::Sessions { command } => {
            commands::sessions::run(command).await
        }
        Commands::Snapshot { command } => {
            commands::snapshot::run(command).await
        }
        Commands::Mcp { command } => {
            commands::mcp::run(command).await
        }
    }
}
```

This provides the foundation. Continue with Phase 2?
