//! Reading a note by vault-relative path, with strict path confinement.
//!
//! Confinement mirrors EvoClaw's gate philosophy: reject absolute paths and
//! `..` components up front, then canonicalize (resolving symlinks) and assert
//! the real path is still inside the vault root — defeating symlink escape.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// Resolve a vault-relative path to an absolute path proven to be inside the
/// (already-canonicalized) vault root.
pub fn resolve_in_vault(vault: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.trim();
    let rel_path = Path::new(rel);

    if rel_path.is_absolute()
        || rel_path
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(Error::PathEscape(rel.to_string()));
    }

    let canon = vault
        .join(rel_path)
        .canonicalize()
        .map_err(|_| Error::NoteNotFound(rel.to_string()))?;

    if !canon.starts_with(vault) {
        return Err(Error::PathEscape(rel.to_string()));
    }
    Ok(canon)
}

/// Read a note. When `section` is set, return only that heading's slice
/// (from the matching heading down to the next same-or-higher-level heading).
pub fn read_note(vault: &Path, rel: &str, section: Option<&str>) -> Result<String> {
    let path = resolve_in_vault(vault, rel)?;
    let content = std::fs::read_to_string(&path)?;
    match section {
        Some(s) => extract_section(&content, s)
            .ok_or_else(|| Error::NoteNotFound(format!("{rel}#{s}"))),
        None => Ok(content),
    }
}

fn extract_section(content: &str, section: &str) -> Option<String> {
    let want = section.trim().to_lowercase();
    let lines: Vec<&str> = content.lines().collect();

    let (start, level) = lines.iter().enumerate().find_map(|(i, line)| {
        heading(line).and_then(|(lvl, text)| {
            let text = text.to_lowercase();
            (text == want || text.contains(&want)).then_some((i, lvl))
        })
    })?;

    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(j, line)| heading(line).filter(|(lvl, _)| *lvl <= level).map(|_| j))
        .unwrap_or(lines.len());

    Some(lines[start..end].join("\n"))
}

/// Parse an ATX heading line into `(level, text)`. Requires a space after the
/// hashes, so an Obsidian `#tag` is not treated as a heading.
fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && t.as_bytes().get(hashes) == Some(&b' ') {
        Some((hashes, t[hashes..].trim()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rejects_absolute_and_parent_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().canonicalize().unwrap();
        assert!(matches!(
            resolve_in_vault(&vault, "../etc/passwd"),
            Err(Error::PathEscape(_))
        ));
        assert!(matches!(
            resolve_in_vault(&vault, "/etc/passwd"),
            Err(Error::PathEscape(_))
        ));
    }

    #[test]
    fn rejects_symlink_escaping_vault() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.md");
        fs::write(&secret, "top secret").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&secret, vault.join("link.md")).unwrap();
            // The symlink resolves outside the vault -> rejected.
            assert!(matches!(
                resolve_in_vault(&vault, "link.md"),
                Err(Error::PathEscape(_))
            ));
        }
    }

    #[test]
    fn reads_full_note_and_section() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().canonicalize().unwrap();
        let body = "# Top\nintro\n## Alpha\naaa\n## Beta\nbbb\n";
        fs::write(vault.join("n.md"), body).unwrap();

        let full = read_note(&vault, "n.md", None).unwrap();
        assert!(full.contains("intro") && full.contains("bbb"));

        let alpha = read_note(&vault, "n.md", Some("Alpha")).unwrap();
        assert!(alpha.contains("## Alpha") && alpha.contains("aaa"));
        assert!(!alpha.contains("bbb"), "section must stop at next heading");

        assert!(matches!(
            read_note(&vault, "n.md", Some("Missing")),
            Err(Error::NoteNotFound(_))
        ));
    }
}
