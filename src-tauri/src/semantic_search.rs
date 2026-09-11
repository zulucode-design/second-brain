//! Machine-local semantic retrieval derived from the Markdown vault.

use crate::types::SearchResult;
use crate::vault::para::ParaCategory;
use rusqlite::{params, Connection, ErrorCode};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use walkdir::WalkDir;

const EMBEDDING_PROFILE: &str = "ollama:embeddinggemma:chunks-v1";
const CHUNK_CHARACTERS: usize = 1_500;
const CHUNK_OVERLAP: usize = 200;
// Do not present a note merely because it is the least unrelated result in a small vault.
const MIN_SEMANTIC_SCORE: f32 = 0.45;

/// The external inference boundary. Production talks to Ollama; tests substitute a
/// deterministic implementation while retaining the real SQLite store.
pub trait EmbeddingBackend: Send + Sync {
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String>;
}

pub struct OllamaEmbeddingBackend {
    endpoint: String,
    bearer_token: Option<String>,
    /// Built on first use, never in `new`.
    ///
    /// A blocking client must not be built on the async runtime: reqwest's debug builds
    /// panic when they do, and release builds block a runtime worker instead. `new` runs
    /// inside `open_vault`, an async command, so building here left every `tauri dev`
    /// launch on a permanent spinner (#64). `embed` only ever runs off the runtime — on the
    /// background worker, or through `spawn_blocking` — so building where it is used keeps
    /// construction off the runtime whoever opens the index. Dropping one on the runtime is
    /// safe: its `Drop` only signals and joins its own thread.
    client: OnceLock<reqwest::blocking::Client>,
}

impl OllamaEmbeddingBackend {
    pub fn new(base_url: &str, bearer_token: Option<String>) -> Result<Self, String> {
        let base_url = base_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            return Err("No Ollama address is configured for semantic search".to_string());
        }
        Ok(Self {
            endpoint: format!("{base_url}/api/embed"),
            bearer_token: bearer_token.filter(|token| !token.trim().is_empty()),
            client: OnceLock::new(),
        })
    }

    fn client(&self) -> Result<&reqwest::blocking::Client, String> {
        if let Some(client) = self.client.get() {
            return Ok(client);
        }
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(crate::ai_health::REQUEST_CONNECT_TIMEOUT)
            .timeout(crate::ai_health::REQUEST_STALL_TIMEOUT)
            .build()
            .map_err(|error| format!("Could not start the embedding client: {error}"))?;
        // Two workers racing here each build one; the loser's is dropped, off the runtime.
        Ok(self.client.get_or_init(|| client))
    }
}

impl EmbeddingBackend for OllamaEmbeddingBackend {
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
        #[derive(serde::Serialize)]
        struct Request<'a> {
            model: &'static str,
            input: &'a [String],
            truncate: bool,
        }
        #[derive(serde::Deserialize)]
        struct Response {
            embeddings: Vec<Vec<f32>>,
        }

        let mut request = self.client()?.post(&self.endpoint).json(&Request {
            model: "embeddinggemma",
            input: inputs,
            // Chunking owns the size boundary. Silent server-side truncation would make
            // the stored vector describe less text than its matching snippet claims.
            truncate: false,
        });
        if let Some(token) = self.bearer_token.as_deref() {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .map_err(|error| format!("Could not reach the embedding backend: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            let detail = response.text().unwrap_or_default();
            return Err(if detail.trim().is_empty() {
                format!("The embedding backend returned HTTP {status}")
            } else {
                format!("The embedding backend returned HTTP {status}: {detail}")
            });
        }
        response
            .json::<Response>()
            .map(|response| response.embeddings)
            .map_err(|error| format!("The embedding backend returned invalid JSON: {error}"))
    }
}

pub struct SemanticIndex {
    database: Mutex<Connection>,
    backend: Arc<dyn EmbeddingBackend>,
    wake_worker: OnceLock<Sender<()>>,
    profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticStatus {
    pub indexed_notes: usize,
    pub queued_notes: usize,
    pub model: String,
}

impl SemanticIndex {
    pub fn open(
        vault: &Path,
        base_url: &str,
        bearer_token: Option<String>,
    ) -> Result<Self, String> {
        let database = crate::machine_local::semantic_database_path(vault)?;
        Self::open_at(
            &database,
            Arc::new(OllamaEmbeddingBackend::new(base_url, bearer_token)?),
        )
    }

    pub fn open_at(database: &Path, backend: Arc<dyn EmbeddingBackend>) -> Result<Self, String> {
        Self::open_at_with_profile(database, backend, EMBEDDING_PROFILE)
    }

