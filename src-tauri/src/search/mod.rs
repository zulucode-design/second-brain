pub mod external;

use crate::types::SearchResult;
use crate::vault::frontmatter;
use crate::vault::operations::helixnotes_dir;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tantivy::collector::TopDocs;
#[cfg(desktop)]
use tantivy::directory::MmapDirectory;
#[cfg(any(mobile, test))]
use tantivy::directory::RamDirectory;
use tantivy::query::{BooleanQuery, FuzzyTermQuery, Occur, PhrasePrefixQuery, Query, TermQuery};
use tantivy::schema::*;
use tantivy::tokenizer::{LowerCaser, TextAnalyzer, Token, TokenStream, Tokenizer};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};
use walkdir::WalkDir;

/// Bumped whenever the index schema or tokenizer changes, so the on-disk index is
/// wiped and rebuilt once on the next vault open (the index is derived from the
/// notes, so this never loses data).
const INDEX_SCHEMA_VERSION: &str = "2-cjk-bigram";

/// The pre-vault-id index location: keyed by a hash of the vault's path, so it was
/// orphaned whenever the vault folder moved. Only used to clean up the stale copy.
#[cfg(desktop)]
fn legacy_path_keyed_index(vault_path: &str) -> Option<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(vault_path.as_bytes());
    let key: String = hasher.finalize()[..8]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    dirs::data_local_dir().map(|d| d.join("helixnotes").join("search").join(key))
}

/// True for characters in the CJK / Japanese / Korean blocks, which are written
/// without spaces between words. These get uni/bi-gram tokenized so substring
/// search works; everything else keeps the default word-splitting behaviour.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF   // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs (Han)
        | 0x3040..=0x309F // Hiragana
        | 0x30A0..=0x30FF // Katakana
        | 0xAC00..=0xD7AF // Hangul Syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFF00..=0xFFEF // Halfwidth and Fullwidth Forms
    )
}

/// Tokenize `text` so that CJK runs become overlapping unigrams + bigrams (recall
/// over precision: a query for any substring of a CJK run will match), while runs
/// of other alphanumeric characters become a single word token (same as tantivy's
/// SimpleTokenizer, so Latin/English indexing is unchanged). Lowercasing is applied
/// by a LowerCaser filter in the analyzer, not here.
fn cjk_tokens(text: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut position: usize = 0;
    let mut cjk_run: Vec<(usize, char)> = Vec::new();
    let mut word = String::new();
    let mut word_start: usize = 0;

    macro_rules! flush_word {
        () => {{
            if !word.is_empty() {
                let len = word.len();
                tokens.push(Token {
                    offset_from: word_start,
                    offset_to: word_start + len,
                    position,
                    text: std::mem::take(&mut word),
                    position_length: 1,
                });
                position += 1;
            }
        }};
    }
    macro_rules! flush_cjk {
        () => {{
            let n = cjk_run.len();
            for i in 0..n {
                let (off, ch) = cjk_run[i];
                tokens.push(Token {
                    offset_from: off,
                    offset_to: off + ch.len_utf8(),
                    position,
                    text: ch.to_string(),
                    position_length: 1,
                });
                position += 1;
            }
            for i in 0..n.saturating_sub(1) {
                let (off1, c1) = cjk_run[i];
                let (off2, c2) = cjk_run[i + 1];
                let mut s = String::with_capacity(c1.len_utf8() + c2.len_utf8());
                s.push(c1);
                s.push(c2);
                tokens.push(Token {
                    offset_from: off1,
                    offset_to: off2 + c2.len_utf8(),
                    position,
                    text: s,
                    position_length: 1,
                });
                position += 1;
            }
            cjk_run.clear();
        }};
    }

    for (offset, c) in text.char_indices() {
        if is_cjk(c) {
            flush_word!();
            cjk_run.push((offset, c));
        } else if c.is_alphanumeric() {
            flush_cjk!();
            if word.is_empty() {
                word_start = offset;
            }
            word.push(c);
        } else {
            flush_cjk!();
            flush_word!();
        }
    }
    flush_cjk!();
    flush_word!();
    let _ = position; // the final flush increments position but nothing reads it after
    tokens
}

