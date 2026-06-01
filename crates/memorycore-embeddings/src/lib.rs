use anyhow::{Context, Result};
use memorycore_core::{connect_project_db, now_unix, ProjectLayout};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::fd::AsRawFd;

const EMBEDDING_DIMS: usize = 64;
const MAGIC: &[u8; 4] = b"MCEM";
const VERSION: u32 = 3;
const SINGLE_LAYER_VERSION: u32 = 2;
const LEGACY_VERSION: u32 = 1;
const GRAPH_MAGIC: &[u8; 4] = b"MCNG";
const GRAPH_DEGREE: usize = 8;
const GRAPH_LEVELS: usize = 4;

#[derive(Debug, Clone)]
pub struct EmbeddingRecord {
    pub chunk_type: String,
    pub chunk_id: String,
    pub metadata: serde_json::Value,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingSearchHit {
    pub score: f32,
    pub id: i64,
    pub chunk_type: String,
    pub chunk_id: String,
    pub embedding_offset: i64,
    pub metadata: serde_json::Value,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone)]
struct StoreRecord {
    chunk_type: String,
    chunk_id: String,
    vector: Vec<f32>,
}

#[derive(Debug, Clone)]
struct EmbeddingStore {
    records: Vec<StoreRecord>,
    layers: Vec<Vec<Vec<usize>>>,
}

pub fn crate_status() -> &'static str {
    "memorycore-embeddings ready"
}

pub fn embeddings_path(project_root: &Path) -> PathBuf {
    ProjectLayout::new(project_root)
        .embeddings
        .join("chunks.bin")
}

pub fn build_message_embeddings(project_root: &Path) -> Result<usize> {
    let conn = connect_project_db(project_root)?;
    build_message_embeddings_with_conn(project_root, &conn)
}

pub fn build_message_embeddings_with_conn(project_root: &Path, conn: &Connection) -> Result<usize> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, session_id, role, content, timestamp, metadata
        FROM messages
        ORDER BY session_id, timestamp, id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        let metadata_text: String = row.get(5)?;
        let metadata = serde_json::from_str(&metadata_text).unwrap_or_else(|_| json!({}));
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            metadata,
        ))
    })?;

    let mut records = Vec::new();
    for row in rows {
        let (message_id, session_id, role, content, timestamp, metadata) = row?;
        let chunk_id = format!("message:{session_id}:{message_id}");
        let vector = embed_text(&content, EMBEDDING_DIMS);
        records.push(EmbeddingRecord {
            chunk_type: "message".to_string(),
            chunk_id,
            metadata: json!({
                "message_id": message_id,
                "session_id": session_id,
                "role": role,
                "timestamp": timestamp,
                "source_metadata": metadata,
            }),
            vector,
        });
    }

    write_store(project_root, conn, &records)?;
    Ok(records.len())
}