    fn open_at_with_profile(
        database: &Path,
        backend: Arc<dyn EmbeddingBackend>,
        profile: &str,
    ) -> Result<Self, String> {
        if let Some(parent) = database.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let connection = match initialize_database(database) {
            Ok(connection) => connection,
            Err(error) if is_corrupt_database(&error) => {
                remove_derived_database(database)?;
                initialize_database(database).map_err(|error| error.to_string())?
            }
            Err(error) => return Err(error.to_string()),
        };
        Ok(Self {
            database: Mutex::new(connection),
            backend,
            wake_worker: OnceLock::new(),
            profile: profile.to_string(),
        })
    }
}

fn initialize_database(database: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(database)?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS notes (
                    note_key TEXT PRIMARY KEY,
                    path TEXT NOT NULL UNIQUE,
                    title TEXT NOT NULL,
                    category TEXT,
                    content_hash TEXT NOT NULL,
                    profile TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS chunks (
                    note_key TEXT NOT NULL REFERENCES notes(note_key) ON DELETE CASCADE,
                    ordinal INTEGER NOT NULL,
                    text TEXT NOT NULL,
                    embedding BLOB NOT NULL,
                    PRIMARY KEY (note_key, ordinal)
                 );
                 CREATE TABLE IF NOT EXISTS pending_notes (
                    note_key TEXT PRIMARY KEY,
                    path TEXT NOT NULL UNIQUE,
                    content_hash TEXT NOT NULL,
                    profile TEXT NOT NULL
                 );",
    )?;
    Ok(connection)
}

fn is_corrupt_database(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}

fn remove_derived_database(database: &Path) -> Result<(), String> {
    for path in [
        database.to_path_buf(),
        path_with_suffix(database, "-wal"),
        path_with_suffix(database, "-shm"),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Could not replace the corrupt semantic index at {}: {error}",
                    path.display()
                ))
            }
        }
    }
    Ok(())
}

fn path_with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}

impl SemanticIndex {
    pub fn note_changed(&self, path: &Path) -> Result<(), String> {
        let raw = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let (meta, _) = crate::vault::frontmatter::parse_note(&raw, filename);
        let path_text = path.to_string_lossy().to_string();
        let note_key = if meta.id.trim().is_empty() {
            format!("path:{path_text}")
        } else {
            format!("id:{}", meta.id)
        };
        let raw_hash = content_hash(&raw);
        let database = self.database.lock().map_err(|error| error.to_string())?;
        database
            .execute(
                "DELETE FROM pending_notes WHERE note_key = ?1 OR path = ?2",
                params![note_key, path_text],
            )
            .map_err(|error| error.to_string())?;
        database
            .execute(
                "INSERT INTO pending_notes (note_key, path, content_hash, profile)
                 VALUES (?1, ?2, ?3, ?4)",
                params![note_key, path_text, raw_hash, self.profile.as_str()],
            )
            .map_err(|error| error.to_string())?;
        if let Some(wake) = self.wake_worker.get() {
            let _ = wake.send(());
        }
        Ok(())
    }

