//! BM25 search over the index: field-boosted query parsing, optional tag /
//! path-prefix filters, and snippet generation.

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::snippet::SnippetGenerator;
use tantivy::{TantivyDocument, Term};

use crate::error::Result;
use crate::index::KbIndex;

const TITLE_BOOST: f32 = 2.0;
const HEADINGS_BOOST: f32 = 1.5;
const SNIPPET_MAX_CHARS: usize = 200;
const PREFIX_OVERFETCH: usize = 5;
const PREFIX_OVERFETCH_CAP: usize = 200;

/// One search result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub path: String,
    pub title: String,
    pub score: f32,
    pub snippet: String,
    pub tags: Vec<String>,
}

/// Run a BM25 query. `tags` filters to notes carrying ANY listed tag;
/// `path_prefix` keeps only notes whose vault-relative path starts with it.
pub fn search(
    kb: &KbIndex,
    query: &str,
    limit: usize,
    tags: Option<&[String]>,
    path_prefix: Option<&str>,
) -> Result<Vec<SearchHit>> {
    let searcher = kb.reader.searcher();
    let f = &kb.fields;
    let limit = limit.max(1);

    let mut parser = QueryParser::for_index(&kb.index, vec![f.title, f.body, f.headings]);
    parser.set_field_boost(f.title, TITLE_BOOST);
    parser.set_field_boost(f.headings, HEADINGS_BOOST);
    // Lenient parsing tolerates user queries with stray special characters.
    let (text_query, _errs) = parser.parse_query_lenient(query);

    let final_query: Box<dyn Query> = match tags {
        Some(ts) if !ts.is_empty() => {
            let any_tag: Vec<(Occur, Box<dyn Query>)> = ts
                .iter()
                .map(|t| {
                    let tq: Box<dyn Query> = Box::new(TermQuery::new(
                        Term::from_field_text(f.tags, t),
                        IndexRecordOption::Basic,
                    ));
                    (Occur::Should, tq)
                })
                .collect();
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, text_query),
                (Occur::Must, Box::new(BooleanQuery::new(any_tag)) as Box<dyn Query>),
            ]))
        }
        _ => text_query,
    };

    // Over-fetch when a path prefix filter will discard some hits afterwards.
    let fetch = if path_prefix.is_some() {
        limit.saturating_mul(PREFIX_OVERFETCH).min(PREFIX_OVERFETCH_CAP).max(limit)
    } else {
        limit
    };
    // tantivy 0.26: TopDocs is a builder; `order_by_score` yields the
    // score-ranked collector producing `(Score, DocAddress)` pairs.
    let collector = TopDocs::with_limit(fetch).order_by_score();
    let top = searcher.search(&*final_query, &collector)?;

    let mut snippet_gen = SnippetGenerator::create(&searcher, &*final_query, f.body)?;
    snippet_gen.set_max_num_chars(SNIPPET_MAX_CHARS);

    let mut hits = Vec::with_capacity(limit);
    for (score, addr) in top {
        let doc: TantivyDocument = searcher.doc(addr)?;
        let path = str_field(&doc, f.path);
        if let Some(prefix) = path_prefix {
            if !path.starts_with(prefix) {
                continue;
            }
        }
        let body = str_field(&doc, f.body);
        let snippet = snippet_gen.snippet(&body);
        let snippet_text = if snippet.fragment().is_empty() {
            body.chars().take(SNIPPET_MAX_CHARS).collect()
        } else {
            snippet.fragment().to_string()
        };

        hits.push(SearchHit {
            path,
            title: str_field(&doc, f.title),
            score,
            snippet: snippet_text,
            tags: doc
                .get_all(f.tags)
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

fn str_field(doc: &TantivyDocument, field: tantivy::schema::Field) -> String {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::index::KbIndex;
    use std::fs;
    use std::path::PathBuf;

    fn build_vault(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, Config) {
        let vault = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let path = vault.path().join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, body).unwrap();
        }
        let config = Config {
            vault: vault.path().to_path_buf(),
            cache_dir: cache.path().to_path_buf(),
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
            max_results: 8,
        };
        (vault, cache, config)
    }

    #[test]
    fn ranks_matching_notes_and_generates_snippet() {
        let (_v, _c, config) = build_vault(&[
            ("apple.md", "# Apple\nApples are a sweet red fruit.\n"),
            ("car.md", "# Car\nA car is a road vehicle.\n"),
        ]);
        let kb = KbIndex::open_or_build(&config).unwrap();

        let hits = search(&kb, "sweet fruit", 5, None, None).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "apple.md");
        assert!(hits[0].snippet.to_lowercase().contains("fruit"));
    }

    #[test]
    fn title_match_outranks_body_match() {
        let (_v, _c, config) = build_vault(&[
            ("doc1.md", "# Banana\nunrelated body text here\n"),
            ("doc2.md", "# Unrelated\nthis note merely mentions banana once\n"),
        ]);
        let kb = KbIndex::open_or_build(&config).unwrap();

        let hits = search(&kb, "banana", 5, None, None).unwrap();
        assert_eq!(hits[0].path, "doc1.md", "title hit should rank first");
    }

    #[test]
    fn filters_by_tag_any() {
        let (_v, _c, config) = build_vault(&[
            ("p.md", "# P\nproject notes #work\n"),
            ("h.md", "# H\nproject notes #home\n"),
        ]);
        let kb = KbIndex::open_or_build(&config).unwrap();

        let hits = search(&kb, "project", 5, Some(&["work".to_string()]), None).unwrap();
        let paths: Vec<_> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["p.md"]);
    }

    #[test]
    fn filters_by_path_prefix() {
        let (_v, _c, config) = build_vault(&[
            ("area/note.md", "# A\nshared keyword\n"),
            ("other/note.md", "# B\nshared keyword\n"),
        ]);
        let kb = KbIndex::open_or_build(&config).unwrap();

        let hits = search(&kb, "shared keyword", 5, None, Some("area/")).unwrap();
        let paths: Vec<_> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["area/note.md"]);
    }

    #[test]
    fn searches_chinese_with_word_segmentation() {
        let (_v, _c, config) = build_vault(&[
            (
                "cook.md",
                "---\ntags: [cooking]\n---\n# 番茄炒蛋\n先炒蛋再放番茄，加糖提鲜。\n",
            ),
            ("k8s.md", "# 部署\n用 kubectl 滚动更新服务。\n"),
        ]);
        let kb = KbIndex::open_or_build(&config).unwrap();

        // "提鲜" only matches after Chinese word segmentation; the default
        // tokenizer would glue the whole clause into one token and miss it.
        let hits = search(&kb, "提鲜", 5, None, None).unwrap();
        let paths: Vec<_> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["cook.md"]);

        // Works together with the tag filter.
        let hits = search(&kb, "提鲜", 5, Some(&["cooking".to_string()]), None).unwrap();
        assert_eq!(hits.len(), 1);
    }

    // Silence unused import warning when the tests above don't touch PathBuf directly.
    #[allow(dead_code)]
    fn _uses(_: PathBuf) {}
}
