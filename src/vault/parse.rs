//! Pure markdown parsing: split frontmatter, extract structure (title, tags,
//! wikilinks, headings) and a plain-text body for indexing.
//!
//! All functions here are pure over `&str` content so they unit-test without
//! touching the filesystem.

use std::sync::LazyLock;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser as MdParser, Tag, TagEnd};
use regex::Regex;

use crate::vault::Note;

/// Obsidian inline tag: `#tag`, `#nested/tag`. Requires a non-`#`, non-space
/// start char so a markdown `# Heading` (hash + space) is NOT captured.
static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)#([A-Za-z0-9_][A-Za-z0-9_/-]*)").unwrap());

/// Obsidian wikilink: `[[Target]]`, `[[Target|alias]]`, `[[Target#section]]`.
/// Captures the target only (before `|` or `#`).
static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]|#]+)").unwrap());

/// Parse raw note `content` into a structured [`Note`].
pub fn parse_note(rel_path: &str, content: &str, mtime: u64) -> Note {
    let (frontmatter, body_md) = split_frontmatter(content);
    let fm = frontmatter
        .as_deref()
        .and_then(|f| serde_yaml::from_str::<serde_yaml::Value>(f).ok());

    let mut tags = frontmatter_tags(&fm);
    for cap in TAG_RE.captures_iter(&body_md) {
        tags.push(cap[1].to_string());
    }
    dedupe(&mut tags);

    let mut links: Vec<String> = WIKILINK_RE
        .captures_iter(&body_md)
        .map(|c| c[1].trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    dedupe(&mut links);

    let (body, headings, first_h1) = render_plain_and_headings(&body_md);

    let title = fm_title(&fm)
        .or(first_h1)
        .unwrap_or_else(|| filename_stem(rel_path));

    Note {
        rel_path: rel_path.to_string(),
        title,
        body,
        tags,
        links,
        headings,
        mtime,
    }
}

/// Split a leading YAML frontmatter block (delimited by `---` lines) from the
/// body. Returns `(frontmatter_yaml, body_markdown)`.
fn split_frontmatter(content: &str) -> (Option<String>, String) {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let after_open = if let Some(r) = content.strip_prefix("---\n") {
        r
    } else if let Some(r) = content.strip_prefix("---\r\n") {
        r
    } else {
        return (None, content.to_string());
    };

    let mut idx = 0usize;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let fm = after_open[..idx].to_string();
            let body = after_open.get(idx + line.len()..).unwrap_or("").to_string();
            return (Some(fm), body);
        }
        idx += line.len();
    }
    // Opening fence with no close — treat the whole thing as body.
    (None, content.to_string())
}

/// Walk the markdown, producing a whitespace-collapsed plain-text body, the
/// list of headings, and the first H1 (for the title fallback).
fn render_plain_and_headings(md: &str) -> (String, Vec<String>, Option<String>) {
    let mut plain = String::new();
    let mut headings = Vec::new();
    let mut first_h1: Option<String> = None;
    let mut in_heading = false;
    let mut cur_heading = String::new();
    let mut cur_level: Option<HeadingLevel> = None;

    for ev in MdParser::new_ext(md, Options::empty()) {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                in_heading = true;
                cur_level = Some(level);
                cur_heading.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
                let h = cur_heading.trim().to_string();
                if !h.is_empty() {
                    if first_h1.is_none() && cur_level == Some(HeadingLevel::H1) {
                        first_h1 = Some(h.clone());
                    }
                    headings.push(h);
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if in_heading {
                    cur_heading.push_str(&t);
                }
                plain.push_str(&t);
                plain.push(' ');
            }
            Event::SoftBreak | Event::HardBreak => plain.push(' '),
            _ => {}
        }
    }

    let body = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    (body, headings, first_h1)
}

fn fm_title(fm: &Option<serde_yaml::Value>) -> Option<String> {
    let s = fm.as_ref()?.get("title")?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Frontmatter `tags:` — accepts a YAML sequence or a single string
/// (space/comma separated). Leading `#` is stripped.
fn frontmatter_tags(fm: &Option<serde_yaml::Value>) -> Vec<String> {
    let mut out = Vec::new();
    let Some(v) = fm.as_ref().and_then(|v| v.get("tags")) else {
        return out;
    };
    match v {
        serde_yaml::Value::String(s) => {
            for part in s.split([',', ' ']) {
                push_tag(&mut out, part);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                if let Some(s) = item.as_str() {
                    push_tag(&mut out, s);
                }
            }
        }
        _ => {}
    }
    out
}

fn push_tag(out: &mut Vec<String>, raw: &str) {
    let t = raw.trim().trim_start_matches('#');
    if !t.is_empty() {
        out.push(t.to_string());
    }
}

fn filename_stem(rel_path: &str) -> String {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    name.strip_suffix(".md").unwrap_or(name).to_string()
}

fn dedupe(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|x| seen.insert(x.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_frontmatter_title_and_list_tags() {
        let md = "---\ntitle: My Note\ntags:\n  - alpha\n  - beta\n---\n# Ignored H1\nbody text\n";
        let note = parse_note("n.md", md, 0);
        assert_eq!(note.title, "My Note");
        assert_eq!(note.tags, vec!["alpha", "beta"]);
        assert!(note.body.contains("body text"));
        // Frontmatter must not leak into the indexed body.
        assert!(!note.body.contains("title"));
    }

    #[test]
    fn frontmatter_tags_accepts_string_form() {
        let md = "---\ntags: alpha, beta gamma\n---\ncontent\n";
        let note = parse_note("n.md", md, 0);
        assert_eq!(note.tags, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn collects_inline_tags_but_not_headings() {
        let md = "# Real Heading\nsome #inline-tag and #nested/tag here\n";
        let note = parse_note("n.md", md, 0);
        assert!(note.tags.contains(&"inline-tag".to_string()));
        assert!(note.tags.contains(&"nested/tag".to_string()));
        // "# Real Heading" (hash + space) must not become a tag.
        assert!(!note.tags.iter().any(|t| t.starts_with("Real")));
    }

    #[test]
    fn extracts_wikilinks_stripping_alias_and_section() {
        let md = "see [[Target Note]] and [[Other|alias]] and [[Third#section]]\n";
        let note = parse_note("n.md", md, 0);
        assert_eq!(note.links, vec!["Target Note", "Other", "Third"]);
    }

    #[test]
    fn title_falls_back_to_first_h1_then_filename() {
        let h1 = parse_note("folder/x.md", "# The Title\nbody\n", 0);
        assert_eq!(h1.title, "The Title");

        let stem = parse_note("folder/My File.md", "no heading, just body\n", 0);
        assert_eq!(stem.title, "My File");
    }

    #[test]
    fn collects_headings_and_plain_body() {
        let md = "# Top\ntext one\n## Sub\ntext two\n";
        let note = parse_note("n.md", md, 0);
        assert_eq!(note.headings, vec!["Top", "Sub"]);
        assert!(note.body.contains("text one"));
        assert!(note.body.contains("text two"));
        // Markdown heading markers must be stripped from the body.
        assert!(!note.body.contains('#'));
    }

    #[test]
    fn no_frontmatter_is_fine() {
        let note = parse_note("n.md", "just body, no fences\n", 42);
        assert_eq!(note.title, "n");
        assert_eq!(note.mtime, 42);
        assert!(note.tags.is_empty());
    }
}
