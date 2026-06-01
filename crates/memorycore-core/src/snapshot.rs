use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use walkdir::{DirEntry, WalkDir};

use crate::{append_event, now_unix, ProjectLayout};

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotRecord {
    pub hash: String,
    pub timestamp: i64,
    pub message: String,
    pub file_count: i64,
    pub total_size: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotFileRecord {
    pub path: String,
    pub object_hash: String,
    pub size: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotDetails {
    pub snapshot: SnapshotRecord,
    pub files: Vec<SnapshotFileRecord>,
}

#[derive(Debug, Clone)]
pub struct SnapshotOutcome {
    pub record: SnapshotRecord,
    pub event_id: i64,
}

pub fn create_snapshot(
    project_root: &Path,
    conn: &Connection,
    message: &str,
    source: &str,
) -> Result<SnapshotOutcome> {
    let objects_dir = ProjectLayout::new(project_root).snapshot_objects;
    fs::create_dir_all(&objects_dir).context("create snapshot objects directory")?;

    let mut files = Vec::new();
    let mut total_size = 0_u64;
    let mut snapshot_hasher = Sha256::new();
    let timestamp = now_unix();

    for entry in WalkDir::new(project_root)
        .into_iter()
        .filter_entry(|entry| !is_ignored(entry))
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || is_probably_binary(path) {
            continue;
        }
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let object_hash = format!("{:x}", Sha256::digest(&bytes));
        let rel = path
            .strip_prefix(project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        fs::write(objects_dir.join(&object_hash), &bytes)
            .with_context(|| format!("write snapshot object {object_hash}"))?;
        snapshot_hasher.update(rel.as_bytes());
        snapshot_hasher.update(object_hash.as_bytes());
        total_size += bytes.len() as u64;
        files.push((rel, object_hash, bytes.len() as i64));
    }

    snapshot_hasher.update(message.as_bytes());
    snapshot_hasher.update(timestamp.to_string().as_bytes());
    let snapshot_hash = format!("{:x}", snapshot_hasher.finalize());

    conn.execute(
        r#"
        INSERT INTO snapshots (hash, timestamp, message, file_count, total_size)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            &snapshot_hash,
            timestamp,
            message,
            files.len() as i64,
            total_size as i64
        ],
    )?;

    let project_name = project_root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| project_root.display().to_string());
    conn.execute(
        r#"
        INSERT INTO graph_nodes
            (id, kind, name, path, span_start, span_end, hash, metadata, updated_at)
        VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5, ?6)
        ON CONFLICT(id) DO UPDATE SET
            kind=excluded.kind,
            name=excluded.name,
            path=excluded.path,
            span_start=excluded.span_start,
            span_end=excluded.span_end,
            hash=excluded.hash,
            metadata=excluded.metadata,
            updated_at=excluded.updated_at
        "#,
        params![
            "project:root",
            "Project",
            project_name,
            ".",
            serde_json::json!({}).to_string(),
            timestamp
        ],
    )?;

    let snapshot_node_id = format!("snapshot:{snapshot_hash}");
    conn.execute(
        r#"
        INSERT INTO graph_nodes
            (id, kind, name, path, span_start, span_end, hash, metadata, updated_at)
        VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, ?6, ?7)
        ON CONFLICT(id) DO UPDATE SET
            kind=excluded.kind,
            name=excluded.name,
            path=excluded.path,
            span_start=excluded.span_start,
            span_end=excluded.span_end,
            hash=excluded.hash,
            metadata=excluded.metadata,
            updated_at=excluded.updated_at
        "#,
        params![
            &snapshot_node_id,
            "Snapshot",
            message,
            format!("snapshots/{snapshot_hash}"),
            &snapshot_hash,
            serde_json::json!({
                "message": message,
                "file_count": files.len(),
                "total_size": total_size,
                "source": source
            })
            .to_string(),
            timestamp
        ],
    )?;

    for (path, object_hash, size) in &files {
        let file_node_id = format!("file:{path}");
        let file_name = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        conn.execute(
            r#"
            INSERT INTO graph_nodes
                (id, kind, name, path, span_start, span_end, hash, metadata, updated_at)
            VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                kind=excluded.kind,
                name=excluded.name,
                path=excluded.path,
                span_start=excluded.span_start,
                span_end=excluded.span_end,
                hash=excluded.hash,
                metadata=excluded.metadata,
                updated_at=excluded.updated_at
            "#,
            params![
                &file_node_id,
                "File",
                file_name,
                path,
                object_hash,
                serde_json::json!({
                    "size": size,
                    "captured_by": &snapshot_hash
                })
                .to_string(),
                timestamp
            ],
        )?;
        conn.execute(
            r#"
            INSERT INTO graph_edges
                (id, source_id, target_id, kind, weight, confidence, metadata, updated_at)
            VALUES (?1, ?2, ?3, ?4, 1.0, 1.0, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                source_id=excluded.source_id,
                target_id=excluded.target_id,
                kind=excluded.kind,
                weight=excluded.weight,
                confidence=excluded.confidence,
                metadata=excluded.metadata,
                updated_at=excluded.updated_at
            "#,
            params![
                format!("edge:{snapshot_node_id}:contains:{file_node_id}"),
                &snapshot_node_id,
                &file_node_id,
                "contains",
                serde_json::json!({
                    "snapshot_hash": &snapshot_hash,
                    "path": path,
                    "size": size
                })
                .to_string(),
                timestamp
            ],
        )?;
    }

    conn.execute(
        r#"
        INSERT INTO graph_edges
            (id, source_id, target_id, kind, weight, confidence, metadata, updated_at)
        VALUES (?1, ?2, ?3, ?4, 1.0, 1.0, ?5, ?6)
        ON CONFLICT(id) DO UPDATE SET
            source_id=excluded.source_id,
            target_id=excluded.target_id,
            kind=excluded.kind,
            weight=excluded.weight,
            confidence=excluded.confidence,
            metadata=excluded.metadata,
            updated_at=excluded.updated_at
        "#,
        params![
            format!("edge:project:root:contains:{snapshot_node_id}"),
            "project:root",
            &snapshot_node_id,
            "contains",
            serde_json::json!({
                "source": source,
                "snapshot_hash": &snapshot_hash,
                "file_count": files.len(),
                "total_size": total_size
            })
            .to_string(),
            timestamp
        ],
    )?;

    for (path, object_hash, size) in &files {
        conn.execute(
            r#"
            INSERT OR REPLACE INTO snapshot_files
                (snapshot_hash, path, object_hash, size)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![&snapshot_hash, path, object_hash, size],
        )?;
    }

    let event_id = append_event(
        conn,
        source,
        "snapshot_created",
        &serde_json::json!({
            "hash": &snapshot_hash,
            "message": message,
            "file_count": files.len(),
            "total_size": total_size
        }),
    )?;

    Ok(SnapshotOutcome {
        record: SnapshotRecord {
            hash: snapshot_hash,
            timestamp,
            message: message.to_string(),
            file_count: files.len() as i64,
            total_size: total_size as i64,
        },
        event_id,
    })
}