    /// Embed one queued path. `Ok(false)` means inference is unavailable and the item was
    /// deliberately left in the durable queue; local storage failures remain real errors.
    fn embed_pending_path(&self, path: &Path) -> Result<bool, String> {
        let raw = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let (meta, body) = crate::vault::frontmatter::parse_note(&raw, filename);
        let path_text = path.to_string_lossy().to_string();
        let note_key = if meta.id.trim().is_empty() {
            format!("path:{path_text}")
        } else {
            format!("id:{}", meta.id)
        };
        let raw_hash = content_hash(&raw);
        let chunks = chunks_for(&meta.title, &body);
        let inputs: Vec<String> = chunks.iter().map(|chunk| chunk.input.clone()).collect();
        let embeddings = match self.backend.embed(&inputs) {
            Ok(embeddings) => embeddings,
            Err(error) => {
                log::info!("Semantic embedding remains queued: {error}");
                return Ok(false);
            }
        };
        if embeddings.len() != chunks.len() || embeddings.iter().any(Vec::is_empty) {
            log::warn!("Semantic embedding remains queued: backend returned an unexpected number of vectors");
            return Ok(false);
        }
        let dimensions = embeddings[0].len();
        if embeddings
            .iter()
            .any(|embedding| embedding.len() != dimensions)
        {
            log::warn!("Semantic embedding remains queued: backend returned inconsistent vector dimensions");
            return Ok(false);
        }

        let mut database = self.database.lock().map_err(|error| error.to_string())?;
        let transaction = database.transaction().map_err(|error| error.to_string())?;
        let still_current = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM pending_notes
                    WHERE note_key = ?1 AND path = ?2 AND content_hash = ?3 AND profile = ?4
                 )",
                params![note_key, path_text, raw_hash, self.profile.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| error.to_string())?;
        if !still_current {
            return Ok(true);
        }
        transaction
            .execute(
                "DELETE FROM notes WHERE note_key = ?1 OR path = ?2",
                params![note_key, path_text],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO notes (note_key, path, title, category, content_hash, profile)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    note_key,
                    path_text,
                    meta.title,
                    meta.category.map(|category| category.folder_name()),
                    raw_hash,
                    self.profile.as_str(),
                ],
            )
            .map_err(|error| error.to_string())?;
        for (ordinal, (chunk, embedding)) in chunks.iter().zip(embeddings).enumerate() {
            transaction
                .execute(
                    "INSERT INTO chunks (note_key, ordinal, text, embedding)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![note_key, ordinal as i64, chunk.snippet, encode(&embedding)],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction
            .execute(
                "DELETE FROM pending_notes WHERE note_key = ?1",
                params![note_key],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .commit()
            .map(|()| true)
            .map_err(|error| error.to_string())
    }

    pub fn note_removed(&self, path: &Path) -> Result<(), String> {
        let mut database = self.database.lock().map_err(|error| error.to_string())?;
        let transaction = database.transaction().map_err(|error| error.to_string())?;
        let path = path.to_string_lossy();
        transaction
            .execute("DELETE FROM notes WHERE path = ?1", [path.as_ref()])
            .map_err(|error| error.to_string())?;
        transaction
            .execute("DELETE FROM pending_notes WHERE path = ?1", [path.as_ref()])
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn search(
        &self,
        query: &str,
        category: Option<ParaCategory>,
        limit: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let mut query_embeddings = self.backend.embed(&[query.to_string()])?;
        let query_embedding = query_embeddings
            .pop()
            .filter(|embedding| !embedding.is_empty())
            .ok_or("The embedding backend returned no query vector")?;
        let database = self.database.lock().map_err(|error| error.to_string())?;
        let mut statement = database
            .prepare(
                "SELECT n.note_key, n.path, n.title, c.text, c.embedding
                 FROM notes n JOIN chunks c ON c.note_key = n.note_key
                 WHERE n.profile = ?1 AND (?2 IS NULL OR n.category = ?2)",
            )
            .map_err(|error| error.to_string())?;
        let category_name = category.map(|value| value.folder_name());
        let rows = statement
            .query_map(params![self.profile.as_str(), category_name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            })
            .map_err(|error| error.to_string())?;

        let mut best_by_note: HashMap<String, SearchResult> = HashMap::new();
        for row in rows {
            let (note_key, path, title, snippet, bytes) = row.map_err(|error| error.to_string())?;
            let embedding = decode(&bytes)?;
            let Some(score) = cosine_similarity(&query_embedding, &embedding) else {
                continue;
            };
            if score < MIN_SEMANTIC_SCORE {
                continue;
            }
            let candidate = SearchResult {
                path,
                title,
                snippet,
                score,
            };
            match best_by_note.get_mut(&note_key) {
                Some(current) if current.score < candidate.score => *current = candidate,
                None => {
                    best_by_note.insert(note_key, candidate);
                }
                _ => {}
            }
        }
        let mut results: Vec<SearchResult> = best_by_note.into_values().collect();
        results.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
        });
        results.truncate(limit);
        Ok(results)
    }

    pub fn rebuild_from_notes(&self, vault: &Path) -> Result<(), String> {
        let paths: Vec<std::path::PathBuf> = WalkDir::new(vault)
            .into_iter()
            .filter_entry(|entry| !crate::search::is_ignored_by_index(entry.path(), vault))
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_file()
                    && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
            })
            .map(|entry| entry.into_path())
            .collect();
        {
            let mut database = self.database.lock().map_err(|error| error.to_string())?;
            let transaction = database.transaction().map_err(|error| error.to_string())?;
            transaction
                .execute("DELETE FROM notes", [])
                .map_err(|error| error.to_string())?;
            transaction
                .execute("DELETE FROM pending_notes", [])
                .map_err(|error| error.to_string())?;
            transaction.commit().map_err(|error| error.to_string())?;
        }
        for path in paths {
            self.note_changed(&path)?;
        }
        Ok(())
    }

    pub fn reconcile_from_notes(&self, vault: &Path) -> Result<(), String> {
        let recorded: HashMap<String, (String, String)> = {
            let database = self.database.lock().map_err(|error| error.to_string())?;
            let mut statement = database
                .prepare(
                    "SELECT path, content_hash, profile FROM notes
                     UNION ALL
                     SELECT path, content_hash, profile FROM pending_notes",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| error.to_string())?;
            let mut recorded = HashMap::new();
            for row in rows {
                let (path, hash, profile) = row.map_err(|error| error.to_string())?;
                recorded.insert(path, (hash, profile));
            }
            recorded
        };

        let mut seen = std::collections::HashSet::new();
        for entry in WalkDir::new(vault)
            .into_iter()
            .filter_entry(|entry| !crate::search::is_ignored_by_index(entry.path(), vault))
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_file()
                    && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
            })
        {
            let path = entry.path();
            let path_text = path.to_string_lossy().to_string();
            let raw = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
            let current = content_hash(&raw);
            seen.insert(path_text.clone());
            if recorded.get(&path_text) != Some(&(current, self.profile.clone())) {
                self.note_changed(path)?;
            }
        }
        for path in recorded.keys().filter(|path| !seen.contains(*path)) {
            self.note_removed(Path::new(path))?;
        }
        Ok(())
    }

    pub fn status(&self) -> Result<SemanticStatus, String> {
        let database = self.database.lock().map_err(|error| error.to_string())?;
        let indexed_notes = database
            .query_row("SELECT COUNT(*) FROM notes", [], |row| {
                row.get::<_, usize>(0)
            })
            .map_err(|error| error.to_string())?;
        let queued_notes = database
            .query_row("SELECT COUNT(*) FROM pending_notes", [], |row| {
                row.get::<_, usize>(0)
            })
            .map_err(|error| error.to_string())?;
        Ok(SemanticStatus {
            indexed_notes,
            queued_notes,
            model: "embeddinggemma".to_string(),
        })
    }

    pub fn retry_pending(&self) -> Result<(), String> {
        let paths = {
            let database = self.database.lock().map_err(|error| error.to_string())?;
            let mut statement = database
                .prepare("SELECT path FROM pending_notes ORDER BY rowid")
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?
        };
        for path in paths {
            let path = std::path::PathBuf::from(path);
            if path.is_file() {
                self.note_changed(&path)?;
                if !self.embed_pending_path(&path)? {
                    break;
                }
            } else {
                self.note_removed(&path)?;
            }
        }
        Ok(())
    }

    pub fn start_background(self: &Arc<Self>) {
        self.start_background_with_interval(Duration::from_secs(20));
    }

    fn start_background_with_interval(self: &Arc<Self>, interval: Duration) {
        let (sender, receiver) = mpsc::channel();
        if self.wake_worker.set(sender).is_err() {
            return;
        }
        let index = Arc::downgrade(self);
        std::thread::spawn(move || {
            while let Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) =
                receiver.recv_timeout(interval)
            {
                let Some(index) = index.upgrade() else {
                    break;
                };
                if let Err(error) = index.retry_pending() {
                    log::warn!("Could not process the semantic embedding queue: {error}");
                }
            }
        });
    }
}

