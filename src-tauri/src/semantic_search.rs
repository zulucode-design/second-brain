//! Machine-local semantic retrieval derived from the Markdown vault.

use crate::types::{NoteMeta, SearchResult};
use crate::vault::para::ParaCategory;
use rusqlite::{params, Connection, ErrorCode};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

const EMBEDDING_PROFILE: &str = "ollama:embeddinggemma:chunks-v2";
/// The layout of this machine-local projection, recorded in the file's `user_version`.
///
/// Bump this whenever the tables, columns, or constraints below change. `CREATE TABLE IF NOT
/// EXISTS` alone silently adopts whatever tables it finds, so an older file would keep its
/// old columns and only fail later, at a query, as if the data were wrong. The version makes
/// that mismatch explicit at open. It is a separate contract from `EMBEDDING_PROFILE`: this
/// one describes the shape of the store, that one describes what the vectors in it mean.
const SEMANTIC_SCHEMA_VERSION: i64 = 1;
/// `notes`, `chunks`, and `pending_notes` — a file carrying only some of them is unusable.
const SEMANTIC_TABLES: [&str; 3] = ["notes", "chunks", "pending_notes"];
const CHUNK_CHARACTERS: usize = 1_500;
const CHUNK_OVERLAP: usize = 200;
// Do not present a note merely because it is the least unrelated result in a small vault.
// Calibrated for the chunks-v2 prompts against live embeddinggemma (#130): 24 paraphrased
// queries over 24 notes scored their right note at 0.258 or above, and 8 queries with no
// matching note topped out at 0.192.
const MIN_SEMANTIC_SCORE: f32 = 0.22;
/// EmbeddingGemma's retrieval prompts. Without them a paraphrase and an unrelated note
/// scored within 0.02 of each other, so no cutoff could keep one and hide the other (#130).
/// Changing either prompt changes what stored vectors mean, so it requires a new profile.
const QUERY_PROMPT: &str = "task: search result | query: ";
/// How long a search waits for the vector cache to finish loading before it reports the index
/// as still loading. Waiting forever would leave the search spinner up if loading stalls.
const CACHE_READY_DEADLINE: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(5)
};

fn document_input(title: &str, text: &str) -> String {
    let title = if title.trim().is_empty() {
        "none"
    } else {
        title
    };
    format!("title: {title} | text: {text}")
}

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

/// One indexed note's vectors, decoded, so a search ranks without reading any blob (#159).
/// Reading every chunk row per search cost about 46 MB of I/O on the 10,000-note fixture, and
/// on Windows a scan that met startup I/O took over a second.
struct CachedNote {
    path: String,
    /// Breaks score ties, so ranking needs no lookup however many notes tie.
    title: String,
    profile: String,
    category: Option<String>,
    /// `(ordinal, vector)`, in ordinal order.
    chunks: Vec<(i64, Vec<f32>)>,
}

/// ponytail: every vector stays in memory, so memory grows linearly with chunk count: about
/// 3 KB per 768-dimension chunk plus metadata, or roughly 31 MB for the 10,000-note fixture.
/// Storing f16 or quantized vectors is the upgrade, and it needs its own ranking validation.
enum CacheState {
    /// Not loaded yet; the first search loads it inline.
    NotLoaded,
    /// A background load is running; searches wait for it, up to `CACHE_READY_DEADLINE`.
    Loading,
    Ready(HashMap<String, CachedNote>),
    /// Loading failed; searches use the SQL scan for the rest of this session.
    Failed,
}

/// Kept equal to the committed `notes` and `chunks` tables. Every change is published while
/// the database mutex is still held, after its commit, and always taken in that order:
/// database, then cache.
struct VectorCache {
    state: Mutex<CacheState>,
    ready: Condvar,
}

pub struct SemanticIndex {
    database: Mutex<Connection>,
    cache: VectorCache,
    /// Set when a newer index on the same database replaces this one. A retired index writes
    /// nothing, so a commit cannot land behind the new index's cache snapshot; the new index
    /// reconciles from the vault and picks up anything this one dropped.
    retired: AtomicBool,
    /// How many searches ranked with the SQL scan, so tests can tell the two paths apart.
    #[cfg(test)]
    sql_scans: AtomicUsize,
    /// Reports where a search is, so tests can order a race without sleeping.
    #[cfg(test)]
    search_events: Mutex<Option<Sender<&'static str>>>,
    backend: Arc<dyn EmbeddingBackend>,
    wake_worker: OnceLock<Sender<()>>,
    embedding_outage_reported: AtomicBool,
    last_reported_unreadable: AtomicUsize,
    profile: String,
}

struct PreparedNote {
    path: String,
    key: String,
    hash: String,
    meta: NoteMeta,
    body: String,
}