pub fn list_embeddings(project_root: &Path) -> Result<Vec<serde_json::Value>> {
    let conn = connect_project_db(project_root)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, chunk_type, chunk_id, embedding_offset, metadata
        FROM embeddings
        ORDER BY id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        let metadata_text: String = row.get(4)?;
        let metadata = serde_json::from_str(&metadata_text).unwrap_or_else(|_| json!({}));
        Ok(json!({
            "id": row.get::<_, i64>(0)?,
            "chunk_type": row.get::<_, String>(1)?,
            "chunk_id": row.get::<_, String>(2)?,
            "embedding_offset": row.get::<_, i64>(3)?,
            "metadata": metadata
        }))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn search_embeddings(
    project_root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<EmbeddingSearchHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let conn = connect_project_db(project_root)?;
    search_embeddings_with_conn(project_root, &conn, query, limit)
}

pub fn search_embeddings_with_conn(
    project_root: &Path,
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<EmbeddingSearchHit>> {
    let query_vector = embed_text(query, EMBEDDING_DIMS);
    let store = read_embedding_store(project_root)?;
    let mut hits = Vec::new();

    for index in ann_candidate_indices(&store, &query_vector, limit) {
        let record = &store.records[index];
        let score = dot_product(&query_vector, &record.vector);
        if score <= 0.0 {
            continue;
        }
        let Some((id, embedding_offset, metadata)) =
            embedding_metadata(conn, &record.chunk_type, &record.chunk_id)?
        else {
            continue;
        };
        let snippet = embedding_snippet(conn, &record.chunk_type, &metadata)?;
        hits.push(EmbeddingSearchHit {
            score,
            id,
            chunk_type: record.chunk_type.clone(),
            chunk_id: record.chunk_id.clone(),
            embedding_offset,
            metadata,
            snippet,
        });
    }

    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.chunk_type.cmp(&right.chunk_type))
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    hits.truncate(limit.max(1));
    Ok(hits)
}

pub fn read_store_summary(project_root: &Path) -> Result<Vec<(String, String, Vec<f32>)>> {
    let store = read_embedding_store(project_root)?;
    Ok(store
        .records
        .into_iter()
        .map(|record| (record.chunk_type, record.chunk_id, record.vector))
        .collect())
}

fn read_embedding_store(project_root: &Path) -> Result<EmbeddingStore> {
    let path = embeddings_path(project_root);
    let store_bytes = StoreBytes::open(&path)
        .with_context(|| format!("open embedding store {}", path.display()))?;
    let mut file = Cursor::new(store_bytes.as_slice());
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        anyhow::bail!("invalid embeddings store magic");
    }
    let version = read_u32(&mut file)?;
    if version != VERSION && version != SINGLE_LAYER_VERSION && version != LEGACY_VERSION {
        anyhow::bail!("unsupported embeddings store version {version}");
    }
    let dims = read_u32(&mut file)? as usize;
    if dims != EMBEDDING_DIMS {
        anyhow::bail!("unexpected embedding dims {dims}");
    }
    let count = read_u64(&mut file)? as usize;
    let mut records = Vec::new();
    for _ in 0..count {
        let chunk_type = read_string(&mut file)?;
        let chunk_id = read_string(&mut file)?;
        let mut vector = vec![0_f32; dims];
        for slot in &mut vector {
            *slot = read_f32(&mut file)?;
        }
        records.push(StoreRecord {
            chunk_type,
            chunk_id,
            vector,
        });
    }

    let layers = if version == VERSION {
        read_layered_neighbor_graph(&mut file, count)?
    } else if version == SINGLE_LAYER_VERSION {
        let mut graph_magic = [0_u8; 4];
        file.read_exact(&mut graph_magic)?;
        if &graph_magic != GRAPH_MAGIC {
            anyhow::bail!("invalid embeddings graph magic");
        }
        let _degree = read_u32(&mut file)? as usize;
        let graph_count = read_u64(&mut file)? as usize;
        if graph_count != count {
            anyhow::bail!(
                "embedding graph count {graph_count} does not match record count {count}"
            );
        }
        let mut graph = Vec::with_capacity(count);
        for _ in 0..count {
            let neighbor_count = read_u32(&mut file)? as usize;
            let mut row = Vec::with_capacity(neighbor_count);
            for _ in 0..neighbor_count {
                let index = read_u64(&mut file)? as usize;
                if index < count {
                    row.push(index);
                }
            }
            graph.push(row);
        }
        vec![graph]
    } else {
        vec![vec![Vec::new(); count]]
    };

    Ok(EmbeddingStore { records, layers })
}

