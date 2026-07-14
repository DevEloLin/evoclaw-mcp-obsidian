//! Tantivy BM25 index: schema, build, and mtime-based incremental refresh.
//!
//! Freshness is tracked with a small sidecar `manifest.json` (rel_path -> mtime)
//! next to the index, so refresh never has to enumerate stored documents.

pub mod search;

use std::collections::HashMap;

use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, FAST, STORED, STRING,
};
use tantivy::tokenizer::{LowerCaser, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument, Term};

use crate::config::Config;
use crate::error::Result;
use crate::vault::{self, Note, ScannedFile};

const WRITER_HEAP_BYTES: usize = 50_000_000;

/// Tokenizer name for full-text fields: jieba (Chinese word segmentation)
/// plus lowercasing so English matches case-insensitively.
const TOKENIZER: &str = "jieba";

/// Handles to every schema field, resolved once at open time.
#[derive(Clone, Copy)]
pub struct Fields {
    pub path: Field,
    pub title: Field,
    pub body: Field,
    pub tags: Field,
    pub headings: Field,
    pub mtime: Field,
}

pub struct KbIndex {
    pub index: Index,
    pub fields: Fields,
    pub reader: IndexReader,
    manifest_path: std::path::PathBuf,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RefreshStats {
    pub upserted: usize,
    pub removed: usize,
}

fn build_schema() -> (Schema, Fields) {
    // Full-text fields tokenized with jieba, keeping frequencies + positions
    // (positions are needed for phrase queries and snippet highlighting).
    let indexing = TextFieldIndexing::default()
        .set_tokenizer(TOKENIZER)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let text_stored = TextOptions::default()
        .set_indexing_options(indexing.clone())
        .set_stored();
    let text_only = TextOptions::default().set_indexing_options(indexing);

    let mut b = Schema::builder();
    // STRING = untokenized: exact term for delete + tag filtering; STORED for retrieval.
    let path = b.add_text_field("path", STRING | STORED);
    let title = b.add_text_field("title", text_stored.clone());
    let body = b.add_text_field("body", text_stored); // STORED so snippets can be generated
    let tags = b.add_text_field("tags", STRING | STORED);
    let headings = b.add_text_field("headings", text_only);
    let mtime = b.add_u64_field("mtime", STORED | FAST);
    let schema = b.build();
    (
        schema,
        Fields {
            path,
            title,
            body,
            tags,
            headings,
            mtime,
        },
    )
}

/// Register the jieba-based analyzer on an index so both indexing and query
/// parsing segment Chinese text (and lowercase ASCII).
fn register_tokenizers(index: &Index) {
    let analyzer = TextAnalyzer::builder(tantivy_jieba::JiebaTokenizer::new())
        .filter(LowerCaser)
        .build();
    index.tokenizers().register(TOKENIZER, analyzer);
}

impl KbIndex {
    /// Open (or create) the index under `config.cache_dir` and bring it in sync
    /// with the current vault contents.
    pub fn open_or_build(config: &Config) -> Result<Self> {
        let index_dir = config.cache_dir.join("index");
        std::fs::create_dir_all(&index_dir)?;

        let (schema, fields) = build_schema();
        let dir = tantivy::directory::MmapDirectory::open(&index_dir)?;
        let index = Index::open_or_create(dir, schema)?;
        register_tokenizers(&index);
        let reader = index.reader()?;

        let kb = KbIndex {
            index,
            fields,
            reader,
            manifest_path: config.cache_dir.join("manifest.json"),
        };
        let files = vault::scan(config)?;
        kb.refresh(&files)?;
        Ok(kb)
    }

    /// Bring the index in sync with `files`: (re)index new/changed notes and
    /// drop notes that disappeared, based on the mtime manifest.
    pub fn refresh(&self, files: &[ScannedFile]) -> Result<RefreshStats> {
        let manifest = self.load_manifest()?;
        let current: HashMap<String, u64> =
            files.iter().map(|f| (f.rel_path.clone(), f.mtime)).collect();

        let upserts: Vec<&ScannedFile> = files
            .iter()
            .filter(|f| manifest.get(&f.rel_path) != Some(&f.mtime))
            .collect();
        let removed: Vec<String> = manifest
            .keys()
            .filter(|p| !current.contains_key(*p))
            .cloned()
            .collect();

        if upserts.is_empty() && removed.is_empty() {
            return Ok(RefreshStats::default());
        }

        let mut writer: IndexWriter = self.index.writer(WRITER_HEAP_BYTES)?;
        for path in &removed {
            writer.delete_term(Term::from_field_text(self.fields.path, path));
        }
        for f in &upserts {
            // Delete-then-add makes upsert idempotent (path is the unique key).
            writer.delete_term(Term::from_field_text(self.fields.path, &f.rel_path));
            let note = vault::load_note(f)?;
            writer.add_document(self.note_doc(&note))?;
        }
        writer.commit()?;
        self.reader.reload()?;
        self.save_manifest(&current)?;

        Ok(RefreshStats {
            upserted: upserts.len(),
            removed: removed.len(),
        })
    }

    fn note_doc(&self, note: &Note) -> TantivyDocument {
        let mut d = TantivyDocument::default();
        d.add_text(self.fields.path, &note.rel_path);
        d.add_text(self.fields.title, &note.title);
        d.add_text(self.fields.body, &note.body);
        for t in &note.tags {
            d.add_text(self.fields.tags, t);
        }
        if !note.headings.is_empty() {
            d.add_text(self.fields.headings, note.headings.join(" "));
        }
        d.add_u64(self.fields.mtime, note.mtime);
        d
    }

    fn load_manifest(&self) -> Result<HashMap<String, u64>> {
        match std::fs::read_to_string(&self.manifest_path) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn save_manifest(&self, manifest: &HashMap<String, u64>) -> Result<()> {
        std::fs::write(&self.manifest_path, serde_json::to_vec(manifest)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn cfg(vault: PathBuf, cache: PathBuf) -> Config {
        Config {
            vault,
            cache_dir: cache,
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
            max_results: 8,
        }
    }

    #[test]
    fn refresh_is_incremental_by_mtime() {
        let vault = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        fs::write(vault.path().join("a.md"), "# A\nalpha content\n").unwrap();
        let config = cfg(vault.path().to_path_buf(), cache.path().to_path_buf());

        let kb = KbIndex::open_or_build(&config).unwrap();

        // Second refresh with unchanged files does nothing.
        let files = vault::scan(&config).unwrap();
        let stats = kb.refresh(&files).unwrap();
        assert_eq!(stats.upserted, 0);
        assert_eq!(stats.removed, 0);

        // Add a file with a newer mtime -> one upsert.
        let b = vault.path().join("b.md");
        fs::write(&b, "# B\nbeta content\n").unwrap();
        let mut files = vault::scan(&config).unwrap();
        for f in &mut files {
            if f.rel_path == "b.md" {
                f.mtime += 1000;
            }
        }
        let stats = kb.refresh(&files).unwrap();
        assert_eq!(stats.upserted, 1);

        // Remove a file -> one removal.
        fs::remove_file(&b).unwrap();
        let files = vault::scan(&config).unwrap();
        let stats = kb.refresh(&files).unwrap();
        assert_eq!(stats.removed, 1);
    }
}
