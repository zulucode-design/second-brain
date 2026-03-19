use crate::types::SearchResult;
use crate::vault::frontmatter;
use crate::vault::operations::helixnotes_dir;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tantivy::collector::TopDocs;
#[cfg(not(target_os = "android"))]
use tantivy::directory::MmapDirectory;
#[cfg(target_os = "android")]
use tantivy::directory::RamDirectory;
use tantivy::query::{BooleanQuery, FuzzyTermQuery, Occur, PhrasePrefixQuery, Query};
use tantivy::schema::*;
use tantivy::{Index, IndexWriter, TantivyDocument, Term};
use walkdir::WalkDir;

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

impl SearchIndex {
    pub fn new(vault_path: &str) -> Result<Self, String> {
        let mut schema_builder = Schema::builder();
        let path_field = schema_builder.add_text_field("path", STRING | STORED);
        let title_field = schema_builder.add_text_field("title", TEXT | STORED);
        let body_field = schema_builder.add_text_field("body", TEXT);
        let tags_field = schema_builder.add_text_field("tags", TEXT | STORED);
        let schema = schema_builder.build();

        // Android: use in-memory index (flock doesn't work on /storage/emulated FUSE filesystem)
        // Desktop: use mmap directory for persistent index on disk
        #[cfg(target_os = "android")]
        let index = {
            let dir = RamDirectory::create();
            Index::open_or_create(dir, schema.clone()).map_err(|e| e.to_string())?
        };
        #[cfg(not(target_os = "android"))]
        let index = {
            let index_dir = helixnotes_dir(vault_path).join("search_index");
            fs::create_dir_all(&index_dir).map_err(|e| e.to_string())?;
            let dir = MmapDirectory::open(&index_dir).map_err(|e| e.to_string())?;
            Index::open_or_create(dir, schema.clone()).map_err(|e| e.to_string())?
        };

        #[cfg(target_os = "android")]
        let heap_size = 15_000_000;
        #[cfg(not(target_os = "android"))]
        let heap_size = 50_000_000;

        let writer = index.writer(heap_size).map_err(|e| e.to_string())?;

        Ok(Self {
            index,
            writer: Mutex::new(Some(writer)),
            schema,
            path_field,
            title_field,
            body_field,
            tags_field,
        })
    }

    pub fn rebuild(&self, vault_path: &str) -> Result<(), String> {
        let mut writer_guard = self.writer.lock().map_err(|e| e.to_string())?;
        let writer = writer_guard.as_mut().ok_or("Writer not available")?;

        writer.delete_all_documents().map_err(|e| e.to_string())?;

        let hn_dir = helixnotes_dir(vault_path);

        for entry in WalkDir::new(vault_path)
            .into_iter()
            .filter_entry(|e| {
                let p = e.path();
                !p.starts_with(&hn_dir)
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with('.'))
                        .unwrap_or(false)
            })
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
                doc.add_text(self.tags_field, &meta.tags.join(" "));
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
        doc.add_text(self.tags_field, &meta.tags.join(" "));
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

    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let reader = self.index.reader().map_err(|e| e.to_string())?;
        let searcher = reader.searcher();

        let fields = vec![self.title_field, self.body_field, self.tags_field];
        let terms: Vec<String> = query_str
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .collect();

        // For each search term, create prefix + fuzzy queries across all fields (OR),
        // then AND all terms together
        let term_queries: Vec<(Occur, Box<dyn Query>)> = terms
            .iter()
            .map(|term| {
                let field_queries: Vec<(Occur, Box<dyn Query>)> = fields
                    .iter()
                    .flat_map(|&field| {
                        let prefix: Box<dyn Query> = Box::new(PhrasePrefixQuery::new(
                            vec![Term::from_field_text(field, term)],
                        ));
                        let fuzzy: Box<dyn Query> = Box::new(FuzzyTermQuery::new(
                            Term::from_field_text(field, term),
                            1,
                            true,
                        ));
                        vec![(Occur::Should, prefix), (Occur::Should, fuzzy)]
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