fn read_layered_neighbor_graph(
    file: &mut Cursor<&[u8]>,
    record_count: usize,
) -> Result<Vec<Vec<Vec<usize>>>> {
    let mut graph_magic = [0_u8; 4];
    file.read_exact(&mut graph_magic)?;
    if &graph_magic != GRAPH_MAGIC {
        anyhow::bail!("invalid embeddings graph magic");
    }
    let _degree = read_u32(file)? as usize;
    let level_count = read_u32(file)? as usize;
    let graph_count = read_u64(file)? as usize;
    if graph_count != record_count {
        anyhow::bail!(
            "embedding graph count {graph_count} does not match record count {record_count}"
        );
    }

    let mut layers = Vec::with_capacity(level_count);
    for _ in 0..level_count {
        let mut layer = Vec::with_capacity(record_count);
        for _ in 0..record_count {
            let neighbor_count = read_u32(file)? as usize;
            let mut row = Vec::with_capacity(neighbor_count);
            for _ in 0..neighbor_count {
                let index = read_u64(file)? as usize;
                if index < record_count {
                    row.push(index);
                }
            }
            layer.push(row);
        }
        layers.push(layer);
    }
    if layers.is_empty() {
        layers.push(vec![Vec::new(); record_count]);
    }
    Ok(layers)
}

enum StoreBytes {
    #[cfg(unix)]
    Mmap { ptr: *mut libc::c_void, len: usize },
    #[cfg(not(unix))]
    Bytes(Vec<u8>),
}

impl StoreBytes {
    fn open(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            let file = OpenOptions::new().read(true).open(path)?;
            let len = file.metadata()?.len() as usize;
            if len == 0 {
                anyhow::bail!("empty embedding store");
            }
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ,
                    libc::MAP_PRIVATE,
                    file.as_raw_fd(),
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error()).context("mmap embedding store");
            }
            Ok(Self::Mmap { ptr, len })
        }
        #[cfg(not(unix))]
        {
            Ok(Self::Bytes(std::fs::read(path)?))
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            #[cfg(unix)]
            Self::Mmap { ptr, len } => unsafe {
                std::slice::from_raw_parts(*ptr as *const u8, *len)
            },
            #[cfg(not(unix))]
            Self::Bytes(bytes) => bytes,
        }
    }
}

impl Drop for StoreBytes {
    fn drop(&mut self) {
        #[cfg(unix)]
        match self {
            Self::Mmap { ptr, len } => {
                unsafe {
                    libc::munmap(*ptr, *len);
                };
            }
        }
        #[cfg(not(unix))]
        match self {
            Self::Bytes(_) => {}
        }
    }
}

fn ann_candidate_indices(store: &EmbeddingStore, query_vector: &[f32], limit: usize) -> Vec<usize> {
    let count = store.records.len();
    if count == 0 {
        return Vec::new();
    }
    let Some(base_layer) = store.layers.first() else {
        return (0..count).collect();
    };
    if base_layer.iter().all(Vec::is_empty) {
        return (0..count).collect();
    }

    let visit_budget = count.min((limit.max(1) * GRAPH_DEGREE * 4).max(32));
    let mut visited = vec![false; count];
    let mut expanded = vec![false; count];
    let mut candidates = Vec::<(f32, usize)>::new();
    let entry = hierarchical_entry_point(store, query_vector);

    push_ann_candidate(entry, store, query_vector, &mut visited, &mut candidates);
    for seed in ann_seed_indices(count) {
        push_ann_candidate(seed, store, query_vector, &mut visited, &mut candidates);
    }

    while visited.iter().filter(|seen| **seen).count() < visit_budget {
        let Some((_, index)) = candidates
            .iter()
            .filter(|(_, index)| !expanded[*index])
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
            })
            .copied()
        else {
            break;
        };
        expanded[index] = true;
        for &neighbor in &base_layer[index] {
            if visited.iter().filter(|seen| **seen).count() >= visit_budget {
                break;
            }
            push_ann_candidate(neighbor, store, query_vector, &mut visited, &mut candidates);
        }
    }

    let mut indices = candidates
        .into_iter()
        .map(|(_, index)| index)
        .collect::<Vec<_>>();
    indices.sort_unstable();
    indices.dedup();
    if indices.len() < limit.min(count) {
        for index in 0..count {
            if !indices.contains(&index) {
                indices.push(index);
            }
            if indices.len() >= limit.min(count) {
                break;
            }
        }
    }
    indices
}