/// A pre-computed token stream (all tokens collected up front by `cjk_tokens`).
struct PreTokenizedStream {
    tokens: Vec<Token>,
    idx: usize,
}

impl TokenStream for PreTokenizedStream {
    fn advance(&mut self) -> bool {
        if self.idx < self.tokens.len() {
            self.idx += 1;
            true
        } else {
            false
        }
    }
    fn token(&self) -> &Token {
        &self.tokens[self.idx - 1]
    }
    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.idx - 1]
    }
}

#[derive(Clone)]
struct CjkTokenizer;

impl Tokenizer for CjkTokenizer {
    type TokenStream<'a> = PreTokenizedStream;
    fn token_stream<'a>(&'a mut self, text: &'a str) -> PreTokenizedStream {
        PreTokenizedStream {
            tokens: cjk_tokens(text),
            idx: 0,
        }
    }
}

pub struct SearchIndex {
    index: Index,
    writer: Mutex<Option<IndexWriter>>,
    #[allow(dead_code)]
    schema: Schema,
    path_field: Field,
    title_field: Field,
    body_field: Field,
    tags_field: Field,
}

struct SearchSchema {
    schema: Schema,
    path_field: Field,
    title_field: Field,
    body_field: Field,
    tags_field: Field,
}

/// Whether `path` sits in a part of the vault the index ignores.
///
/// Hidden entries are not notes: `.helixnotes` is machine-local state, and a dot-prefixed
/// name is a scratch file, an editor lock, or a folder the user hid on purpose. Both the
/// full rebuild and the watcher consult this, because if they disagreed a note indexed
/// live would silently vanish at the next open — the index would depend on which route
/// last touched it.
pub(crate) fn is_ignored_by_index(path: &Path, vault_path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(vault_path) else {
        // Outside the vault entirely; nothing in the index can describe it.
        return true;
    };
    relative.components().any(|component| {
        matches!(component, std::path::Component::Normal(name)
            if name.to_string_lossy().starts_with('.'))
    })
}

fn build_search_schema() -> SearchSchema {
    let mut schema_builder = Schema::builder();
    let path_field = schema_builder.add_text_field("path", STRING | STORED);
    let cjk_indexing = TextFieldIndexing::default()
        .set_tokenizer("cjk")
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let cjk_text = TextOptions::default().set_indexing_options(cjk_indexing.clone());
    let cjk_text_stored = cjk_text.clone().set_stored();
    let title_field = schema_builder.add_text_field("title", cjk_text_stored.clone());
    let body_field = schema_builder.add_text_field("body", cjk_text);
    let tags_field = schema_builder.add_text_field("tags", cjk_text_stored);

    SearchSchema {
        schema: schema_builder.build(),
        path_field,
        title_field,
        body_field,
        tags_field,
    }
}