impl PreparedNote {
    fn from_text(path: &Path, raw: &str) -> Self {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let (meta, body) = crate::vault::frontmatter::parse_note(raw, filename);
        let path = path.to_string_lossy().to_string();
        let key = if meta.id.trim().is_empty() {
            format!("path:{path}")
        } else {
            format!("id:{}", meta.id)
        };
        Self {
            path,
            key,
            hash: content_hash(raw),
            meta,
            body,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticStatus {
    pub indexed_notes: usize,
    pub queued_notes: usize,
    pub model: String,
}

enum RetryOutcome {
    QueueProcessed,
    BackendUnavailable,
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
        let connection = open_derived_database(database)?;
        Ok(Self {
            database: Mutex::new(connection),
            cache: VectorCache {
                state: Mutex::new(CacheState::NotLoaded),
                ready: Condvar::new(),
            },
            retired: AtomicBool::new(false),
            #[cfg(test)]
            sql_scans: AtomicUsize::new(0),
            #[cfg(test)]
            search_events: Mutex::new(None),
            backend,
            wake_worker: OnceLock::new(),
            embedding_outage_reported: AtomicBool::new(false),
            last_reported_unreadable: AtomicUsize::new(0),
            profile: profile.to_string(),
        })
    }
}

/// What opening the derived database found.
enum Prepared {
    /// Usable as-is: the schema version and tables are the ones this build wrote.
    Ready(Connection),
    /// Present but not interpretable by this build, with the reason to log.
    Incompatible(String),
}

/// Open the semantic projection, replacing it whenever its contents cannot be trusted.
///
/// Every branch here is safe because this database holds no source of truth. The Markdown
/// vault does, and `reconcile_from_notes` — which every caller runs after opening — requeues
/// whatever the replacement is missing. Reusing a file this build cannot interpret is the
/// only unsafe option, because the damage would surface later as wrong answers.
fn open_derived_database(database: &Path) -> Result<Connection, String> {
    let reason = match prepare_database(database) {
        Ok(Prepared::Ready(connection)) => return Ok(connection),
        Ok(Prepared::Incompatible(reason)) => reason,
        Err(error) if is_corrupt_database(&error) => {
            "the file is not a readable SQLite database".to_string()
        }
        Err(error) => return Err(error.to_string()),
    };

    log::info!("Recreating the semantic index from Markdown because {reason}.");
    remove_derived_database(database)?;
    match prepare_database(database).map_err(|error| error.to_string())? {
        Prepared::Ready(connection) => Ok(connection),
        // The replacement is a file this build just created, so this cannot normally happen.
        Prepared::Incompatible(reason) => Err(format!(
            "The semantic index could not be recreated: {reason}"
        )),
    }
}

fn prepare_database(database: &Path) -> rusqlite::Result<Prepared> {
    let connection = Connection::open(database)?;
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;
    // Reads the file header, so a file that is not a database is rejected here.
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let tables = derived_table_count(&connection)?;

    if tables == 0 {
        // A new file, or one just replaced: stamp the version together with the tables so the
        // two can never disagree.
        create_schema(&connection)?;
        return Ok(Prepared::Ready(connection));
    }
    if tables != SEMANTIC_TABLES.len() {
        return Ok(Prepared::Incompatible(format!(
            "it has {tables} of the {} expected tables",
            SEMANTIC_TABLES.len()
        )));
    }
    if version != SEMANTIC_SCHEMA_VERSION {
        return Ok(Prepared::Incompatible(if version == 0 {
            "it predates semantic schema versioning".to_string()
        } else {
            format!(
                "it declares semantic schema version {version} and this build writes \
                 {SEMANTIC_SCHEMA_VERSION}"
            )
        }));
    }
    Ok(Prepared::Ready(connection))
}

/// How many of the expected tables this file actually has.
///
/// Driven by `SEMANTIC_TABLES` rather than a literal name list, so adding a table to the
/// schema cannot leave the completeness check silently looking for the old set.
fn derived_table_count(connection: &Connection) -> rusqlite::Result<usize> {
    let mut statement = connection
        .prepare("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    let mut present = 0;
    for table in SEMANTIC_TABLES {
        present += statement.query_row([table], |row| row.get::<_, usize>(0))?;
    }
    Ok(present)
}

fn create_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS notes (
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
    connection.pragma_update(None, "user_version", SEMANTIC_SCHEMA_VERSION)?;
    Ok(())
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
        self.queue_text(path, &raw)
    }

    /// Queue a note from text already read, and wake the worker to embed it.
    fn queue_text(&self, path: &Path, raw: &str) -> Result<(), String> {
        self.refresh_pending_text(&PreparedNote::from_text(path, raw))?;
        if let Some(wake) = self.wake_worker.get() {
            let _ = wake.send(());
        }
        Ok(())
    }

    /// Refresh one durable pending row, without waking the background worker.
    fn refresh_pending_text(&self, note: &PreparedNote) -> Result<(), String> {
        let database = self.database.lock().map_err(|error| error.to_string())?;
        if self.retired.load(Ordering::SeqCst) {
            return Ok(());
        }
        database
            .execute(
                "DELETE FROM pending_notes WHERE note_key = ?1 OR path = ?2",
                params![note.key, note.path],
            )
            .map_err(|error| error.to_string())?;
        // A change event does not mean the content changed: reindexing and file watchers
        // report notes whose embedding is already current. Queueing those re-embedded the
        // vault and left "waiting" counts above the note count (#129).
        let already_indexed = database
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM notes
                    WHERE note_key = ?1 AND path = ?2 AND content_hash = ?3 AND profile = ?4
                 )",
                params![note.key, note.path, note.hash, self.profile.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| error.to_string())?;
        if already_indexed {
            return Ok(());
        }
        database
            .execute(
                "INSERT INTO pending_notes (note_key, path, content_hash, profile)
                 VALUES (?1, ?2, ?3, ?4)",
                params![note.key, note.path, note.hash, self.profile.as_str()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Embed one queued note from its text. `Ok(false)` means inference is unavailable and
    /// the item was deliberately left in the durable queue; local storage failures remain real
    /// errors.
    fn embed_pending_text(&self, note: &PreparedNote) -> Result<bool, String> {
        let chunks = chunks_for(&note.meta.title, &note.body);
        let inputs: Vec<String> = chunks.iter().map(|chunk| chunk.input.clone()).collect();
        let embeddings = match self.backend.embed(&inputs) {
            Ok(embeddings) => embeddings,
            Err(error) => {
                if !self.embedding_outage_reported.swap(true, Ordering::SeqCst) {
                    log::info!("Semantic embedding remains queued: {error}");
                }
                return Ok(false);
            }
        };
        if embeddings.len() != chunks.len() || embeddings.iter().any(Vec::is_empty) {
            if !self.embedding_outage_reported.swap(true, Ordering::SeqCst) {
                log::warn!("Semantic embedding remains queued: backend returned an unexpected number of vectors");
            }
            return Ok(false);
        }
        let dimensions = embeddings[0].len();
        if embeddings
            .iter()
            .any(|embedding| embedding.len() != dimensions)
        {
            if !self.embedding_outage_reported.swap(true, Ordering::SeqCst) {
                log::warn!("Semantic embedding remains queued: backend returned inconsistent vector dimensions");
            }
            return Ok(false);
        }
        self.embedding_outage_reported
            .store(false, Ordering::SeqCst);

        let mut database = self.database.lock().map_err(|error| error.to_string())?;
        if self.retired.load(Ordering::SeqCst) {
            return Ok(true);
        }
        let transaction = database.transaction().map_err(|error| error.to_string())?;
        let still_current = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM pending_notes
                    WHERE note_key = ?1 AND path = ?2 AND content_hash = ?3 AND profile = ?4
                 )",
                params![note.key, note.path, note.hash, self.profile.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| error.to_string())?;
        if !still_current {
            return Ok(true);
        }
        transaction
            .execute(
                "DELETE FROM notes WHERE note_key = ?1 OR path = ?2",
                params![note.key, note.path],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO notes (note_key, path, title, category, content_hash, profile)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    note.key,
                    note.path,
                    note.meta.title,
                    note.meta.category.map(|category| category.folder_name()),
                    note.hash,
                    self.profile.as_str(),
                ],
            )
            .map_err(|error| error.to_string())?;
        for (ordinal, (chunk, embedding)) in chunks.iter().zip(&embeddings).enumerate() {
            transaction
                .execute(
                    "INSERT INTO chunks (note_key, ordinal, text, embedding)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![note.key, ordinal as i64, chunk.snippet, encode(embedding)],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction
            .execute(
                "DELETE FROM pending_notes WHERE note_key = ?1",
                params![note.key],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        // The DELETE above displaced the note under this key and any note at this path.
        self.publish(|notes| {
            notes.retain(|key, cached| key != &note.key && cached.path != note.path);
            notes.insert(
                note.key.clone(),
                CachedNote {
                    path: note.path.clone(),
                    title: note.meta.title.clone(),
                    profile: self.profile.clone(),
                    category: note
                        .meta
                        .category
                        .map(|category| category.folder_name().to_string()),
                    chunks: (0_i64..).zip(embeddings).collect(),
                },
            );
        });
        drop(database);
        Ok(true)
    }

    pub fn note_removed(&self, path: &Path) -> Result<(), String> {
        let mut database = self.database.lock().map_err(|error| error.to_string())?;
        if self.retired.load(Ordering::SeqCst) {
            return Ok(());
        }
        let transaction = database.transaction().map_err(|error| error.to_string())?;
        let path = path.to_string_lossy();
        transaction
            .execute("DELETE FROM notes WHERE path = ?1", [path.as_ref()])
            .map_err(|error| error.to_string())?;
        transaction
            .execute("DELETE FROM pending_notes WHERE path = ?1", [path.as_ref()])
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        self.publish(|notes| notes.retain(|_, cached| cached.path != path.as_ref()));
        drop(database);
        Ok(())
    }

    pub fn search(
        &self,
        query: &str,
        category: Option<ParaCategory>,
        limit: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let scored = self.score(format!("{QUERY_PROMPT}{query}"), category)?;
        let results = best_per_note(&scored.database, &scored.chunks, limit)?;
        crate::perf_probe::record(serde_json::json!({
            "kind": "semantic-backend",
            "embedMs": scored.embed_ms,
            "exactScanMs": scored.scan_started.elapsed().as_secs_f64() * 1000.0,
            "lockWaitMs": scored.lock_wait_ms,
            "results": results.len(),
        }));
        Ok(results)
    }

    /// Every chunk at or above the cutoff, best first, with text for as many as fit in
    /// `max_characters`. `cost` prices a chunk as the prompt will spell it out: its title,
    /// its text, and whether it is the first read from its note. Ask reads these; a note none
    /// of whose chunks fit is reported unread.
    pub fn retrieve(
        &self,
        query: &str,
        category: Option<ParaCategory>,
        max_characters: usize,
        cost: impl Fn(&str, &str, bool) -> usize,
    ) -> Result<Retrieval, String> {
        let mut scored = self.score(format!("{QUERY_PROMPT}{query}"), category)?;
        scored.chunks.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.ordinal.cmp(&right.ordinal))
        });
        let mut retrieval = Retrieval::default();
        let mut used = 0;
        let mut read: HashMap<&str, usize> = HashMap::new();
        for chunk in &scored.chunks {
            let text = chunk_text(&scored.database, chunk)?;
            let length = cost(&chunk.title, &text, !read.contains_key(chunk.key.as_str()));
            // Chunks are at most CHUNK_CHARACTERS long, so stopping at the first one that does
            // not fit costs little and keeps what was read a clean best-first slice.
            if used + length > max_characters {
                break;
            }
            used += length;
            *read.entry(chunk.key.as_str()).or_default() += 1;
            retrieval.chunks.push(RetrievedChunk {
                path: chunk.path.clone(),
                note_id: note_id(&chunk.key),
                title: chunk.title.clone(),
                ordinal: chunk.ordinal,
                text,
            });
        }
        let mut related: HashMap<&str, usize> = HashMap::new();
        for chunk in &scored.chunks {
            *related.entry(chunk.key.as_str()).or_default() += 1;
        }
        retrieval.partly_read = read
            .iter()
            .filter(|(key, count)| related.get(*key).is_some_and(|total| total > *count))
            .count();
        let mut listed = HashSet::new();
        for chunk in &scored.chunks {
            if listed.insert(chunk.key.as_str()) && !read.contains_key(chunk.key.as_str()) {
                retrieval.unread.push(UnreadNote {
                    path: chunk.path.clone(),
                    note_id: note_id(&chunk.key),
                    title: chunk.title.clone(),
                });
            }
        }
        retrieval.related_notes = listed.len();
        Ok(retrieval)
    }

    /// The notes most like a just-written note (#9), best first: at most `limit` of them, out of
    /// `total` scoring at least `min_score`. `exclude_id` is the written note's own id.
    ///
    /// The note is embedded the way notes are indexed, as a document, not as a search query, so
    /// a near-copy of an indexed chunk scores close to 1. Only its first chunk's worth is used,
    /// the same length every stored vector covers.
    pub fn similar(
        &self,
        title: &str,
        text: &str,
        exclude_id: &str,
        min_score: f32,
        limit: usize,
    ) -> Result<Similar, String> {
        let text: String = text.chars().take(CHUNK_CHARACTERS).collect();
        let Scored {
            database, chunks, ..
        } = self.score(document_input(title, &text), None)?;
        let exclude = format!("id:{exclude_id}");
        let chunks: Vec<ScoredChunk> = chunks
            .into_iter()
            .filter(|chunk| chunk.key != exclude && chunk.score >= min_score)
            .collect();
        let mut notes = best_per_note(&database, &chunks, usize::MAX)?;
        let total = notes.len();
        notes.truncate(limit);
        Ok(Similar { notes, total })
    }

    /// Scores every chunk at or above the cutoff, in note-then-ordinal order, and keeps the
    /// database locked so the caller can read the winners' text from the same snapshot.
    fn score(&self, input: String, category: Option<ParaCategory>) -> Result<Scored<'_>, String> {
        let embed_started = Instant::now();
        let mut query_embeddings = self.backend.embed(&[input])?;
        let embed_ms = embed_started.elapsed().as_secs_f64() * 1000.0;
        // Everything after the embedding counts as the scan, waits included, so a slow load or
        // a held lock shows up in exactScanMs instead of being hidden (#159).
        let scan_started = Instant::now();
        let query_embedding = query_embeddings
            .pop()
            .filter(|embedding| !embedding.is_empty())
            .ok_or("The embedding backend returned no query vector")?;
        let deadline = Instant::now() + CACHE_READY_DEADLINE;
        loop {
            self.wait_for_cache(deadline)?;
            self.search_event("waited");
            let lock_started = Instant::now();
            let database = self.database.lock().map_err(|error| error.to_string())?;
            let lock_wait_ms = lock_started.elapsed().as_secs_f64() * 1000.0;
            // A newer index replaced this one while the query was embedding. Its cache is frozen
            // while the shared database moves on, so it must not rank or hydrate (#159).
            if self.retired.load(Ordering::SeqCst) {
                return Err(
                    "Semantic search restarted with new settings. Search again.".to_string()
                );
            }
            let mut state = self.cache.state.lock().map_err(|error| error.to_string())?;
            if matches!(*state, CacheState::NotLoaded) {
                *state = loaded_state(load_vectors(&database));
            }
            let chunks = match &*state {
                CacheState::Ready(notes) => self.score_cached(notes, &query_embedding, category),
                // A background load began after the wait above: wait for it, never scan SQLite
                // meanwhile.
                CacheState::Loading => {
                    self.search_event("saw-loading");
                    continue;
                }
                CacheState::NotLoaded | CacheState::Failed => {
                    drop(state);
                    self.score_sql(&database, &query_embedding, category)?
                }
            };
            return Ok(Scored {
                database,
                chunks,
                embed_ms,
                lock_wait_ms,
                scan_started,
            });
        }
    }

    /// Scores from the vector cache, without reading any blob (#159).
    fn score_cached(
        &self,
        notes: &HashMap<String, CachedNote>,
        query_embedding: &[f32],
        category: Option<ParaCategory>,
    ) -> Vec<ScoredChunk> {
        let category_name = category.map(|value| value.folder_name());
        let mut scored = Vec::new();
        for (key, note) in notes {
            if note.profile != self.profile
                || category_name.is_some_and(|wanted| note.category.as_deref() != Some(wanted))
            {
                continue;
            }
            for (ordinal, embedding) in &note.chunks {
                let Some(score) = cosine_similarity(query_embedding, embedding) else {
                    continue;
                };
                if score >= MIN_SEMANTIC_SCORE {
                    scored.push(ScoredChunk {
                        key: key.clone(),
                        path: note.path.clone(),
                        title: note.title.clone(),
                        ordinal: *ordinal,
                        score,
                    });
                }
            }
        }
        scored
    }

    /// The scan used when the cache could not load: every chunk row, read from SQLite.
    fn score_sql(
        &self,
        database: &Connection,
        query_embedding: &[f32],
        category: Option<ParaCategory>,
    ) -> Result<Vec<ScoredChunk>, String> {
        #[cfg(test)]
        self.sql_scans.fetch_add(1, Ordering::SeqCst);
        let mut statement = database
            .prepare(
                "SELECT n.note_key, n.path, n.title, c.ordinal, c.embedding
                 FROM notes n JOIN chunks c ON c.note_key = n.note_key
                 WHERE n.profile = ?1 AND (?2 IS NULL OR n.category = ?2)
                 ORDER BY n.note_key, c.ordinal",
            )
            .map_err(|error| error.to_string())?;
        let category_name = category.map(|value| value.folder_name());
        let rows = statement
            .query_map(params![self.profile.as_str(), category_name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        let mut scored = Vec::new();
        for row in rows {
            let (key, path, title, ordinal, bytes) = row.map_err(|error| error.to_string())?;
            let embedding = decode(&bytes)?;
            let Some(score) = cosine_similarity(query_embedding, &embedding) else {
                continue;
            };
            if score >= MIN_SEMANTIC_SCORE {
                scored.push(ScoredChunk {
                    key,
                    path,
                    title,
                    ordinal,
                    score,
                });
            }
        }
        Ok(scored)
    }
    #[cfg(test)]
    fn search_event(&self, event: &'static str) {
        if let Some(events) = self.search_events.lock().unwrap().as_ref() {
            let _ = events.send(event);
        }
    }

    #[cfg(not(test))]
    fn search_event(&self, _event: &'static str) {}

    /// Applies a committed change to the cache. Callers still hold the database mutex.
    fn publish(&self, change: impl FnOnce(&mut HashMap<String, CachedNote>)) {
        if let Ok(mut state) = self.cache.state.lock() {
            if let CacheState::Ready(notes) = &mut *state {
                change(notes);
            }
        }
    }

    /// Waits, outside the database mutex, while a background load runs.
    fn wait_for_cache(&self, deadline: Instant) -> Result<(), String> {
        let mut state = self.cache.state.lock().map_err(|error| error.to_string())?;
        while matches!(*state, CacheState::Loading) {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(
                    "Semantic search is still loading its index. Try again in a moment."
                        .to_string(),
                );
            };
            state = self
                .cache
                .ready
                .wait_timeout(state, remaining)
                .map_err(|error| error.to_string())?
                .0;
        }
        Ok(())
    }

    /// Starts loading the cache off the calling thread. A search that arrives meanwhile waits.
    fn start_cache_load(self: &Arc<Self>) {
        {
            let Ok(mut state) = self.cache.state.lock() else {
                return;
            };
            if !matches!(*state, CacheState::NotLoaded) {
                return;
            }
            *state = CacheState::Loading;
        }
        let index = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("semantic-cache".to_string())
            .spawn(move || {
                if let Some(index) = index.upgrade() {
                    index.load_cache_now();
                }
            });
        if let Err(error) = spawned {
            log::warn!(
                "Semantic search uses the slower scan: its cache could not start loading: {error}"
            );
            self.finish_loading(CacheState::Failed);
        }
    }

    fn load_cache_now(&self) {
        // Whatever happens below, waiters must not sleep out their deadline on a dead load.
        struct WakeOnExit<'a>(&'a SemanticIndex);
        impl Drop for WakeOnExit<'_> {
            fn drop(&mut self) {
                self.0.finish_loading(CacheState::Failed);
            }
        }
        let _wake = WakeOnExit(self);
        let Ok(database) = self.database.lock() else {
            return;
        };
        // Published while the database mutex is still held, so no write lands between the
        // snapshot and the cache it becomes.
        let next = loaded_state(load_vectors(&database));
        self.finish_loading(next);
        drop(database);
    }

    /// Ends a load: sets `next` unless another outcome already landed, and wakes every waiter.
    fn finish_loading(&self, next: CacheState) {
        if let Ok(mut state) = self.cache.state.lock() {
            if matches!(*state, CacheState::Loading) {
                *state = next;
            }
        }
        self.cache.ready.notify_all();
    }

    /// Stops this index writing, because a newer index on the same database replaces it.
    /// Taking the database mutex first lets a write already in progress finish.
    pub fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    pub fn retire(&self) {
        let _database = self.database.lock();
        self.retired.store(true, Ordering::SeqCst);
    }

    /// Puts `next` in the app's slot and starts it. Whatever it displaces is retired first,
    /// whichever database it uses: after a switch away and back, the old instance of the same
    /// vault could otherwise still commit behind the new one's cache snapshot (#159).
    pub fn replace_in(slot: &mut Option<Arc<SemanticIndex>>, next: Arc<SemanticIndex>) {
        if let Some(old) = slot.take() {
            old.retire();
        }
        *slot = Some(next.clone());
        next.start_background();
    }

    pub fn rebuild_from_notes(&self, vault: &Path) -> Result<(), String> {
        let paths = note_paths(vault);
        {
            let mut database = self.database.lock().map_err(|error| error.to_string())?;
            if self.retired.load(Ordering::SeqCst) {
                return Ok(());
            }
            let transaction = database.transaction().map_err(|error| error.to_string())?;
            transaction
                .execute("DELETE FROM notes", [])
                .map_err(|error| error.to_string())?;
            transaction
                .execute("DELETE FROM pending_notes", [])
                .map_err(|error| error.to_string())?;
            transaction.commit().map_err(|error| error.to_string())?;
            // Ready and empty: the embeddings that follow refill it, so nothing reloads.
            self.publish(HashMap::clear);
        }
        let mut unreadable = 0;
        for path in paths {
            match std::fs::read_to_string(&path) {
                Ok(raw) => self.queue_text(&path, &raw)?,
                Err(_) => unreadable += 1,
            }
        }
        if unreadable > 0 {
            log::warn!("Semantic rebuild skipped unreadable notes: {unreadable}");
        }
        Ok(())
    }

    pub fn reconcile_from_notes(&self, vault: &Path) -> Result<(), String> {
        // Collected before any per-note work: a lazy walk keeps its directory handles open
        // for the whole pass, and on Windows an open handle under a folder stops a restore
        // from moving that folder (#144).
        self.reconcile_paths(&note_paths(vault))
    }

    fn reconcile_paths(&self, paths: &[std::path::PathBuf]) -> Result<(), String> {
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
        let mut unreadable = 0;
        for path in paths {
            let path_text = path.to_string_lossy().to_string();
            let raw = match std::fs::read_to_string(path) {
                Ok(raw) => raw,
                // Moved away since the walk (a restore in progress), locked, or not UTF-8: keep
                // what the index has for it, since a restore that rolls back puts it back, and
                // let the rest of the pass run. A later reconcile drops a note that is really
                // gone; one that stays unreadable keeps its old entry until it can be read.
                Err(_) => {
                    unreadable += 1;
                    seen.insert(path_text);
                    continue;
                }
            };
            let current = content_hash(&raw);
            seen.insert(path_text.clone());
            if recorded.get(&path_text) != Some(&(current, self.profile.clone())) {
                // Reuse the text read above; a second read fails if the note vanished between.
                self.queue_text(path, &raw)?;
            }
        }
        for path in recorded.keys().filter(|path| !seen.contains(*path)) {
            self.note_removed(Path::new(path))?;
        }
        if unreadable > 0 {
            log::warn!("Semantic reconcile skipped unreadable notes: {unreadable}");
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

    fn retry_pending(&self) -> Result<RetryOutcome, String> {
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
        let mut unreadable = 0;
        for path in paths {
            let path = std::path::PathBuf::from(path);
            // One read serves both steps, so a note removed mid-retry is caught here instead of
            // failing a later read (#144).
            let raw = match std::fs::read_to_string(&path) {
                Ok(raw) => raw,
                // Gone, or no longer a file: dropped from the queue and the index. If it comes
                // back (a restore that rolled back), the next reconcile queues it again.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound || !path.is_file() => {
                    self.note_removed(&path)?;
                    continue;
                }
                // Still a file but unreadable: it stays queued for the next retry instead of
                // holding up every note queued behind it.
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
            };
            let note = PreparedNote::from_text(&path, &raw);
            self.refresh_pending_text(&note)?;
            if !self.embed_pending_text(&note)? {
                return Ok(RetryOutcome::BackendUnavailable);
            }
        }
        // The worker retries every 20 s; the count is reported when it changes, not on every
        // tick.
        if self
            .last_reported_unreadable
            .swap(unreadable, Ordering::SeqCst)
            != unreadable
            && unreadable > 0
        {
            log::warn!("Semantic retry left unreadable notes queued: {unreadable}");
        }
        Ok(RetryOutcome::QueueProcessed)
    }

    pub fn start_background(self: &Arc<Self>) {
        self.start_cache_load();
        self.start_background_with_interval(Duration::from_secs(20));
    }

    fn start_background_with_interval(self: &Arc<Self>, interval: Duration) {
        self.start_background_worker(interval, || {});
    }

    fn start_background_worker(
        self: &Arc<Self>,
        interval: Duration,
        mut offline_interval_elapsed: impl FnMut() + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        if self.wake_worker.set(sender).is_err() {
            return;
        }
        let index = Arc::downgrade(self);
        std::thread::spawn(move || {
            let mut offline_until: Option<std::time::Instant> = None;
            loop {
                if let Some(deadline) = offline_until {
                    // Notes are already durable. Coalesce every wake received during an
                    // outage so reconciliation cannot turn a large vault into a retry burst.
                    while let Some(remaining) =
                        deadline.checked_duration_since(std::time::Instant::now())
                    {
                        match receiver.recv_timeout(remaining) {
                            Ok(()) => continue,
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    if std::time::Instant::now() >= deadline {
                        offline_interval_elapsed();
                    }
                } else {
                    match receiver.recv_timeout(interval) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                let Some(index) = index.upgrade() else {
                    break;
                };
                if index.retired.load(Ordering::SeqCst) {
                    break;
                }
                match index.retry_pending() {
                    Ok(RetryOutcome::QueueProcessed) => offline_until = None,
                    Ok(RetryOutcome::BackendUnavailable) => {
                        offline_until = Some(std::time::Instant::now() + interval)
                    }
                    Err(error) => {
                        offline_until = None;
                        log::warn!("Could not process the semantic embedding queue: {error}");
                    }
                }
            }
        });
    }
}

/// One chunk at or above the cutoff, before its text is read.
struct ScoredChunk {
    key: String,
    path: String,
    title: String,
    ordinal: i64,
    score: f32,
}

struct Scored<'a> {
    database: std::sync::MutexGuard<'a, Connection>,
    chunks: Vec<ScoredChunk>,
    embed_ms: f64,
    lock_wait_ms: f64,
    scan_started: Instant,
}

/// The best chunk of each note, best notes first. The first chunk wins a tie, as both scans
/// list a note's chunks in ordinal order.
fn best_per_note(
    database: &Connection,
    chunks: &[ScoredChunk],
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    let mut best: HashMap<&str, &ScoredChunk> = HashMap::new();
    for chunk in chunks {
        match best.get(chunk.key.as_str()) {
            Some(current) if current.score >= chunk.score => {}
            _ => {
                best.insert(chunk.key.as_str(), chunk);
            }
        }
    }
    let mut ranked: Vec<&ScoredChunk> = best.into_values().collect();
    ranked.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.title.cmp(&right.title))
    });
    ranked.truncate(limit);
    ranked
        .into_iter()
        .map(|chunk| {
            Ok(SearchResult {
                path: chunk.path.clone(),
                title: chunk.title.clone(),
                snippet: chunk_text(database, chunk)?,
                score: chunk.score,
            })
        })
        .collect()
}

/// The frontmatter id inside a note key, which is `id:<id>` for notes that have one.
fn note_id(key: &str) -> Option<String> {
    key.strip_prefix("id:").map(str::to_string)
}

fn chunk_text(database: &Connection, chunk: &ScoredChunk) -> Result<String, String> {
    database
        .prepare_cached("SELECT text FROM chunks WHERE note_key = ?1 AND ordinal = ?2")
        .and_then(|mut statement| {
            statement.query_row(params![chunk.key, chunk.ordinal], |row| row.get(0))
        })
        .map_err(|error| error.to_string())
}

/// What [`SemanticIndex::similar`] found: the best notes, and how many passed in all.
#[derive(Debug)]
pub struct Similar {
    pub notes: Vec<SearchResult>,
    pub total: usize,
}

/// What Ask reads for one question.
#[derive(Debug, Default)]
pub struct Retrieval {
    /// Best first.
    pub chunks: Vec<RetrievedChunk>,
    /// Notes with a chunk at or above the cutoff, read or not.
    pub related_notes: usize,
    /// Related notes none of whose chunks fit the budget, best first.
    pub unread: Vec<UnreadNote>,
    /// Read notes with at least one related chunk that did not fit.
    pub partly_read: usize,
}

#[derive(Debug)]
pub struct RetrievedChunk {
    pub path: String,
    /// The note's frontmatter id, when it has one, so a citation can tell its note from a
    /// later note at the same path.
    pub note_id: Option<String>,
    pub title: String,
    pub ordinal: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct UnreadNote {
    pub path: String,
    pub note_id: Option<String>,
    pub title: String,
}

/// Every indexed chunk's vector, decoded, grouped by note in ordinal order.
fn load_vectors(database: &Connection) -> Result<HashMap<String, CachedNote>, String> {
    let mut statement = database
        .prepare(
            "SELECT n.note_key, n.path, n.profile, n.category, c.ordinal, c.embedding, n.title
             FROM notes n JOIN chunks c ON c.note_key = n.note_key
             ORDER BY n.note_key, c.ordinal",
        )
        .map_err(|error| error.to_string())?;
    let mut rows = statement.query([]).map_err(|error| error.to_string())?;
    let mut notes: HashMap<String, CachedNote> = HashMap::new();
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let key: String = row.get(0).map_err(|error| error.to_string())?;
        let ordinal: i64 = row.get(4).map_err(|error| error.to_string())?;
        let bytes: Vec<u8> = row.get(5).map_err(|error| error.to_string())?;
        let embedding = decode(&bytes)?;
        if let Some(note) = notes.get_mut(&key) {
            note.chunks.push((ordinal, embedding));
            continue;
        }
        let note = CachedNote {
            path: row.get(1).map_err(|error| error.to_string())?,
            title: row.get(6).map_err(|error| error.to_string())?,
            profile: row.get(2).map_err(|error| error.to_string())?,
            category: row.get(3).map_err(|error| error.to_string())?,
            chunks: vec![(ordinal, embedding)],
        };
        notes.insert(key, note);
    }
    Ok(notes)
}

fn loaded_state(loaded: Result<HashMap<String, CachedNote>, String>) -> CacheState {
    match loaded {
        Ok(notes) => CacheState::Ready(notes),
        Err(error) => {
            log::warn!("Semantic search uses the slower scan: its cache could not load: {error}");
            CacheState::Failed
        }
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
            input: document_input(title, ""),
            snippet: String::new(),
        }];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < characters.len() {
        let end = (start + CHUNK_CHARACTERS).min(characters.len());
        let snippet: String = characters[start..end].iter().collect();
        chunks.push(NoteChunk {
            input: document_input(title, &snippet),
            snippet,
        });
        if end == characters.len() {
            break;
        }
        start = end - CHUNK_OVERLAP;
    }
    chunks
}