fn hierarchical_entry_point(store: &EmbeddingStore, query_vector: &[f32]) -> usize {
    let count = store.records.len();
    let mut current = ann_seed_indices(count)
        .into_iter()
        .max_by(|left, right| {
            let left_score = dot_product(query_vector, &store.records[*left].vector);
            let right_score = dot_product(query_vector, &store.records[*right].vector);
            left_score
                .total_cmp(&right_score)
                .then_with(|| right.cmp(left))
        })
        .unwrap_or(0);

    for level in (1..store.layers.len()).rev() {
        current = greedy_descent_layer(store, query_vector, level, current);
    }
    current
}

fn greedy_descent_layer(
    store: &EmbeddingStore,
    query_vector: &[f32],
    level: usize,
    mut current: usize,
) -> usize {
    let Some(layer) = store.layers.get(level) else {
        return current;
    };
    loop {
        let mut best = current;
        let mut best_score = dot_product(query_vector, &store.records[current].vector);
        for &neighbor in layer.get(current).into_iter().flatten() {
            let score = dot_product(query_vector, &store.records[neighbor].vector);
            if score > best_score {
                best = neighbor;
                best_score = score;
            }
        }
        if best == current {
            return current;
        }
        current = best;
    }
}

fn ann_seed_indices(count: usize) -> Vec<usize> {
    let mut seeds = vec![
        0,
        count / 4,
        count / 2,
        (count * 3) / 4,
        count.saturating_sub(1),
    ];
    seeds.sort_unstable();
    seeds.dedup();
    seeds
}

fn push_ann_candidate(
    index: usize,
    store: &EmbeddingStore,
    query_vector: &[f32],
    visited: &mut [bool],
    candidates: &mut Vec<(f32, usize)>,
) {
    if index >= store.records.len() || visited[index] {
        return;
    }
    visited[index] = true;
    candidates.push((
        dot_product(query_vector, &store.records[index].vector),
        index,
    ));
}

fn embedding_metadata(
    conn: &Connection,
    chunk_type: &str,
    chunk_id: &str,
) -> Result<Option<(i64, i64, serde_json::Value)>> {
    let row = conn
        .query_row(
            r#"
            SELECT id, embedding_offset, metadata
            FROM embeddings
            WHERE chunk_type = ?1 AND chunk_id = ?2
            LIMIT 1
            "#,
            (chunk_type, chunk_id),
            |row| {
                let metadata_text: String = row.get(2)?;
                let metadata = serde_json::from_str(&metadata_text).unwrap_or_else(|_| json!({}));
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, metadata))
            },
        )
        .ok();
    Ok(row)
}

fn embedding_snippet(
    conn: &Connection,
    chunk_type: &str,
    metadata: &serde_json::Value,
) -> Result<Option<String>> {
    if chunk_type != "message" {
        return Ok(None);
    }
    let Some(message_id) = metadata
        .get("message_id")
        .and_then(serde_json::Value::as_i64)
    else {
        return Ok(None);
    };
    let content = conn
        .query_row(
            "SELECT content FROM messages WHERE id = ?1 LIMIT 1",
            [message_id],
            |row| row.get::<_, String>(0),
        )
        .ok();
    Ok(content.map(|content| truncate_snippet(&content, 240)))
}