impl SearchIndex {
    pub fn new(vault_path: &str) -> Result<Self, String> {
        let fields = build_search_schema();
        let schema = fields.schema.clone();

        // Mobile: use in-memory index (flock is unreliable on the sandboxed/FUSE filesystem)
        // Desktop: use mmap directory for persistent index on disk
        #[cfg(mobile)]
        let index = {
            let dir = RamDirectory::create();
            Index::open_or_create(dir, schema.clone()).map_err(|e| e.to_string())?
        };
        #[cfg(desktop)]
        let index = {
            // No in-vault fallback: an index inside a synced vault is the situation this
            // whole layout exists to prevent, so a machine with nowhere to put it fails
            // loudly instead.
            let base = crate::machine_local::search_dir(std::path::Path::new(vault_path))?;
            let index_dir = base.join("index");
            let version_path = base.join("version");

            // Drop indexes left at the two earlier locations. The index is derived from
            // the notes, so discarding it costs one rebuild and nothing else.
            let hn = helixnotes_dir(vault_path);
            let _ = fs::remove_dir_all(hn.join("search_index"));
            let _ = fs::remove_file(hn.join("search_index.version"));
            if let Some(legacy) = legacy_path_keyed_index(vault_path) {
                let _ = fs::remove_dir_all(legacy);
            }

            // One-time wipe when the schema/tokenizer version changes; rebuild() repopulates.
            let version_ok = fs::read_to_string(&version_path)
                .map(|v| v.trim() == INDEX_SCHEMA_VERSION)
                .unwrap_or(false);
            if !version_ok {
                let _ = fs::remove_dir_all(&index_dir);
            }
            fs::create_dir_all(&index_dir).map_err(|e| e.to_string())?;
            let dir = MmapDirectory::open(&index_dir).map_err(|e| e.to_string())?;
            let idx = match Index::open_or_create(dir, schema.clone()) {
                Ok(idx) => idx,
                Err(_) => {
                    // Schema mismatch (older index) or corruption: wipe and recreate.
                    let _ = fs::remove_dir_all(&index_dir);
                    fs::create_dir_all(&index_dir).map_err(|e| e.to_string())?;
                    let dir = MmapDirectory::open(&index_dir).map_err(|e| e.to_string())?;
                    Index::open_or_create(dir, schema.clone()).map_err(|e| e.to_string())?
                }
            };
            let _ = fs::write(&version_path, INDEX_SCHEMA_VERSION);
            idx
        };

        Self::from_index(index, fields)
    }