/// Every note the semantic index covers, collected eagerly so no directory handle outlives
/// the walk.
fn note_paths(vault: &Path) -> Vec<std::path::PathBuf> {
    WalkDir::new(vault)
        .into_iter()
        .filter_entry(|entry| !crate::search::is_ignored_by_index(entry.path(), vault))
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
        })
        .map(|entry| entry.into_path())
        .collect()
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

    use super::{
        CacheState, EmbeddingBackend, OllamaEmbeddingBackend, PreparedNote, SemanticIndex,
        UnreadNote, CHUNK_CHARACTERS,
    };
    use crate::types::SearchResult;
    use crate::vault::para::ParaCategory;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    struct MeaningBackend;

    struct UnavailableBackend;

    #[derive(Debug, PartialEq)]
    enum OfflineWorkerEvent {
        BackendCallStarted,
        RetryIntervalElapsed,
    }

    struct ObservedUnavailableBackend {
        events: std::sync::mpsc::Sender<OfflineWorkerEvent>,
        release_first_call: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    struct RecoveringBackend {
        available: Arc<AtomicBool>,
    }

    struct ObservedRecoveringBackend {
        available: Arc<AtomicBool>,
        first_call: Mutex<Option<std::sync::mpsc::Sender<()>>>,
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

    impl EmbeddingBackend for ObservedUnavailableBackend {
        fn embed(&self, _inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            let _ = self.events.send(OfflineWorkerEvent::BackendCallStarted);
            if let Some(release) = self.release_first_call.lock().unwrap().take() {
                release
                    .recv()
                    .expect("the test should release the first backend call");
            }
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

    impl EmbeddingBackend for ObservedRecoveringBackend {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            if !self.available.load(Ordering::SeqCst) {
                if let Some(first_call) = self.first_call.lock().unwrap().take() {
                    let _ = first_call.send(());
                }
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
                        // cosine 0.30 with the query: a weak but real chunks-v2 match
                        vec![0.3, 0.9539392]
                    } else if input.contains("weak") {
                        // cosine 0.19: the best score a query with no matching note reached
                        vec![0.19, 0.9817841]
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
        // Quoted, so a title holding `: ` stays a title (a JSON string is valid YAML).
        let title = serde_json::to_string(title).unwrap();
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

    fn text_only(_title: &str, text: &str, _first: bool) -> usize {
        text.chars().count()
    }

    #[test]
    fn retrieval_reads_every_related_chunk_best_first_within_the_budget() {
        let root = scratch("retrieve-budget");
        let long = root.join("Long.md");
        let short = root.join("Short.md");
        let unrelated = root.join("Unrelated.md");
        // Three chunks, each about coffee; the budget below fits two of them.
        let long_body = format!(
            "{}{}",
            "coffee ".repeat(CHUNK_CHARACTERS / 7),
            "coffee ".repeat(CHUNK_CHARACTERS / 7 + 100)
        );
        write_note(&long, "long-id", "Long coffee log", "Areas", &long_body);
        write_note(&short, "short-id", "Short coffee", "Areas", "coffee");
        write_note(
            &unrelated,
            "other-id",
            "Other",
            "Areas",
            "Quantum mechanics.",
        );
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        for note in [&long, &short, &unrelated] {
            index.note_changed(note).unwrap();
        }
        index.retry_pending().unwrap();
        let everything = index
            .retrieve("mornings", None, usize::MAX, text_only)
            .unwrap();
        assert_eq!(everything.related_notes, 2);
        assert_eq!(everything.chunks.len(), 4);
        assert!(everything.unread.is_empty());

        // Ties on score order by title, then ordinal: the long note's chunks come first.
        let budget: usize = everything.chunks[..2]
            .iter()
            .map(|chunk| chunk.text.chars().count())
            .sum();
        let partial = index.retrieve("mornings", None, budget, text_only).unwrap();
        let read: Vec<(&str, i64)> = partial
            .chunks
            .iter()
            .map(|chunk| (chunk.title.as_str(), chunk.ordinal))
            .collect();
        assert_eq!(read, vec![("Long coffee log", 0), ("Long coffee log", 1)]);
        assert_eq!(partial.related_notes, 2);
        assert_eq!(
            partial.partly_read, 1,
            "the long note's third chunk did not fit"
        );
        assert_eq!(everything.partly_read, 0);
        assert_eq!(
            partial.unread,
            vec![UnreadNote {
                path: short.to_string_lossy().to_string(),
                note_id: Some("short-id".to_string()),
                title: "Short coffee".to_string(),
            }]
        );
        drop(index);
        cleanup(root);
    }

    #[test]
    fn similar_notes_skip_the_new_note_itself_and_count_every_match_past_the_cap() {
        let root = scratch("similar");
        let capture = root.join("Capture.md");
        let first = root.join("First.md");
        let second = root.join("Second.md");
        let unrelated = root.join("Unrelated.md");
        write_note(&capture, "capture-id", "Capture", "Areas", "coffee beans");
        write_note(&first, "first-id", "First", "Areas", "coffee ratio");
        write_note(&second, "second-id", "Second", "Projects", "coffee grinder");
        write_note(
            &unrelated,
            "other-id",
            "Other",
            "Areas",
            "Quantum mechanics.",
        );
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        for note in [&capture, &first, &second, &unrelated] {
            index.note_changed(note).unwrap();
        }
        index.retry_pending().unwrap();

        let found = index
            .similar("Capture", "coffee beans", "capture-id", 0.9, 1)
            .unwrap();
        assert_eq!(found.total, 2, "both coffee notes pass, across categories");
        assert_eq!(found.notes.len(), 1, "the cap holds");
        assert_eq!(found.notes[0].title, "First", "ties order by title");
        drop(index);
        cleanup(root);
    }

    #[test]
    fn the_similarity_check_skips_short_notes_and_drops_matches_gone_from_disk() {
        let root = scratch("similarity-check");
        let vault = root.to_str().unwrap();
        let capture = root.join("Capture.md");
        let short = root.join("Short.md");
        let kept = root.join("Kept.md");
        let gone = root.join("Gone.md");
        write_note(
            &capture,
            "capture-id",
            "Capture",
            "Areas",
            "coffee beans today",
        );
        write_note(&short, "short-id", "Short", "Areas", "coffee");
        write_note(&kept, "kept-id", "Kept", "Areas", "coffee ratio");
        write_note(&gone, "gone-id", "Gone", "Areas", "coffee grinder");
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        for note in [&capture, &short, &kept, &gone] {
            index.note_changed(note).unwrap();
        }
        index.retry_pending().unwrap();
        std::fs::remove_file(&gone).unwrap();

        let check = |note: &Path| {
            crate::similar_notes::check(&index, vault, note.to_str().unwrap()).unwrap()
        };
        assert!(check(&short).is_none(), "two words are too few to judge");
        let found = check(&capture).expect("the coffee notes are similar");
        let titles: Vec<&str> = found
            .matches
            .iter()
            .map(|found| found.note.title.as_str())
            .collect();
        assert_eq!(
            titles,
            ["Kept", "Short"],
            "a deleted note has nothing to append to"
        );
        drop(index);
        cleanup(root);
    }

    /// Scores every fixture capture against every fixture note with live embeddinggemma, the
    /// numbers the capture-similarity bar (`similar_notes::SIMILAR_SCORE`) is set from (#9). Run:
    /// `SB_OLLAMA_URL=http://127.0.0.1:11434 cargo test similarity_calibration -- --ignored --nocapture`
    #[test]
    #[ignore = "needs embeddinggemma on Ollama at SB_OLLAMA_URL; prints the similarity scores"]
    fn similarity_calibration() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../scripts/similarity-fixture.json")).unwrap();
        let url =
            std::env::var("SB_OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
        let root = scratch("similarity-calibration");
        let index = SemanticIndex::open_at(
            &root.join("semantic.sqlite3"),
            Arc::new(super::OllamaEmbeddingBackend::new(&url, None).unwrap()),
        )
        .unwrap();
        for (number, note) in fixture["notes"].as_array().unwrap().iter().enumerate() {
            let path = root.join(format!("{number}.md"));
            write_note(
                &path,
                &format!("note-{number}"),
                note["title"].as_str().unwrap(),
                note["category"].as_str().unwrap(),
                note["body"].as_str().unwrap(),
            );
            index.note_changed(&path).unwrap();
        }
        index.retry_pending().unwrap();

        let (mut lowest_duplicate, mut highest_other) = (f32::MAX, f32::MIN);
        for capture in fixture["captures"].as_array().unwrap() {
            // Filed the way the overlay files it: the first line is the title.
            let filed = crate::hotkey::capture::split(capture["text"].as_str().unwrap()).unwrap();
            let found = index
                .similar(&filed.title, &filed.body, "capture", f32::MIN, usize::MAX)
                .unwrap();
            let duplicate = capture["duplicateOf"].as_str();
            for note in &found.notes {
                if Some(note.title.as_str()) == duplicate {
                    lowest_duplicate = lowest_duplicate.min(note.score);
                } else {
                    highest_other = highest_other.max(note.score);
                }
            }
            let scores: Vec<String> = found
                .notes
                .iter()
                .map(|note| format!("{:.3} {}", note.score, note.title))
                .collect();
            println!(
                "{}\n  duplicate of: {}\n  {}",
                filed.title,
                duplicate.unwrap_or("-"),
                scores.join("\n  ")
            );
        }
        println!(
            "lowest duplicate score {lowest_duplicate:.3}; highest score of any other note {highest_other:.3}"
        );
        drop(index);
        cleanup(root);
    }

    #[test]
    fn retrieval_charges_each_note_its_overhead_so_tiny_notes_cannot_overflow_the_prompt() {
        let root = scratch("retrieve-overhead");
        let notes: Vec<_> = (0..4)
            .map(|index| {
                let path = root.join(format!("Tiny {index}.md"));
                write_note(
                    &path,
                    &format!("tiny-{index}"),
                    &format!("Tiny {index}"),
                    "Areas",
                    "coffee",
                );
                path
            })
            .collect();
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        for note in &notes {
            index.note_changed(note).unwrap();
        }
        index.retry_pending().unwrap();
        // Each note's first chunk costs 100 on top of its six characters of text.
        let with_overhead =
            |_: &str, text: &str, first: bool| text.chars().count() + if first { 100 } else { 0 };

        let retrieval = index
            .retrieve("mornings", None, 250, with_overhead)
            .unwrap();

        assert_eq!(retrieval.chunks.len(), 2);
        assert_eq!(retrieval.unread.len(), 2);
        drop(index);
        cleanup(root);
    }

    #[test]
    fn a_replaced_index_refuses_to_retrieve() {
        let root = scratch("retrieve-retired");
        let note = root.join("Coffee.md");
        write_note(&note, "coffee-id", "Coffee", "Areas", "coffee");
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        index.note_changed(&note).unwrap();
        index.retry_pending().unwrap();

        // A vault switch retires the old index; a question started before it must not read
        // whatever the shared database holds next.
        index.retire();

        assert!(index.is_retired());
        assert!(index
            .retrieve("mornings", None, usize::MAX, text_only)
            .is_err());
        drop(index);
        cleanup(root);
    }

    #[test]
    fn retrieval_respects_the_category_filter() {
        let root = scratch("retrieve-category");
        let area = root.join("Area.md");
        let project = root.join("Project.md");
        write_note(&area, "area-id", "Area coffee", "Areas", "coffee");
        write_note(
            &project,
            "project-id",
            "Project coffee",
            "Projects",
            "coffee",
        );
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();
        index.note_changed(&area).unwrap();
        index.note_changed(&project).unwrap();
        index.retry_pending().unwrap();

        let retrieval = index
            .retrieve(
                "mornings",
                Some(ParaCategory::Projects),
                usize::MAX,
                text_only,
            )
            .unwrap();

        assert_eq!(retrieval.related_notes, 1);
        assert_eq!(retrieval.chunks[0].title, "Project coffee");
        drop(index);
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

    struct RecordingBackend(Mutex<Vec<String>>);

    impl EmbeddingBackend for RecordingBackend {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            self.0.lock().unwrap().extend(inputs.iter().cloned());
            Ok(inputs.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    #[test]
    fn queries_and_notes_use_embeddinggemmas_retrieval_prompts() {
        let root = scratch("retrieval-prompts");
        let note = root.join("Bread.md");
        write_note(
            &note,
            "bread-id",
            "Sourdough",
            "Resources",
            "A long cold proof.",
        );
        let untitled = root.join("Untitled.md");
        write_note(&untitled, "untitled-id", "", "Resources", "");
        let backend = Arc::new(RecordingBackend(Mutex::new(Vec::new())));
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), backend.clone()).unwrap();
        index.note_changed(&note).unwrap();
        index.note_changed(&untitled).unwrap();
        index.retry_pending().unwrap();
        index.search("how to bake bread", None, 10).unwrap();

        let inputs = backend.0.lock().unwrap().clone();
        assert!(inputs.contains(&"title: Sourdough | text: A long cold proof.".to_string()));
        assert!(inputs
            .iter()
            .any(|input| input.starts_with("title: ") && input.ends_with("| text: ")));
        assert_eq!(
            inputs.last().unwrap(),
            "task: search result | query: how to bake bread"
        );
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
    fn an_unchanged_indexed_note_is_not_queued_again() {
        let root = scratch("unchanged-requeue");
        let note = root.join("Settled.md");
        write_note(
            &note,
            "settled-id",
            "Settled",
            "Areas",
            "Already embedded text.",
        );
        let database = root.join("semantic.sqlite3");
        let index = SemanticIndex::open_at(&database, Arc::new(ThresholdBackend)).unwrap();
        index.note_changed(&note).unwrap();
        index.retry_pending().unwrap();
        assert_eq!(index.status().unwrap().queued_notes, 0);
        drop(index);

        // Offline now, so anything queued would stay queued and show as waiting.
        let offline = SemanticIndex::open_at(&database, Arc::new(UnavailableBackend)).unwrap();
        offline.note_changed(&note).unwrap();
        let status = offline.status().unwrap();
        assert_eq!((status.indexed_notes, status.queued_notes), (1, 0));

        write_note(&note, "settled-id", "Settled", "Areas", "Edited text.");
        offline.note_changed(&note).unwrap();
        assert_eq!(
            offline.status().unwrap().queued_notes,
            1,
            "a real edit is still queued"
        );
        drop(offline);
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
        let (first_call, first_call_received) = std::sync::mpsc::channel();
        let index = Arc::new(
            SemanticIndex::open_at(
                &root.join("semantic.sqlite3"),
                Arc::new(ObservedRecoveringBackend {
                    available: available.clone(),
                    first_call: Mutex::new(Some(first_call)),
                }),
            )
            .unwrap(),
        );
        let retry_interval = std::time::Duration::from_millis(50);
        index.start_background_with_interval(retry_interval);
        index.note_changed(&note).unwrap();
        first_call_received
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the worker should attempt the queued note while offline");
        available.store(true, Ordering::SeqCst);

        // The next scheduled retry should process the durable queue after recovery. Windows
        // can be heavily loaded, so retain a bounded but generous scheduling window.
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
    fn an_unavailable_backend_is_retried_only_at_the_worker_interval() {
        let root = scratch("background-offline-interval");
        let note = root.join("Offline.md");
        write_note(
            &note,
            "offline-background-id",
            "Offline thought",
            "Projects",
            "A thought saved while the desktop is asleep.",
        );
        let (events, observed_events) = std::sync::mpsc::channel();
        let (release_first_call, first_call_release) = std::sync::mpsc::channel();
        let index = Arc::new(
            SemanticIndex::open_at(
                &root.join("semantic.sqlite3"),
                Arc::new(ObservedUnavailableBackend {
                    events: events.clone(),
                    release_first_call: Mutex::new(Some(first_call_release)),
                }),
            )
            .unwrap(),
        );
        let retry_interval = std::time::Duration::from_millis(200);
        index.start_background_worker(retry_interval, move || {
            let _ = events.send(OfflineWorkerEvent::RetryIntervalElapsed);
        });
        index.note_changed(&note).unwrap();

        // Hold the first attempt inside the backend while the extra wakes are queued. That
        // fixes their ordering without asking the test thread to finish inside a wall-clock
        // window: however slowly the runner schedules this code, all wakes precede the outage.
        assert_eq!(
            observed_events
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the external change should wake the semantic worker"),
            OfflineWorkerEvent::BackendCallStarted
        );
        for _ in 0..5 {
            index.note_changed(&note).unwrap();
        }
        release_first_call.send(()).unwrap();

        assert_eq!(
            observed_events
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the retry interval should elapse"),
            OfflineWorkerEvent::RetryIntervalElapsed,
            "queued wakes bypassed the retry interval"
        );
        assert_eq!(
            observed_events
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the worker should retry when the interval elapses"),
            OfflineWorkerEvent::BackendCallStarted
        );

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
    fn a_note_that_vanishes_mid_pass_keeps_its_entry_until_the_next_reconcile() {
        let root = scratch("reconcile-vanished");
        let areas = root.join("Areas");
        std::fs::create_dir_all(&areas).unwrap();
        let kept = areas.join("Kept.md");
        let moved = areas.join("Moved.md");
        write_note(&kept, "kept-id", "Kept", "Areas", "taxes");
        write_note(&moved, "moved-id", "Moved", "Areas", "taxes");
        let calls = Arc::new(AtomicUsize::new(0));
        let index = SemanticIndex::open_at(
            &root.join("semantic.sqlite3"),
            Arc::new(CountingBackend {
                calls: calls.clone(),
            }),
        )
        .unwrap();
        index.note_changed(&kept).unwrap();
        index.note_changed(&moved).unwrap();
        index.retry_pending().unwrap();

        // A restore moves the note away after the walk listed it (#144). The restore may
        // still roll back, so this pass keeps the entry; the next one, which walks again,
        // drops it.
        let paths = super::note_paths(&root);
        std::fs::remove_file(&moved).unwrap();
        index.reconcile_paths(&paths).unwrap();
        assert_eq!(index.status().unwrap().indexed_notes, 2);

        index.reconcile_from_notes(&root).unwrap();
        assert_eq!(index.status().unwrap().indexed_notes, 1);
        assert_eq!(index.status().unwrap().queued_notes, 0);
        drop(index);
        cleanup(root);
    }

    #[test]
    fn an_unreadable_note_neither_stops_reconciliation_nor_leaves_the_index() {
        let root = scratch("reconcile-unreadable");
        let areas = root.join("Areas");
        std::fs::create_dir_all(&areas).unwrap();
        let broken = areas.join("Broken.md");
        let later = areas.join("Later.md");
        write_note(&broken, "broken-id", "Broken", "Areas", "taxes");
        let calls = Arc::new(AtomicUsize::new(0));
        let index = SemanticIndex::open_at(
            &root.join("semantic.sqlite3"),
            Arc::new(CountingBackend {
                calls: calls.clone(),
            }),
        )
        .unwrap();
        index.note_changed(&broken).unwrap();
        index.retry_pending().unwrap();

        std::fs::write(&broken, [0xff, 0xfe, 0xfd]).unwrap();
        write_note(&later, "later-id", "Later", "Areas", "coffee");
        index.reconcile_from_notes(&root).unwrap();

        // The unreadable note keeps its entry, and the note after it is still queued.
        assert_eq!(index.status().unwrap().indexed_notes, 1);
        assert_eq!(index.status().unwrap().queued_notes, 1);
        drop(index);
        cleanup(root);
    }

    #[test]
    fn an_unreadable_queued_note_does_not_hold_up_the_rest_of_the_queue() {
        let root = scratch("retry-unreadable");
        let areas = root.join("Areas");
        std::fs::create_dir_all(&areas).unwrap();
        let broken = areas.join("Broken.md");
        let behind = areas.join("Behind.md");
        write_note(&broken, "broken-id", "Broken", "Areas", "taxes");
        write_note(&behind, "behind-id", "Behind", "Areas", "coffee");
        let calls = Arc::new(AtomicUsize::new(0));
        let index = SemanticIndex::open_at(
            &root.join("semantic.sqlite3"),
            Arc::new(CountingBackend {
                calls: calls.clone(),
            }),
        )
        .unwrap();
        index.note_changed(&broken).unwrap();
        index.note_changed(&behind).unwrap();
        std::fs::write(&broken, [0xff, 0xfe, 0xfd]).unwrap();

        index.retry_pending().unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(index.status().unwrap().indexed_notes, 1);
        assert_eq!(
            index.status().unwrap().queued_notes,
            1,
            "the unreadable note stays queued"
        );
        drop(index);
        cleanup(root);
    }

    #[test]
    fn rebuild_skips_an_unreadable_note_and_queues_the_notes_after_it() {
        let root = scratch("rebuild-unreadable");
        let areas = root.join("Areas");
        std::fs::create_dir_all(&areas).unwrap();
        let broken = areas.join("Broken.md");
        let later = areas.join("Later.md");
        std::fs::write(&broken, [0xff, 0xfe]).unwrap();
        write_note(&later, "later-id", "Later", "Areas", "coffee");
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(MeaningBackend))
                .unwrap();

        index.rebuild_from_notes(&root).unwrap();
        assert_eq!(index.status().unwrap().queued_notes, 1);
        index.retry_pending().unwrap();
        assert_eq!(index.status().unwrap().indexed_notes, 1);

        write_note(&broken, "broken-id", "Broken", "Areas", "taxes");
        index.reconcile_from_notes(&root).unwrap();
        assert_eq!(index.status().unwrap().queued_notes, 1);
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

    fn recorded_schema_version(database: &Path) -> i64 {
        let connection = rusqlite::Connection::open(database).unwrap();
        connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    fn stamp_schema_version(database: &Path, version: i64) {
        let connection = rusqlite::Connection::open(database).unwrap();
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
    }

    /// Index one note and return the vault root and database path it used.
    fn indexed_vault(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = scratch(label);
        let areas = root.join("Areas");
        std::fs::create_dir_all(&areas).unwrap();
        let note = areas.join("Routine.md");
        let database = root.join("semantic.sqlite3");
        write_note(&note, "schema-id", "Routine", "Areas", "coffee before work");
        let index = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();
        index.note_changed(&note).unwrap();
        index.retry_pending().unwrap();
        assert_eq!(index.status().unwrap().indexed_notes, 1);
        drop(index);
        (root, database)
    }

    #[test]
    fn a_fresh_database_records_the_schema_version_it_was_created_with() {
        let root = scratch("schema-stamp");
        let database = root.join("semantic.sqlite3");
        let index = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();
        drop(index);

        assert_eq!(
            recorded_schema_version(&database),
            super::SEMANTIC_SCHEMA_VERSION,
            "a database created by this build must declare its own schema version"
        );
        cleanup(root);
    }

    #[test]
    fn a_database_at_the_current_schema_version_is_reopened_with_its_work_intact() {
        let (root, database) = indexed_vault("schema-compatible");

        let reopened = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();

        // Nothing was discarded: reopening a compatible index must not cost the user a rebuild.
        assert_eq!(reopened.status().unwrap().indexed_notes, 1);
        assert_eq!(reopened.status().unwrap().queued_notes, 0);
        assert_eq!(
            reopened
                .search("how I structure my mornings", None, 10)
                .unwrap()
                .len(),
            1
        );
        drop(reopened);
        cleanup(root);
    }

    #[test]
    fn an_unrecognized_schema_version_is_rebuilt_rather_than_reused() {
        let (root, database) = indexed_vault("schema-newer");
        // A newer build wrote this file; its tables may mean something else entirely.
        stamp_schema_version(&database, super::SEMANTIC_SCHEMA_VERSION + 7);

        let reopened = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();

        assert_eq!(
            reopened.status().unwrap().indexed_notes,
            0,
            "derived rows from an unknown schema must not be reused"
        );
        assert_eq!(
            recorded_schema_version(&database),
            super::SEMANTIC_SCHEMA_VERSION,
            "the replacement must declare the version this build actually created"
        );

        // Rebuild recovery: the Markdown is the source of truth, so the index comes back.
        reopened.reconcile_from_notes(&root).unwrap();
        reopened.retry_pending().unwrap();
        assert_eq!(reopened.status().unwrap().indexed_notes, 1);
        assert_eq!(
            reopened
                .search("how I structure my mornings", None, 10)
                .unwrap()
                .len(),
            1
        );
        drop(reopened);
        cleanup(root);
    }

    #[test]
    fn a_database_predating_schema_versioning_is_rebuilt() {
        let (root, database) = indexed_vault("schema-unversioned");
        // Version 0 is what every database written before this contract existed reports.
        stamp_schema_version(&database, 0);

        let reopened = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();

        assert_eq!(reopened.status().unwrap().indexed_notes, 0);
        assert_eq!(
            recorded_schema_version(&database),
            super::SEMANTIC_SCHEMA_VERSION
        );
        reopened.reconcile_from_notes(&root).unwrap();
        reopened.retry_pending().unwrap();
        assert_eq!(reopened.status().unwrap().indexed_notes, 1);
        drop(reopened);
        cleanup(root);
    }

    #[test]
    fn an_incomplete_set_of_derived_tables_is_replaced_not_patched() {
        let (root, database) = indexed_vault("schema-partial");
        {
            // A half-written or half-migrated file: the version says current, the tables do not.
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection.execute_batch("DROP TABLE chunks;").unwrap();
        }

        let reopened = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();

        assert_eq!(reopened.status().unwrap().indexed_notes, 0);
        reopened.reconcile_from_notes(&root).unwrap();
        reopened.retry_pending().unwrap();
        assert_eq!(reopened.status().unwrap().indexed_notes, 1);
        assert_eq!(
            reopened
                .search("how I structure my mornings", None, 10)
                .unwrap()
                .len(),
            1,
            "a replaced database must be able to serve searches again"
        );
        drop(reopened);
        cleanup(root);
    }

    #[test]
    fn a_rebuilt_database_keeps_the_embedding_profile_contract() {
        // Schema version and embedding profile are separate contracts: replacing the file for
        // a schema reason must still record vectors under the profile this build compares.
        let (root, database) = indexed_vault("schema-profile");
        stamp_schema_version(&database, super::SEMANTIC_SCHEMA_VERSION + 1);

        let reopened = SemanticIndex::open_at(&database, Arc::new(MeaningBackend)).unwrap();
        reopened.reconcile_from_notes(&root).unwrap();
        reopened.retry_pending().unwrap();

        drop(reopened);
        // Scoped: Windows refuses to delete a database file while any handle is still open,
        // so the connection has to be gone before `cleanup` removes the directory.
        let profiles: Vec<String> = {
            let connection = rusqlite::Connection::open(&database).unwrap();
            let mut statement = connection
                .prepare("SELECT DISTINCT profile FROM notes")
                .unwrap();
            let profiles = statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            profiles
        };
        assert_eq!(profiles, vec![super::EMBEDDING_PROFILE.to_string()]);
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

        drop(index);
        // Seeded first, then reopened, so the timed searches use the cache an opened vault has.
        let index = Arc::new(
            SemanticIndex::open_at(
                &root.join("semantic.sqlite3"),
                Arc::new(FixedSizeBackend {
                    dimensions: DIMENSIONS,
                }),
            )
            .unwrap(),
        );
        index.start_background();
        let mut timings = Vec::new();
        for _ in 0..5 {
            let started = std::time::Instant::now();
            let results = index.search("representative query", None, 20).unwrap();
            timings.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(results.len(), 20);
        }
        println!(
            "semantic brute-force benchmark: {NOTES} notes, {DIMENSIONS} dimensions, ms per search {timings:.1?}"
        );
        drop(index);
        cleanup(root);
    }
    // ── #159: the vector cache ranks exactly as the SQL scan, and stays equal to SQLite ──

    /// `(key, path, title, category, chunks)` for one cached note.
    type CacheRow = (String, String, String, Option<String>, Vec<(i64, Vec<f32>)>);

    /// Words map to fixed axes and the title is ignored, so notes with the same body tie
    /// exactly and only their titles order them.
    struct WordAxesBackend;

    impl EmbeddingBackend for WordAxesBackend {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            const AXES: [&str; 6] = ["coffee", "garden", "budget", "jazz", "river", "chess"];
            Ok(inputs
                .iter()
                .map(|input| {
                    let text = input
                        .split(" | text: ")
                        .last()
                        .unwrap_or(input)
                        .to_lowercase();
                    let mut vector: Vec<f32> = AXES
                        .iter()
                        .map(|axis| text.matches(axis).count() as f32)
                        .collect();
                    vector.push(0.05);
                    vector
                })
                .collect())
        }
    }

    fn cached_snapshot(index: &SemanticIndex) -> Vec<CacheRow> {
        let state = index.cache.state.lock().unwrap();
        let CacheState::Ready(notes) = &*state else {
            panic!("the cache is not ready");
        };
        let mut rows: Vec<_> = notes
            .iter()
            .map(|(key, note)| {
                (
                    key.clone(),
                    note.path.clone(),
                    note.title.clone(),
                    note.category.clone(),
                    note.chunks.clone(),
                )
            })
            .collect();
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        rows
    }

    fn sqlite_snapshot(index: &SemanticIndex) -> Vec<CacheRow> {
        let database = index.database.lock().unwrap();
        let mut rows: Vec<_> = super::load_vectors(&database)
            .unwrap()
            .into_iter()
            .map(|(key, note)| (key, note.path, note.title, note.category, note.chunks))
            .collect();
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        rows
    }

    fn sql_results(
        index: &SemanticIndex,
        query: &str,
        category: Option<ParaCategory>,
        limit: usize,
    ) -> Vec<SearchResult> {
        let query_embedding = index
            .backend
            .embed(&[format!("{}{query}", super::QUERY_PROMPT)])
            .unwrap()
            .pop()
            .unwrap();
        let database = index.database.lock().unwrap();
        let chunks = index
            .score_sql(&database, &query_embedding, category)
            .unwrap();
        super::best_per_note(&database, &chunks, limit).unwrap()
    }

    fn assert_same_results(left: &[SearchResult], right: &[SearchResult]) {
        let shape = |results: &[SearchResult]| {
            results
                .iter()
                .map(|result| {
                    (
                        result.path.clone(),
                        result.title.clone(),
                        result.snippet.clone(),
                        result.score.to_bits(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(left), shape(right));
    }

    /// A vault whose ranking exercises every rule: several chunks per note with the best one
    /// winning, score ties broken by title across the limit, a note below the cutoff, and two
    /// categories.
    fn ranking_vault(root: &Path) -> Vec<std::path::PathBuf> {
        let long_body = format!("{} river chess chess chess", "filler ".repeat(400));
        let notes = [
            (
                "a",
                "Alpha tie",
                "Areas",
                "coffee coffee budget".to_string(),
            ),
            ("b", "Beta tie", "Areas", "coffee coffee budget".to_string()),
            (
                "c",
                "Gamma tie",
                "Projects",
                "coffee coffee budget".to_string(),
            ),
            ("d", "Long note", "Resources", long_body),
            ("e", "Jazz only", "Areas", "jazz jazz jazz".to_string()),
            (
                "f",
                "Coffee garden",
                "Projects",
                "coffee garden".to_string(),
            ),
        ];
        notes
            .iter()
            .map(|(id, title, category, body)| {
                let path = root.join(format!("{title}.md"));
                write_note(&path, id, title, category, body);
                path
            })
            .collect()
    }

    #[test]
    fn the_cache_ranks_exactly_as_the_sql_scan() {
        let root = scratch("cache-ranking");
        let paths = ranking_vault(&root);
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(WordAxesBackend))
                .unwrap();
        for path in &paths {
            index.note_changed(path).unwrap();
        }
        index.retry_pending().unwrap();

        for (query, category, limit) in [
            ("coffee budget", None, 10),
            ("coffee budget", None, 2),
            ("coffee budget", Some(ParaCategory::Areas), 1),
            ("chess and the river", None, 10),
            ("jazz", Some(ParaCategory::Projects), 10),
            ("coffee", None, 0),
        ] {
            let cached = index.search(query, category, limit).unwrap();
            assert!(matches!(
                *index.cache.state.lock().unwrap(),
                CacheState::Ready(_)
            ));
            assert_same_results(&cached, &sql_results(&index, query, category, limit));
        }
        // The winning chunk's text is the snippet: the long note matched on its last chunk.
        let long = index.search("chess and the river", None, 1).unwrap();
        assert_eq!(long[0].title, "Long note");
        assert!(long[0].snippet.contains("chess"));
        // Ties at the limit are broken by title, not by load order.
        let tied = index.search("coffee budget", None, 2).unwrap();
        assert_eq!(
            tied.iter()
                .map(|result| result.title.as_str())
                .collect::<Vec<_>>(),
            ["Alpha tie", "Beta tie"]
        );

        drop(index);
        cleanup(root);
    }

    #[test]
    fn the_cache_stays_equal_to_sqlite_through_every_write() {
        let root = scratch("cache-writes");
        let paths = ranking_vault(&root);
        let database = root.join("semantic.sqlite3");
        let index = SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap();
        index.search("coffee", None, 5).unwrap(); // loads the (empty) cache inline
        for path in &paths {
            index.note_changed(path).unwrap();
        }
        index.retry_pending().unwrap();
        assert_eq!(cached_snapshot(&index), sqlite_snapshot(&index));

        // An edited note replaces its chunks.
        write_note(&paths[0], "a", "Alpha tie", "Areas", "garden garden");
        index.note_changed(&paths[0]).unwrap();
        index.retry_pending().unwrap();
        assert_eq!(cached_snapshot(&index), sqlite_snapshot(&index));

        // Another note now at the same path displaces the old identity.
        write_note(&paths[1], "b-replaced", "Beta tie", "Areas", "chess");
        index.note_changed(&paths[1]).unwrap();
        index.retry_pending().unwrap();
        let snapshot = cached_snapshot(&index);
        assert!(snapshot.iter().all(|row| row.0 != "id:b"));
        assert_eq!(snapshot, sqlite_snapshot(&index));

        index.note_removed(&paths[2]).unwrap();
        assert_eq!(cached_snapshot(&index), sqlite_snapshot(&index));

        // A rebuild empties it, and the embeddings that follow refill it.
        index.rebuild_from_notes(&root).unwrap();
        assert!(cached_snapshot(&index).is_empty());
        index.retry_pending().unwrap();
        assert_eq!(cached_snapshot(&index), sqlite_snapshot(&index));
        let before_restart = cached_snapshot(&index);
        drop(index);

        // A restart loads the same cache from disk.
        let reopened = SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap();
        reopened.search("coffee", None, 5).unwrap();
        assert_eq!(cached_snapshot(&reopened), before_restart);
        drop(reopened);
        cleanup(root);
    }

    #[test]
    fn a_search_during_the_cache_load_waits_and_returns_everything() {
        let root = scratch("cache-loading");
        let paths = ranking_vault(&root);
        let database = root.join("semantic.sqlite3");
        let seed = SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap();
        for path in &paths {
            seed.note_changed(path).unwrap();
        }
        seed.retry_pending().unwrap();
        let expected = seed.search("coffee budget", None, 10).unwrap();
        drop(seed);

        let index = Arc::new(SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap());
        // Holding the database keeps the load waiting, so the search starts mid-load.
        let held = index.database.lock().unwrap();
        index.start_cache_load();
        assert!(matches!(
            *index.cache.state.lock().unwrap(),
            CacheState::Loading
        ));
        let searcher = {
            let index = index.clone();
            std::thread::spawn(move || index.search("coffee budget", None, 10))
        };
        std::thread::sleep(Duration::from_millis(50));
        drop(held);
        let found = searcher.join().unwrap().unwrap();
        assert_same_results(&found, &expected);

        drop(index);
        cleanup(root);
    }

    #[test]
    fn a_stalled_load_times_out_and_a_failed_one_falls_back_to_sql() {
        let root = scratch("cache-failure");
        let paths = ranking_vault(&root);
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(WordAxesBackend))
                .unwrap();
        for path in &paths {
            index.note_changed(path).unwrap();
        }
        index.retry_pending().unwrap();

        *index.cache.state.lock().unwrap() = CacheState::Loading;
        let started = Instant::now();
        let error = index.search("coffee", None, 10).unwrap_err();
        assert!(error.contains("still loading"), "{error}");
        assert!(started.elapsed() >= super::CACHE_READY_DEADLINE);

        index.finish_loading(CacheState::Failed);
        let fallback = index.search("coffee budget", None, 10).unwrap();
        assert!(matches!(
            *index.cache.state.lock().unwrap(),
            CacheState::Failed
        ));
        assert_same_results(&fallback, &sql_results(&index, "coffee budget", None, 10));
        assert!(!fallback.is_empty());

        drop(index);
        cleanup(root);
    }

    #[test]
    fn a_retired_index_writes_nothing_behind_its_replacement() {
        let root = scratch("cache-retired");
        let paths = ranking_vault(&root);
        let database = root.join("semantic.sqlite3");
        let old = Arc::new(SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap());
        old.note_changed(&paths[0]).unwrap();
        old.retry_pending().unwrap();

        old.retire();
        let replacement =
            Arc::new(SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap());
        replacement.search("coffee", None, 5).unwrap(); // its cache snapshot
                                                        // The old index's late writes are dropped: nothing lands behind that snapshot.
        old.note_changed(&paths[1]).unwrap();
        old.note_removed(&paths[0]).unwrap();
        old.rebuild_from_notes(&root).unwrap();
        assert!(old
            .embed_pending_text(&PreparedNote::from_text(
                &paths[1],
                &std::fs::read_to_string(&paths[1]).unwrap()
            ))
            .unwrap());
        assert_eq!(cached_snapshot(&replacement), sqlite_snapshot(&replacement));
        assert_eq!(replacement.status().unwrap().indexed_notes, 1);
        assert_eq!(replacement.status().unwrap().queued_notes, 0);

        drop(old);
        drop(replacement);
        cleanup(root);
    }

    #[test]
    fn an_obsolete_revision_commits_nothing_and_the_newer_one_stays_queued() {
        let root = scratch("cache-stale-revision");
        let paths = ranking_vault(&root);
        let index =
            SemanticIndex::open_at(&root.join("semantic.sqlite3"), Arc::new(WordAxesBackend))
                .unwrap();
        for path in &paths {
            index.note_changed(path).unwrap();
        }
        index.retry_pending().unwrap();
        index.search("coffee", None, 5).unwrap();

        // The revision a slow worker read, then a newer one queued while it was embedding.
        write_note(&paths[3], "d", "Long note", "Resources", "river");
        let old_revision =
            PreparedNote::from_text(&paths[3], &std::fs::read_to_string(&paths[3]).unwrap());
        index.note_changed(&paths[3]).unwrap();
        write_note(&paths[3], "d", "Long note", "Resources", "river river jazz");
        let newer =
            PreparedNote::from_text(&paths[3], &std::fs::read_to_string(&paths[3]).unwrap());
        index.note_changed(&paths[3]).unwrap();
        let committed = sqlite_snapshot(&index);
        let cached = cached_snapshot(&index);

        assert!(index.embed_pending_text(&old_revision).unwrap());
        assert_eq!(sqlite_snapshot(&index), committed);
        assert_eq!(cached_snapshot(&index), cached);
        let pending_hash: String = index
            .database
            .lock()
            .unwrap()
            .query_row(
                "SELECT content_hash FROM pending_notes WHERE note_key = 'id:d'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_hash, newer.hash);

        index.retry_pending().unwrap();
        assert_eq!(cached_snapshot(&index), sqlite_snapshot(&index));
        assert_eq!(index.status().unwrap().queued_notes, 0);
        drop(index);
        cleanup(root);
    }

    #[test]
    fn a_load_that_starts_after_the_wait_is_waited_for_not_scanned_around() {
        let root = scratch("cache-load-race");
        let paths = ranking_vault(&root);
        let database = root.join("semantic.sqlite3");
        let seed = SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap();
        for path in &paths {
            seed.note_changed(path).unwrap();
        }
        seed.retry_pending().unwrap();
        let expected = seed.search("coffee budget", None, 10).unwrap();
        drop(seed);

        let index = Arc::new(SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap());
        let (events, observed) = std::sync::mpsc::channel();
        *index.search_events.lock().unwrap() = Some(events);
        // The search passes its wait while nothing is loading, then blocks on the database. A
        // load begins before it gets the lock, so it finds `Loading` there, and has to wait.
        let held = index.database.lock().unwrap();
        let searcher = {
            let index = index.clone();
            std::thread::spawn(move || index.search("coffee budget", None, 10))
        };
        let within = Duration::from_secs(5);
        assert_eq!(observed.recv_timeout(within).unwrap(), "waited");
        *index.cache.state.lock().unwrap() = CacheState::Loading;
        drop(held);
        // Fails, rather than passing by luck, when a search scans around the load.
        assert_eq!(observed.recv_timeout(within).unwrap(), "saw-loading");
        let loaded = super::load_vectors(&index.database.lock().unwrap()).unwrap();
        index.finish_loading(CacheState::Ready(loaded));
        let found = searcher.join().unwrap().unwrap();
        assert_same_results(&found, &expected);
        assert_eq!(index.sql_scans.load(Ordering::SeqCst), 0);
        drop(index);
        cleanup(root);
    }

    #[test]
    fn switching_away_and_back_retires_the_first_instance() {
        let root = scratch("cache-switch-back");
        let paths = ranking_vault(&root);
        let other = scratch("cache-switch-other");
        let database = root.join("semantic.sqlite3");
        let mut slot = None;

        let first = Arc::new(SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap());
        first.note_changed(&paths[0]).unwrap();
        SemanticIndex::replace_in(&mut slot, first.clone());
        let elsewhere = Arc::new(
            SemanticIndex::open_at(&other.join("semantic.sqlite3"), Arc::new(WordAxesBackend))
                .unwrap(),
        );
        SemanticIndex::replace_in(&mut slot, elsewhere.clone());
        let back = Arc::new(SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap());
        SemanticIndex::replace_in(&mut slot, back.clone());
        back.search("coffee", None, 5).unwrap();
        let snapshot = cached_snapshot(&back);

        // The first instance's worker finishes its embedding late: it must commit nothing.
        let late = PreparedNote::from_text(&paths[0], &std::fs::read_to_string(&paths[0]).unwrap());
        assert!(first.embed_pending_text(&late).unwrap());
        assert_eq!(cached_snapshot(&back), snapshot);
        assert_eq!(cached_snapshot(&back), sqlite_snapshot(&back));
        assert!(first.retired.load(Ordering::SeqCst) && elsewhere.retired.load(Ordering::SeqCst));
        assert!(!back.retired.load(Ordering::SeqCst));

        drop((first, elsewhere, back, slot));
        cleanup(root);
        cleanup(other);
    }

    /// Embeds like WordAxesBackend, but holds a query until the test lets it go.
    struct PausedQueryBackend {
        started: Mutex<Sender<()>>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl EmbeddingBackend for PausedQueryBackend {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            if inputs
                .iter()
                .any(|input| input.starts_with(super::QUERY_PROMPT))
            {
                let _ = self.started.lock().unwrap().send(());
                self.release.lock().unwrap().recv().unwrap();
            }
            WordAxesBackend.embed(inputs)
        }
    }

    #[test]
    fn a_search_that_outlives_its_index_reports_a_restart_instead_of_stale_results() {
        let root = scratch("cache-retired-search");
        let paths = ranking_vault(&root);
        let database = root.join("semantic.sqlite3");
        let seed = SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap();
        for path in &paths {
            seed.note_changed(path).unwrap();
        }
        seed.retry_pending().unwrap();
        drop(seed);

        let (started, query_started) = mpsc::channel();
        let (release, held_query) = mpsc::channel();
        let old = Arc::new(
            SemanticIndex::open_at(
                &database,
                Arc::new(PausedQueryBackend {
                    started: Mutex::new(started),
                    release: Mutex::new(held_query),
                }),
            )
            .unwrap(),
        );
        let mut slot = None;
        SemanticIndex::replace_in(&mut slot, old.clone());
        let searcher = {
            let old = old.clone();
            std::thread::spawn(move || old.search("coffee budget", None, 10))
        };
        query_started.recv_timeout(Duration::from_secs(5)).unwrap();

        // Settings change while the query embeds; the new index then changes the database.
        let current =
            Arc::new(SemanticIndex::open_at(&database, Arc::new(WordAxesBackend)).unwrap());
        SemanticIndex::replace_in(&mut slot, current.clone());
        current.note_removed(&paths[0]).unwrap();
        release.send(()).unwrap();

        let error = searcher.join().unwrap().unwrap_err();
        assert!(error.contains("restarted"), "{error}");
        assert!(current
            .search("coffee budget", None, 10)
            .unwrap()
            .iter()
            .all(|result| result.title != "Alpha tie"));

        drop((old, current, slot));
        cleanup(root);
    }
}