struct NoteChunk {
    input: String,
    snippet: String,
}

fn chunks_for(title: &str, body: &str) -> Vec<NoteChunk> {
    let characters: Vec<char> = body.chars().collect();
    if characters.is_empty() {
        return vec![NoteChunk {
            input: title.to_string(),
            snippet: String::new(),
        }];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < characters.len() {
        let end = (start + CHUNK_CHARACTERS).min(characters.len());
        let snippet: String = characters[start..end].iter().collect();
        chunks.push(NoteChunk {
            input: format!("{title}\n\n{snippet}"),
            snippet,
        });
        if end == characters.len() {
            break;
        }
        start = end - CHUNK_OVERLAP;
    }
    chunks
}

fn content_hash(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn encode(embedding: &[f32]) -> Vec<u8> {
    embedding
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if !bytes.len().is_multiple_of(std::mem::size_of::<f32>()) {
        return Err("Stored embedding is corrupt".to_string());
    }
    Ok((0..bytes.len())
        .step_by(std::mem::size_of::<f32>())
        .map(|start| {
            f32::from_le_bytes([
                bytes[start],
                bytes[start + 1],
                bytes[start + 2],
                bytes[start + 3],
            ])
        })
        .collect())
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.is_empty() || left.len() != right.len() {
        return None;
    }
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let left_norm: f32 = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm: f32 = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    (left_norm > 0.0 && right_norm > 0.0).then_some(dot / (left_norm * right_norm))
}

#[cfg(test)]
mod tests {
    #[test]
    fn opening_and_replacing_the_index_inside_async_code_does_not_panic() {
        // `open_vault` is an async command, and it builds the index. A blocking HTTP client
        // must never be built on the async runtime: reqwest's debug builds panic there, which
        // left every `tauri dev` launch on a permanent spinner (#64). Dropping one there — a
        // vault switch replacing the old index — has to stay safe too.
        let vault = std::env::temp_dir().join(format!("semantic-async-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&vault).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let first = SemanticIndex::open(&vault, "http://127.0.0.1:9", None).unwrap();
            let replacement = SemanticIndex::open(&vault, "http://127.0.0.1:9", None).unwrap();
            drop(first);
            drop(replacement);
        });
    }

    use super::{EmbeddingBackend, OllamaEmbeddingBackend, SemanticIndex};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    struct MeaningBackend;

    struct UnavailableBackend;

    struct RecoveringBackend {
        available: Arc<AtomicBool>,
    }

    struct CountingBackend {
        calls: Arc<AtomicUsize>,
    }

    struct FixedSizeBackend {
        dimensions: usize,
    }

    struct ThresholdBackend;

    impl EmbeddingBackend for MeaningBackend {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(inputs
                .iter()
                .map(|input| {
                    if input.contains("coffee") || input.contains("mornings") {
                        vec![1.0, 0.0]
                    } else {
                        vec![0.0, 1.0]
                    }
                })
                .collect())
        }
    }

    impl EmbeddingBackend for UnavailableBackend {
        fn embed(&self, _inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Err("desktop is asleep".to_string())
        }
    }

    impl EmbeddingBackend for RecoveringBackend {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            if !self.available.load(Ordering::SeqCst) {
                return Err("desktop is asleep".to_string());
            }
            MeaningBackend.embed(inputs)
        }
    }

    impl EmbeddingBackend for CountingBackend {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            MeaningBackend.embed(inputs)
        }
    }

    impl EmbeddingBackend for FixedSizeBackend {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(inputs
                .iter()
                .map(|_| {
                    let mut embedding = vec![0.0; self.dimensions];
                    embedding[0] = 1.0;
                    embedding
                })
                .collect())
        }
    }

    impl EmbeddingBackend for ThresholdBackend {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(inputs
                .iter()
                .map(|input| {
                    if input.contains("target") {
                        vec![1.0, 0.0]
                    } else if input.contains("moderate") {
                        vec![0.5, 0.8660254]
                    } else if input.contains("weak") {
                        vec![0.4, 0.9165151]
                    } else {
                        vec![0.0, 1.0]
                    }
                })
                .collect())
        }
    }

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("semantic-search-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_note(path: &Path, id: &str, title: &str, category: &str, body: &str) {
        let raw = format!(
            "---\nid: {id}\ntitle: {title}\ntags: []\npinned: false\ncreated: 2026-09-10T00:00:00Z\nmodified: 2026-09-10T00:00:00Z\ncategory: {category}\n---\n\n{body}\n"
        );
        std::fs::write(path, raw).unwrap();
    }

    // Windows cannot remove a directory while SQLite still has an open handle.
    // Keep cleanup explicit in tests so the suite behaves consistently across platforms.
    fn cleanup(root: std::path::PathBuf) {
        for attempt in 0..20 {
            match std::fs::remove_dir_all(&root) {
                Ok(()) => return,
                Err(error)
                    if attempt < 19 && error.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("could not clean up semantic test directory: {error}"),
            }
        }
    }

    #[test]
    fn a_persisted_embedding_finds_meaning_that_keyword_search_would_miss() {
        let root = scratch("meaning");
        let note = root.join("Routine.md");
        let database = root.join("semantic.sqlite3");
        write_note(
            &note,
            "routine-id",
            "Daily routine",
            "Areas",
            "I prepare coffee, stretch, and review my priorities before work.",
        );

        let index = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();
        index.note_changed(&note).unwrap();
        index.retry_pending().unwrap();
        drop(index);

        let reopened = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();
        let results = reopened
            .search("how I structure my mornings", None, 10)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Daily routine");
        assert_eq!(results[0].path, note.to_string_lossy());

        drop(reopened);
        cleanup(root);
    }

    #[test]
    fn semantic_search_hides_notes_below_the_confidence_cutoff() {
        let root = scratch("confidence-cutoff");
        let routine = root.join("Routine.md");
        let unrelated = root.join("Unrelated.md");
        write_note(
            &routine,
            "routine-cutoff-id",
            "Daily routine",
            "Areas",
            "I prepare coffee before work.",
        );
        write_note(
            &unrelated,
            "unrelated-cutoff-id",
            "Unrelated note",
            "Areas",
            "Quantum mechanics and particle wave functions.",
        );
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        index.note_changed(&routine).unwrap();
        index.note_changed(&unrelated).unwrap();
        index.retry_pending().unwrap();

        let results = index
            .search("how I structure my mornings", None, 10)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Daily routine");
        drop(index);
        cleanup(root);
    }

    #[test]
    fn semantic_search_keeps_relevant_matches_on_embeddinggemmas_score_scale() {
        let root = scratch("model-score-scale");
        let moderate = root.join("Moderate.md");
        let weak = root.join("Weak.md");
        write_note(
            &moderate,
            "moderate-id",
            "Moderate",
            "Areas",
            "moderate concept",
        );
        write_note(&weak, "weak-id", "Weak", "Areas", "weak concept");

        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(ThresholdBackend))
                .unwrap();
        index.note_changed(&moderate).unwrap();
        index.note_changed(&weak).unwrap();
        index.retry_pending().unwrap();

        let results = index.search("target", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Moderate");
        drop(index);
        cleanup(root);
    }

    #[test]
    fn semantic_search_keeps_title_only_notes_searchable() {
        let root = scratch("title-only");
        let note = root.join("Moderate.md");
        write_note(&note, "title-only-id", "moderate", "Areas", "");

        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(ThresholdBackend))
                .unwrap();
        index.note_changed(&note).unwrap();
        index.retry_pending().unwrap();

        let results = index.search("target", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "moderate");
        assert!(results[0].snippet.is_empty());
        drop(index);
        cleanup(root);
    }

    #[test]
    fn an_unavailable_backend_queues_the_note_without_failing_its_change() {
        let root = scratch("offline-queue");
        let note = root.join("Offline.md");
        write_note(
            &note,
            "offline-id",
            "Offline thought",
            "Projects",
            "A thought saved while the desktop is asleep.",
        );
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(UnavailableBackend))
                .unwrap();

        assert!(index.note_changed(&note).is_ok());
        assert_eq!(index.status().unwrap().queued_notes, 1);

        drop(index);
        cleanup(root);
    }

    #[test]
    fn queued_work_survives_restart_and_is_embedded_after_recovery() {
        let root = scratch("recovery");
        let note = root.join("Routine.md");
        let database = root.join("semantic.sqlite3");
        write_note(
            &note,
            "recovery-id",
            "Recovered routine",
            "Areas",
            "coffee and stretching happen before I begin work.",
        );
        let available = Arc::new(AtomicBool::new(false));
        let offline = SemanticIndex::open_at(
            &database,
            Arc::new(RecoveringBackend {
                available: available.clone(),
            }),
        )
        .unwrap();
        offline.note_changed(&note).unwrap();
        drop(offline);

        available.store(true, Ordering::SeqCst);
        let recovered =
            SemanticIndex::open_at(&database, Arc::new(RecoveringBackend { available })).unwrap();
        recovered.retry_pending().unwrap();

        assert_eq!(recovered.status().unwrap().queued_notes, 0);
        assert_eq!(
            recovered
                .search("how I structure my mornings", None, 10)
                .unwrap()[0]
                .title,
            "Recovered routine"
        );

        drop(recovered);
        cleanup(root);
    }

    #[test]
    fn the_background_worker_processes_queued_notes_when_the_backend_returns() {
        let root = scratch("background-recovery");
        let note = root.join("Routine.md");
        write_note(
            &note,
            "background-id",
            "Background routine",
            "Areas",
            "coffee before work",
        );
        let available = Arc::new(AtomicBool::new(false));
        let index = Arc::new(
            SemanticIndex::open_at(
                &root.join("semantic.sqlite3"),
                Arc::new(RecoveringBackend {
                    available: available.clone(),
                }),
            )
            .unwrap(),
        );
        index.start_background_with_interval(std::time::Duration::from_millis(10));
        index.note_changed(&note).unwrap();
        available.store(true, Ordering::SeqCst);

        // Windows can be heavily loaded while the full suite starts many filesystem and
        // SQLite tests in parallel; allow the worker a bounded but non-flaky window.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while index.status().unwrap().queued_notes != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(index.status().unwrap().queued_notes, 0);
        assert_eq!(index.status().unwrap().indexed_notes, 1);
        drop(index);
        cleanup(root);
    }

    #[test]
    fn rebuilding_uses_markdown_as_truth_and_discards_stale_entries() {
        let root = scratch("rebuild");
        let projects = root.join("Projects");
        let areas = root.join("Areas");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&areas).unwrap();
        let stale = projects.join("Stale.md");
        let current = areas.join("Current.md");
        write_note(&stale, "stale-id", "Stale", "Projects", "old taxes");
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        index.note_changed(&stale).unwrap();
        index.retry_pending().unwrap();
        std::fs::remove_file(&stale).unwrap();
        write_note(
            &current,
            "current-id",
            "Current routine",
            "Areas",
            "coffee before work",
        );
        let markdown_before_rebuild = std::fs::read(&current).unwrap();

        index.rebuild_from_notes(&root).unwrap();
        index.retry_pending().unwrap();

        assert_eq!(std::fs::read(&current).unwrap(), markdown_before_rebuild);
        let status = index.status().unwrap();
        assert_eq!(status.indexed_notes, 1);
        assert_eq!(status.queued_notes, 0);
        let results = index
            .search("how I structure my mornings", None, 10)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Current routine");

        drop(index);
        cleanup(root);
    }

    #[test]
    fn semantic_results_can_be_scoped_to_one_para_category() {
        let root = scratch("category");
        let projects = root.join("Projects");
        let areas = root.join("Areas");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&areas).unwrap();
        let project = projects.join("Launch.md");
        let area = areas.join("Routine.md");
        write_note(&project, "project-id", "Launch", "Projects", "coffee plan");
        write_note(&area, "area-id", "Routine", "Areas", "coffee ritual");
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        index.note_changed(&project).unwrap();
        index.note_changed(&area).unwrap();
        index.retry_pending().unwrap();

        let results = index
            .search(
                "how I structure my mornings",
                Some(crate::vault::para::ParaCategory::Projects),
                10,
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Launch");
        drop(index);
        cleanup(root);
    }

    #[test]
    fn editing_replaces_the_old_embedding_instead_of_duplicating_the_note() {
        let root = scratch("edit");
        let note = root.join("Changing.md");
        write_note(&note, "changing-id", "Changing", "Areas", "quarterly taxes");
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        index.note_changed(&note).unwrap();
        index.retry_pending().unwrap();
        write_note(
            &note,
            "changing-id",
            "Changing",
            "Areas",
            "coffee before work",
        );
        index.note_changed(&note).unwrap();
        index.retry_pending().unwrap();

        let results = index
            .search("how I structure my mornings", None, 10)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].score, 1.0);
        assert_eq!(index.status().unwrap().indexed_notes, 1);
        drop(index);
        cleanup(root);
    }

    #[test]
    fn deleting_a_note_removes_its_embeddings_and_pending_work() {
        let root = scratch("delete");
        let note = root.join("Gone.md");
        write_note(&note, "gone-id", "Gone", "Areas", "coffee before work");
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        index.note_changed(&note).unwrap();
        index.retry_pending().unwrap();
        index.note_removed(&note).unwrap();

        assert_eq!(
            index.status().unwrap(),
            super::SemanticStatus {
                indexed_notes: 0,
                queued_notes: 0,
                model: "embeddinggemma".to_string(),
            }
        );
        drop(index);
        cleanup(root);
    }

    #[test]
    fn a_long_note_can_match_content_beyond_its_first_embedding_chunk() {
        let root = scratch("chunks");
        let note = root.join("Long.md");
        let body = format!("{} coffee before work", "x".repeat(2_000));
        write_note(&note, "long-id", "Long", "Areas", &body);
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        index.note_changed(&note).unwrap();
        index.retry_pending().unwrap();

        let results = index
            .search("how I structure my mornings", None, 10)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].score, 1.0);
        assert!(results[0].snippet.contains("coffee"));
        drop(index);
        cleanup(root);
    }

    #[test]
    fn the_ollama_adapter_batches_inputs_through_the_embedding_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_string();
            let body = r#"{"model":"embeddinggemma","embeddings":[[1.0,0.0],[0.0,1.0]]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            request
        });
        let backend = OllamaEmbeddingBackend::new(&format!("http://{address}"), None).unwrap();

        let embeddings = backend
            .embed(&["first".to_string(), "second".to_string()])
            .unwrap();
        let request = server.join().unwrap();

        assert_eq!(embeddings, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert!(request.starts_with("POST /api/embed "));
        assert!(request.contains("\"model\":\"embeddinggemma\""));
        assert!(request.contains("\"input\":[\"first\",\"second\"]"));
    }

    #[test]
    fn reconciliation_queues_only_notes_that_changed_while_the_app_was_closed() {
        let root = scratch("reconcile");
        let areas = root.join("Areas");
        std::fs::create_dir_all(&areas).unwrap();
        let unchanged = areas.join("Unchanged.md");
        let changed = areas.join("Changed.md");
        write_note(&unchanged, "unchanged-id", "Unchanged", "Areas", "taxes");
        write_note(&changed, "changed-id", "Changed", "Areas", "taxes");
        let calls = Arc::new(AtomicUsize::new(0));
        let index = SemanticIndex::open_at(
            &root.join("semantic.sqlite3"),
            Arc::new(CountingBackend {
                calls: calls.clone(),
            }),
        )
        .unwrap();
        index.note_changed(&unchanged).unwrap();
        index.note_changed(&changed).unwrap();
        index.retry_pending().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        write_note(
            &changed,
            "changed-id",
            "Changed",
            "Areas",
            "coffee before work",
        );
        index.reconcile_from_notes(&root).unwrap();

        assert_eq!(index.status().unwrap().queued_notes, 1);
        index.retry_pending().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(index.status().unwrap().indexed_notes, 2);
        drop(index);
        cleanup(root);
    }

    #[test]
    fn changing_the_embedding_profile_invalidates_incompatible_vectors() {
        let root = scratch("profile-migration");
        let areas = root.join("Areas");
        std::fs::create_dir_all(&areas).unwrap();
        let note = areas.join("Routine.md");
        let database = root.join("semantic.sqlite3");
        write_note(
            &note,
            "profile-id",
            "Routine",
            "Areas",
            "coffee before work",
        );
        let old = SemanticIndex::open_at_with_profile(
            &database,
            Arc::new(MeaningBackend),
            "ollama:retired-model:chunks-v0",
        )
        .unwrap();
        old.note_changed(&note).unwrap();
        old.retry_pending().unwrap();
        drop(old);

        let current = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();
        current.reconcile_from_notes(&root).unwrap();

        assert_eq!(current.status().unwrap().queued_notes, 1);
        assert!(current
            .search("how I structure my mornings", None, 10)
            .unwrap()
            .is_empty());
        current.retry_pending().unwrap();
        assert_eq!(current.status().unwrap().queued_notes, 0);
        assert_eq!(
            current
                .search("how I structure my mornings", None, 10)
                .unwrap()
                .len(),
            1
        );
        drop(current);
        cleanup(root);
    }

    #[test]
    fn a_corrupt_derived_database_is_recreated_from_markdown() {
        let root = scratch("corrupt-database");
        let areas = root.join("Areas");
        std::fs::create_dir_all(&areas).unwrap();
        let note = areas.join("Routine.md");
        let database = root.join("semantic.sqlite3");
        write_note(
            &note,
            "corrupt-id",
            "Routine",
            "Areas",
            "coffee before work",
        );
        std::fs::write(&database, b"this is not a SQLite database").unwrap();

        let index = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();
        index.rebuild_from_notes(&root).unwrap();
        index.retry_pending().unwrap();

        assert_eq!(index.status().unwrap().indexed_notes, 1);
        assert_eq!(
            index
                .search("how I structure my mornings", None, 10)
                .unwrap()[0]
                .title,
            "Routine"
        );
        drop(index);
        cleanup(root);
    }

    #[test]
    #[ignore = "representative-vault latency measurement; run explicitly"]
    fn brute_force_search_latency_for_ten_thousand_notes() {
        const NOTES: usize = 10_000;
        const DIMENSIONS: usize = 768;
        let root = scratch("latency");
        let index = SemanticIndex::open_at(
            &root.join("semantic.sqlite3"),
            Arc::new(FixedSizeBackend {
                dimensions: DIMENSIONS,
            }),
        )
        .unwrap();
        let embedding = super::encode(&{
            let mut value = vec![0.0; DIMENSIONS];
            value[0] = 1.0;
            value
        });
        {
            let mut database = index.database.lock().unwrap();
            let transaction = database.transaction().unwrap();
            for ordinal in 0..NOTES {
                let key = format!("id:benchmark-{ordinal}");
                transaction
                    .execute(
                        "INSERT INTO notes (note_key, path, title, category, content_hash, profile)
                         VALUES (?1, ?2, ?3, 'Resources', 'benchmark', ?4)",
                        rusqlite::params![
                            key,
                            format!("/representative/Resources/Note {ordinal}.md"),
                            format!("Note {ordinal}"),
                            super::EMBEDDING_PROFILE,
                        ],
                    )
                    .unwrap();
                transaction
                    .execute(
                        "INSERT INTO chunks (note_key, ordinal, text, embedding)
                         VALUES (?1, 0, 'representative note', ?2)",
                        rusqlite::params![key, embedding],
                    )
                    .unwrap();
            }
            transaction.commit().unwrap();
        }

        let started = std::time::Instant::now();
        let results = index.search("representative query", None, 20).unwrap();
        let elapsed = started.elapsed();

        assert_eq!(results.len(), 20);
        println!(
            "semantic brute-force benchmark: {NOTES} notes, {DIMENSIONS} dimensions, {} ms",
            elapsed.as_millis()
        );
        drop(index);
        cleanup(root);
    }
}