fn write_store(project_root: &Path, conn: &Connection, records: &[EmbeddingRecord]) -> Result<()> {
    let layout = ProjectLayout::new(project_root);
    fs::create_dir_all(&layout.embeddings)?;
    let path = embeddings_path(project_root);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("open embeddings store {}", path.display()))?;
    file.write_all(MAGIC)?;
    write_u32(&mut file, VERSION)?;
    write_u32(&mut file, EMBEDDING_DIMS as u32)?;
    write_u64(&mut file, records.len() as u64)?;
    let layers = build_neighbor_layers(records);

    conn.execute("DELETE FROM embeddings", [])?;
    for (index, record) in records.iter().enumerate() {
        write_string(&mut file, &record.chunk_type)?;
        write_string(&mut file, &record.chunk_id)?;
        for value in &record.vector {
            write_f32(&mut file, *value)?;
        }
        conn.execute(
            r#"
            INSERT INTO embeddings (id, chunk_type, chunk_id, embedding_offset, metadata)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            (
                index as i64,
                &record.chunk_type,
                &record.chunk_id,
                index as i64,
                record.metadata.to_string(),
            ),
        )?;
    }
    write_neighbor_layers(&mut file, &layers)?;

    conn.execute(
        r#"
        INSERT OR REPLACE INTO project_info (key, value, updated_at)
        VALUES ('embeddings_path', ?1, ?2)
        "#,
        (path.to_string_lossy().to_string(), now_unix()),
    )?;
    Ok(())
}

fn build_neighbor_layers(records: &[EmbeddingRecord]) -> Vec<Vec<Vec<usize>>> {
    let node_levels = records
        .iter()
        .map(|record| graph_level_for_chunk(&record.chunk_id))
        .collect::<Vec<_>>();
    let max_level = node_levels.iter().copied().max().unwrap_or(0);
    let mut layers = Vec::with_capacity(max_level + 1);
    for level in 0..=max_level {
        let mut layer = Vec::with_capacity(records.len());
        for (index, record) in records.iter().enumerate() {
            if node_levels[index] < level {
                layer.push(Vec::new());
                continue;
            }
            let mut scored = records
                .iter()
                .enumerate()
                .filter(|(other_index, _)| {
                    *other_index != index && node_levels[*other_index] >= level
                })
                .map(|(other_index, other)| {
                    (dot_product(&record.vector, &other.vector), other_index)
                })
                .collect::<Vec<_>>();
            scored.sort_by(|left, right| {
                right
                    .0
                    .total_cmp(&left.0)
                    .then_with(|| left.1.cmp(&right.1))
            });
            layer.push(
                scored
                    .into_iter()
                    .take(GRAPH_DEGREE)
                    .map(|(_, other_index)| other_index)
                    .collect(),
            );
        }
        layers.push(layer);
    }
    layers
}

fn graph_level_for_chunk(chunk_id: &str) -> usize {
    let mut hash = 0_u64;
    for byte in chunk_id.bytes() {
        hash = hash.wrapping_mul(131).wrapping_add(byte as u64 + 1);
    }
    let mut level = 0;
    while level + 1 < GRAPH_LEVELS && hash & 0b11 == 0 {
        level += 1;
        hash >>= 2;
    }
    level
}

fn write_neighbor_layers(writer: &mut impl Write, layers: &[Vec<Vec<usize>>]) -> Result<()> {
    writer.write_all(GRAPH_MAGIC)?;
    write_u32(writer, GRAPH_DEGREE as u32)?;
    write_u32(writer, layers.len() as u32)?;
    let record_count = layers.first().map(Vec::len).unwrap_or(0);
    write_u64(writer, record_count as u64)?;
    for layer in layers {
        for row in layer {
            write_u32(writer, row.len() as u32)?;
            for index in row {
                write_u64(writer, *index as u64)?;
            }
        }
    }
    Ok(())
}

fn dot_product(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .sum()
}

fn truncate_snippet(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let snippet = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{snippet}...")
    } else {
        snippet
    }
}