pub fn list_snapshots(conn: &Connection, limit: usize) -> Result<Vec<SnapshotRecord>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT hash, timestamp, COALESCE(message, ''), COALESCE(file_count, 0), COALESCE(total_size, 0)
        FROM snapshots
        ORDER BY timestamp DESC, hash DESC
        LIMIT ?1
        "#,
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(SnapshotRecord {
            hash: row.get(0)?,
            timestamp: row.get(1)?,
            message: row.get(2)?,
            file_count: row.get(3)?,
            total_size: row.get(4)?,
        })
    })?;
    let mut snapshots = Vec::new();
    for row in rows {
        snapshots.push(row?);
    }
    Ok(snapshots)
}

pub fn snapshot_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))?)
}

pub fn snapshot_details(conn: &Connection, hash: &str) -> Result<Option<SnapshotDetails>> {
    let snapshot = conn
        .query_row(
            r#"
            SELECT hash, timestamp, COALESCE(message, ''), COALESCE(file_count, 0), COALESCE(total_size, 0)
            FROM snapshots
            WHERE hash = ?1
            "#,
            [hash],
            |row| {
                Ok(SnapshotRecord {
                    hash: row.get(0)?,
                    timestamp: row.get(1)?,
                    message: row.get(2)?,
                    file_count: row.get(3)?,
                    total_size: row.get(4)?,
                })
            },
        )
        .ok();
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };

    let mut stmt = conn.prepare(
        r#"
        SELECT path, object_hash, COALESCE(size, 0)
        FROM snapshot_files
        WHERE snapshot_hash = ?1
        ORDER BY path
        "#,
    )?;
    let files = stmt
        .query_map([hash], |row| {
            Ok(SnapshotFileRecord {
                path: row.get(0)?,
                object_hash: row.get(1)?,
                size: row.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(Some(SnapshotDetails { snapshot, files }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_project;
    use tempfile::tempdir;

    #[test]
    fn snapshot_listing_and_details_round_trip() -> Result<()> {
        let temp = tempdir()?;
        std::fs::create_dir_all(temp.path().join("src"))?;
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;

        init_project(temp.path())?;
        let conn = crate::connect_project_db(temp.path())?;
        let outcome = create_snapshot(temp.path(), &conn, "test snapshot", "memorycore-test")?;

        let snapshots = list_snapshots(&conn, 5)?;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].hash, outcome.record.hash);
        assert_eq!(snapshots[0].message, "test snapshot");

        let snapshot_node_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE id = ?1 AND kind = 'Snapshot'",
            [format!("snapshot:{}", outcome.record.hash)],
            |row| row.get(0),
        )?;
        assert_eq!(snapshot_node_count, 1);

        let snapshot_edge_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_edges WHERE source_id = 'project:root' AND target_id = ?1 AND kind = 'contains'",
                [format!("snapshot:{}", outcome.record.hash)],
                |row| row.get(0),
            )?;
        assert_eq!(snapshot_edge_count, 1);

        let details = snapshot_details(&conn, &outcome.record.hash)?.expect("snapshot details");
        assert_eq!(details.snapshot.hash, outcome.record.hash);
        assert_eq!(details.files.len(), 1);
        assert_eq!(details.files[0].path, "src/main.rs");
        Ok(())
    }
}
