//! Vault scanning and the `Note` model.
//!
//! [`scan`] walks the vault honoring include/exclude globs; [`load_note`]
//! reads and parses a single file. Structure extraction lives in [`parse`].

pub mod parse;
pub mod read;

use std::path::PathBuf;

use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::config::Config;
use crate::error::Result;

/// A parsed note ready for indexing.
#[derive(Debug, Clone)]
pub struct Note {
    /// Vault-relative path, forward-slash separated (e.g. `folder/note.md`).
    pub rel_path: String,
    pub title: String,
    /// Whitespace-collapsed plain text (markdown + frontmatter stripped).
    pub body: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub headings: Vec<String>,
    /// Last-modified time, seconds since the Unix epoch.
    pub mtime: u64,
}

/// A file discovered during a scan, before parsing.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub mtime: u64,
}

/// Build a [`GlobSet`] from vault-relative patterns.
pub fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        builder.add(Glob::new(p)?);
    }
    Ok(builder.build()?)
}

/// Walk the vault and return matching files, sorted by relative path.
/// Symlinks are not followed (path confinement + loop safety).
pub fn scan(config: &Config) -> Result<Vec<ScannedFile>> {
    let include = build_globset(&config.include)?;
    let exclude = if config.exclude.is_empty() {
        None
    } else {
        Some(build_globset(&config.exclude)?)
    };

    let mut out = Vec::new();
    for entry in WalkDir::new(&config.vault)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(&config.vault) else {
            continue;
        };
        let rel_path = rel.to_string_lossy().replace('\\', "/");

        if !include.is_match(&rel_path) {
            continue;
        }
        if exclude.as_ref().is_some_and(|ex| ex.is_match(&rel_path)) {
            continue;
        }

        out.push(ScannedFile {
            rel_path,
            abs_path: entry.path().to_path_buf(),
            mtime: file_mtime(entry.path()),
        });
    }

    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// Read and parse one scanned file.
pub fn load_note(file: &ScannedFile) -> Result<Note> {
    let content = std::fs::read_to_string(&file.abs_path)?;
    Ok(parse::parse_note(&file.rel_path, &content, file.mtime))
}

fn file_mtime(path: &std::path::Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn cfg(vault: PathBuf, exclude: Vec<&str>) -> Config {
        Config {
            vault,
            cache_dir: PathBuf::from("/tmp/unused"),
            include: vec!["**/*.md".to_string()],
            exclude: exclude.into_iter().map(String::from).collect(),
            max_results: 8,
        }
    }

    #[test]
    fn scan_finds_root_and_nested_md_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.md"), "root note").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/b.md"), "nested note").unwrap();
        fs::write(root.join("ignore.txt"), "not markdown").unwrap();

        let files = scan(&cfg(root.to_path_buf(), vec![])).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["a.md", "sub/b.md"]);
    }

    #[test]
    fn scan_respects_exclude_glob() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("keep.md"), "x").unwrap();
        fs::create_dir_all(root.join("private")).unwrap();
        fs::write(root.join("private/secret.md"), "x").unwrap();

        let files = scan(&cfg(root.to_path_buf(), vec!["private/**"])).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["keep.md"]);
    }

    #[test]
    fn load_note_reads_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("n.md"), "# Title\nhello world #tag\n").unwrap();

        let files = scan(&cfg(root.to_path_buf(), vec![])).unwrap();
        let note = load_note(&files[0]).unwrap();
        assert_eq!(note.title, "Title");
        assert!(note.body.contains("hello world"));
        assert!(note.tags.contains(&"tag".to_string()));
    }
}