fn embed_text(text: &str, dims: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dims];
    for token in text
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        let mut hash = 0_u64;
        for byte in token.bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(byte as u64 + 1);
        }
        let slot = (hash as usize) % dims;
        vector[slot] += 1.0;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn write_u32(writer: &mut impl Write, value: u32) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u64(writer: &mut impl Write, value: u64) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_f32(writer: &mut impl Write, value: f32) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_string(writer: &mut impl Write, value: &str) -> Result<()> {
    write_u32(writer, value.len() as u32)?;
    writer.write_all(value.as_bytes())?;
    Ok(())
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    let mut buf = [0_u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    let mut buf = [0_u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_f32(reader: &mut impl Read) -> Result<f32> {
    let mut buf = [0_u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

fn read_string(reader: &mut impl Read) -> Result<String> {
    let len = read_u32(reader)? as usize;
    let mut buf = vec![0_u8; len];
    reader.read_exact(&mut buf)?;
    Ok(String::from_utf8(buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use memorycore_core::init_project;
    use tempfile::tempdir;

    #[test]
    fn builds_embeddings_store_for_messages() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        conn.execute(
            "INSERT INTO sessions (id, agent, started_at, message_count) VALUES ('s1', 'codex', 1, 2)",
            [],
        )
        .expect("insert session");
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, metadata) VALUES ('s1', 'user', 'hello world', 1, '{}')",
            [],
        )
        .expect("insert msg1");
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, metadata) VALUES ('s1', 'assistant', 'world answer', 2, '{}')",
            [],
        )
        .expect("insert msg2");

        let count = build_message_embeddings_with_conn(temp.path(), &conn).expect("build");
        assert_eq!(count, 2);

        let rows = list_embeddings(temp.path()).expect("list");
        assert_eq!(rows.len(), 2);

        let store = read_store_summary(temp.path()).expect("read store");
        assert_eq!(store.len(), 2);
        assert_eq!(store[0].0, "message");
        assert_eq!(store[0].2.len(), EMBEDDING_DIMS);

        let index = read_embedding_store(temp.path()).expect("read graph index");
        assert_eq!(index.records.len(), 2);
        assert!(!index.layers.is_empty());
        assert_eq!(index.layers[0].len(), 2);
        assert_eq!(index.layers[0][0], vec![1]);
        assert_eq!(index.layers[0][1], vec![0]);
    }

    #[test]
    fn searches_embeddings_by_local_similarity() {
        let temp = tempdir().expect("temp dir");
        init_project(temp.path()).expect("init");
        let conn = connect_project_db(temp.path()).expect("db");
        conn.execute(
            "INSERT INTO sessions (id, agent, started_at, message_count) VALUES ('s1', 'codex', 1, 2)",
            [],
        )
        .expect("insert session");
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, metadata) VALUES ('s1', 'user', 'rust graph parser imports', 1, '{}')",
            [],
        )
        .expect("insert msg1");
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, metadata) VALUES ('s1', 'assistant', 'dashboard canvas rendering', 2, '{}')",
            [],
        )
        .expect("insert msg2");
        build_message_embeddings_with_conn(temp.path(), &conn).expect("build");

        let hits =
            search_embeddings_with_conn(temp.path(), &conn, "rust imports", 5).expect("search");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].score > 0.0);
        assert_eq!(hits[0].chunk_type, "message");
        assert_eq!(
            hits[0].snippet.as_deref(),
            Some("rust graph parser imports")
        );
    }

    #[test]
    fn builds_multi_layer_neighbor_graph_when_hash_levels_allow_it() {
        let records = (0..256)
            .map(|index| EmbeddingRecord {
                chunk_type: "message".to_string(),
                chunk_id: format!("message:demo:{index}"),
                metadata: json!({}),
                vector: embed_text(&format!("token {index}"), EMBEDDING_DIMS),
            })
            .collect::<Vec<_>>();

        let layers = build_neighbor_layers(&records);
        assert!(layers.len() > 1);
        assert_eq!(layers[0].len(), records.len());
        assert!(layers[0].iter().any(|row| !row.is_empty()));
        assert!(layers[1..]
            .iter()
            .any(|layer| layer.iter().any(|row| !row.is_empty())));
    }
}