    fn from_index(index: Index, fields: SearchSchema) -> Result<Self, String> {
        index.tokenizers().register(
            "cjk",
            TextAnalyzer::builder(CjkTokenizer)
                .filter(LowerCaser)
                .build(),
        );

        #[cfg(mobile)]
        let heap_size = 15_000_000;
        #[cfg(desktop)]
        let heap_size = 50_000_000;

        let writer = index.writer(heap_size).map_err(|e| e.to_string())?;

        Ok(Self {
            index,
            writer: Mutex::new(Some(writer)),
            schema: fields.schema,
            path_field: fields.path_field,
            title_field: fields.title_field,
            body_field: fields.body_field,
            tags_field: fields.tags_field,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_in_memory() -> Result<Self, String> {
        let fields = build_search_schema();
        let index = Index::open_or_create(RamDirectory::create(), fields.schema.clone())
            .map_err(|error| error.to_string())?;
        Self::from_index(index, fields)
    }

    pub fn rebuild(&self, vault_path: &str) -> Result<(), String> {
        let mut writer_guard = self.writer.lock().map_err(|e| e.to_string())?;
        let writer = writer_guard.as_mut().ok_or("Writer not available")?;

        writer.delete_all_documents().map_err(|e| e.to_string())?;

        let vault_root = Path::new(vault_path);

        for entry in WalkDir::new(vault_path)
            .into_iter()
            .filter_entry(|e| !is_ignored_by_index(e.path(), vault_root))
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().is_file()
                    && e.path().extension().and_then(|x| x.to_str()) == Some("md")
            })
        {
            if let Ok(raw) = fs::read_to_string(entry.path()) {
                let filename = entry.file_name().to_string_lossy().to_string();
                let (meta, content) = frontmatter::parse_note(&raw, &filename);
                let path_str = entry.path().to_string_lossy().to_string();

                let mut doc = TantivyDocument::new();
                doc.add_text(self.path_field, &path_str);
                doc.add_text(self.title_field, &meta.title);
                doc.add_text(self.body_field, &content);
                doc.add_text(self.tags_field, meta.tags.join(" "));
                let _ = writer.add_document(doc);
            }
        }

        writer.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn index_note(&self, path: &str) -> Result<(), String> {
        let p = Path::new(path);
        let raw = fs::read_to_string(p).map_err(|e| e.to_string())?;
        let filename = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let (meta, content) = frontmatter::parse_note(&raw, &filename);

        let mut writer_guard = self.writer.lock().map_err(|e| e.to_string())?;
        let writer = writer_guard.as_mut().ok_or("Writer not available")?;

        // Delete old entry
        let term = tantivy::Term::from_field_text(self.path_field, path);
        writer.delete_term(term);

        // Add updated
        let mut doc = TantivyDocument::new();
        doc.add_text(self.path_field, path);
        doc.add_text(self.title_field, &meta.title);
        doc.add_text(self.body_field, &content);
        doc.add_text(self.tags_field, meta.tags.join(" "));
        let _ = writer.add_document(doc);

        writer.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn remove_note(&self, path: &str) -> Result<(), String> {
        let mut writer_guard = self.writer.lock().map_err(|e| e.to_string())?;
        let writer = writer_guard.as_mut().ok_or("Writer not available")?;
        let term = tantivy::Term::from_field_text(self.path_field, path);
        writer.delete_term(term);
        writer.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Apply a complete path mutation in one Tantivy commit.
    ///
    /// Upserts are read before the writer is touched, so a filesystem/read error leaves
    /// the existing index unchanged. Each upsert path is deleted before its replacement
    /// document is added, and all removals/additions become visible together.
    pub fn apply_note_changes(
        &self,
        remove_paths: &[String],
        upsert_paths: &[String],
    ) -> Result<(), String> {
        let mut documents = Vec::with_capacity(upsert_paths.len());
        for path in upsert_paths {
            let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
            let filename = Path::new(path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let (meta, content) = frontmatter::parse_note(&raw, &filename);
            let mut document = TantivyDocument::new();
            document.add_text(self.path_field, path);
            document.add_text(self.title_field, &meta.title);
            document.add_text(self.body_field, &content);
            document.add_text(self.tags_field, meta.tags.join(" "));
            documents.push((path, document));
        }

        let mut writer_guard = self.writer.lock().map_err(|error| error.to_string())?;
        let writer = writer_guard.as_mut().ok_or("Writer not available")?;
        for path in remove_paths.iter().chain(upsert_paths) {
            writer.delete_term(Term::from_field_text(self.path_field, path));
        }
        for (_, document) in documents {
            writer
                .add_document(document)
                .map_err(|error| error.to_string())?;
        }
        writer.commit().map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Every indexed path that sits inside any of `directories`.
    ///
    /// `path` is a `STRING` field, so there is no prefix term to delete by and the stored
    /// values are scanned instead. Every directory in a batch is matched in that single
    /// pass: a sync burst can delete thousands of paths at once, and scanning the index
    /// once per path would turn one flush into thousands of full scans.
    pub fn indexed_paths_under_any(&self, directories: &[String]) -> Result<Vec<String>, String> {
        if directories.is_empty() {
            return Ok(Vec::new());
        }
        let prefixes: Vec<String> = directories
            .iter()
            .map(|directory| {
                let mut prefix = directory.clone();
                if !prefix.ends_with(std::path::MAIN_SEPARATOR) {
                    prefix.push(std::path::MAIN_SEPARATOR);
                }
                prefix
            })
            .collect();
        let reader = self.index.reader().map_err(|error| error.to_string())?;
        let searcher = reader.searcher();
        let mut paths = Vec::new();
        for segment in searcher.segment_readers() {
            // One cached block: this walks each document once in order, so a larger
            // cache would hold blocks that are never read again.
            let store = segment
                .get_store_reader(1)
                .map_err(|error| error.to_string())?;
            for doc_id in segment.doc_ids_alive() {
                let document: TantivyDocument =
                    store.get(doc_id).map_err(|error| error.to_string())?;
                if let Some(value) = document
                    .get_first(self.path_field)
                    .and_then(|value| value.as_str())
                {
                    if prefixes.iter().any(|prefix| value.starts_with(prefix)) {
                        paths.push(value.to_string());
                    }
                }
            }
        }
        Ok(paths)
    }

    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let reader = self.index.reader().map_err(|e| e.to_string())?;
        let searcher = reader.searcher();

        let fields = [self.title_field, self.body_field, self.tags_field];
        // Tokenize the query with the SAME CJK-aware analyzer used for indexing, so a
        // Chinese/Japanese/Korean query becomes the same uni/bigram tokens as the docs.
        // (For pure-ASCII queries this yields the same lowercased word tokens as before.)
        let mut analyzer = self
            .index
            .tokenizers()
            .get("cjk")
            .ok_or("cjk tokenizer not registered")?;
        let terms: Vec<String> = {
            let mut out = Vec::new();
            let mut stream = analyzer.token_stream(query_str);
            while stream.advance() {
                out.push(stream.token().text.clone());
            }
            out
        };

        // For each term, OR queries across all fields, then AND all terms together.
        // CJK tokens (uni/bigrams) already encode segmentation -> exact term match.
        // Latin tokens keep prefix + fuzzy matching (unchanged English behaviour).
        let term_queries: Vec<(Occur, Box<dyn Query>)> = terms
            .iter()
            .map(|term| {
                let is_cjk_term = term.chars().any(is_cjk);
                let field_queries: Vec<(Occur, Box<dyn Query>)> = fields
                    .iter()
                    .flat_map(|&field| {
                        if is_cjk_term {
                            let exact: Box<dyn Query> = Box::new(TermQuery::new(
                                Term::from_field_text(field, term),
                                IndexRecordOption::WithFreqs,
                            ));
                            vec![(Occur::Should, exact)]
                        } else {
                            let prefix: Box<dyn Query> =
                                Box::new(PhrasePrefixQuery::new(vec![Term::from_field_text(
                                    field, term,
                                )]));
                            let fuzzy: Box<dyn Query> = Box::new(FuzzyTermQuery::new(
                                Term::from_field_text(field, term),
                                1,
                                true,
                            ));
                            vec![(Occur::Should, prefix), (Occur::Should, fuzzy)]
                        }
                    })
                    .collect();
                let combined: Box<dyn Query> = Box::new(BooleanQuery::new(field_queries));
                (Occur::Must, combined)
            })
            .collect();

        let query = BooleanQuery::new(term_queries);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit))
            .map_err(|e| e.to_string())?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|e| e.to_string())?;

            let path = doc
                .get_first(self.path_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let title = doc
                .get_first(self.title_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            results.push(SearchResult {
                path,
                title,
                snippet: String::new(),
                score,
            });
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn a_path_change_is_visible_as_one_index_batch() {
        let root = std::env::temp_dir().join(format!("search-move-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let old_path = root.join("Old.md");
        let new_path = root.join("New.md");
        fs::write(&old_path, "---\ntitle: Target\n---\nunique-search-token\n").unwrap();
        let index = SearchIndex::new_in_memory().unwrap();
        index.index_note(&old_path.to_string_lossy()).unwrap();
        fs::rename(&old_path, &new_path).unwrap();

        index
            .apply_note_changes(
                &[old_path.to_string_lossy().to_string()],
                &[new_path.to_string_lossy().to_string()],
            )
            .unwrap();
        let results = index.search("unique-search-token", 10).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, new_path.to_string_lossy());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_upsert_leaves_the_existing_index_unchanged() {
        let root = std::env::temp_dir().join(format!("search-preflight-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let existing = root.join("Existing.md");
        let missing = root.join("Missing.md");
        fs::write(&existing, "---\ntitle: Existing\n---\nstable-index-token\n").unwrap();
        let index = SearchIndex::new_in_memory().unwrap();
        index.index_note(&existing.to_string_lossy()).unwrap();

        let result = index.apply_note_changes(
            &[existing.to_string_lossy().to_string()],
            &[missing.to_string_lossy().to_string()],
        );

        assert!(result.is_err());
        let results = index.search("stable-index-token", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, existing.to_string_lossy());
        fs::remove_dir_all(root).unwrap();
    }
}
