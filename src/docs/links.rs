//! File links in agent output (FR-749). OWNER: deepseek.
//!
//! Contract:
//! - `pub fn extract_doc_links(ws: &Workspace, text: &str) -> Vec<String>` — dedup, order of first
//!   appearance, only existing files inside the workspace, max 50.
//! - `pub fn find_link_spans(ws: &Workspace, line: &str) -> Vec<(std::ops::Range<usize>, String)>`
//!   byte ranges in `line` of each linkable token and its workspace path (for TUI, FR-750).
//! - At most [`MAX_PATH_RESOLUTIONS`] distinct path lookups per call (NFR-752); further candidates
//!   are skipped.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;

use super::confine::Workspace;

/// FR-749: at most this many links are extracted per output.
const MAX_LINKS: usize = 50;
/// Ignore implausibly long tokens before touching the filesystem.
const MAX_TOKEN_CHARS: usize = 300;
/// Cap distinct `canonicalize` attempts per `collect_spans` call (NFR-752).
const MAX_PATH_RESOLUTIONS: usize = 1000;

struct ResolveCtx {
    cache: HashMap<String, Option<String>>,
    attempts: usize,
}

impl ResolveCtx {
    fn resolve(&mut self, ws: &Workspace, candidate: &str) -> Option<String> {
        if let Some(hit) = self.cache.get(candidate) {
            return hit.clone();
        }
        if self.attempts >= MAX_PATH_RESOLUTIONS {
            self.cache.insert(candidate.to_owned(), None);
            return None;
        }
        self.attempts += 1;
        let result = workspace_file(ws, candidate);
        self.cache.insert(candidate.to_owned(), result.clone());
        result
    }
}

/// Returns the deduplicated workspace paths named by `text` (FR-749).
pub fn extract_doc_links(ws: &Workspace, text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    for (_, path) in collect_spans(ws, text) {
        if seen.insert(path.clone()) {
            links.push(path);
            if links.len() >= MAX_LINKS {
                break;
            }
        }
    }
    links
}

/// Byte ranges of linkable tokens in `line` and their workspace paths (FR-750).
pub fn find_link_spans(ws: &Workspace, line: &str) -> Vec<(Range<usize>, String)> {
    collect_spans(ws, line)
}

fn collect_spans(ws: &Workspace, text: &str) -> Vec<(Range<usize>, String)> {
    let mut ctx = ResolveCtx {
        cache: HashMap::new(),
        attempts: 0,
    };
    let mut candidates: Vec<(usize, usize, String, u8)> = Vec::new();

    for matched in file_uri_re().find_iter(text) {
        let raw = trim_trailing_punctuation(matched.as_str());
        let end = matched.start() + raw.len();
        if let Some(path) = file_uri_path(ws, raw, &mut ctx) {
            candidates.push((matched.start(), end, path, 0));
        }
    }

    for captures in markdown_re().captures_iter(text) {
        let (Some(whole), Some(target)) = (captures.get(0), captures.get(1)) else {
            continue;
        };
        if let Some(path) = markdown_target(ws, target.as_str(), &mut ctx) {
            candidates.push((whole.start(), whole.end(), path, 1));
        }
    }

    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index > start
            && let Some(candidate) = bare_token(ws, &text[start..index], start, &mut ctx)
        {
            candidates.push(candidate);
        }
    }

    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.3.cmp(&b.3))
            .then((b.1 - b.0).cmp(&(a.1 - a.0)))
    });

    let mut spans = Vec::new();
    let mut accepted = HashSet::new();
    let mut last_end = 0usize;
    for (start, end, path, _) in candidates {
        if start < last_end {
            continue;
        }
        last_end = end;
        if accepted.insert(path.clone()) {
            spans.push((start..end, path));
        }
    }
    spans
}

/// A workspace-relative token, with `:line[:col]` and trailing punctuation
/// removed. Returns the byte range of the surviving path.
fn bare_token(
    ws: &Workspace,
    token: &str,
    offset: usize,
    ctx: &mut ResolveCtx,
) -> Option<(usize, usize, String, u8)> {
    let trimmed = token.trim_start_matches(['(', '[', '{', '<', '"', '\'']);
    let front = token.len() - trimmed.len();
    let trimmed = trim_trailing_punctuation(trimmed);
    let trimmed = strip_line_suffix(trimmed);
    let trimmed = trim_trailing_punctuation(trimmed);
    if trimmed.is_empty() {
        return None;
    }
    let path = ctx.resolve(ws, trimmed)?;
    let start = offset + front;
    Some((start, start + trimmed.len(), path, 2))
}

fn markdown_target(ws: &Workspace, raw: &str, ctx: &mut ResolveCtx) -> Option<String> {
    let target = raw.trim();
    let target = target
        .strip_prefix('<')
        .and_then(|inner| inner.strip_suffix('>'))
        .unwrap_or(target);
    let target = target.split('#').next().unwrap_or(target);
    let target = strip_line_suffix(target);
    let target = percent_decode(target);
    ctx.resolve(ws, &target)
}

fn workspace_file(ws: &Workspace, candidate: &str) -> Option<String> {
    if candidate.is_empty() || candidate.chars().count() > MAX_TOKEN_CHARS {
        return None;
    }
    let has_slash = candidate.contains('/');
    let has_extension = candidate
        .rsplit('/')
        .next()
        .is_some_and(|name| name.contains('.') && !name.starts_with('.'));
    if !has_slash && !has_extension {
        return None;
    }
    let resolved = ws.resolve(candidate).ok()?;
    resolved.absolute.is_file().then_some(resolved.relative)
}

fn file_uri_path(ws: &Workspace, raw: &str, ctx: &mut ResolveCtx) -> Option<String> {
    let rest = raw.strip_prefix("file://")?;
    let rest = match rest.strip_prefix("localhost") {
        None => rest,
        Some("") if rest.len() == "localhost".len() => "/",
        Some(after) if after.starts_with('/') => after,
        Some(_) => return None,
    };
    let decoded = percent_decode(rest);
    if !decoded.starts_with('/') {
        return None;
    }
    let absolute = PathBuf::from(decoded);
    let relative = ws.relative_of(&absolute)?;
    ctx.resolve(ws, &relative)
}

fn strip_line_suffix(input: &str) -> &str {
    let mut result = input;
    for _ in 0..2 {
        if let Some(colon) = result.rfind(':') {
            let suffix = &result[colon + 1..];
            if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) {
                result = &result[..colon];
                continue;
            }
        }
        break;
    }
    result
}

fn trim_trailing_punctuation(input: &str) -> &str {
    const TRAILING: [char; 11] = [')', ']', '}', '>', '.', ',', ';', '\'', '"', '`', ':'];
    let mut end = input.len();
    while end > 0 {
        let Some(ch) = input[..end].chars().next_back() else {
            break;
        };
        if TRAILING.contains(&ch) {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    &input[..end]
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2]))
        {
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn file_uri_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"file://[^\s<>"'`)\]]+"#).expect("static file uri regex"))
}

fn markdown_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[[^\]\n]*\]\(([^)\n]+)\)").expect("static markdown regex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_uri_root(dir: &std::path::Path) -> String {
        let lossy = dir.to_string_lossy();
        let normalized = lossy.replace('\\', "/");
        format!("file:///{normalized}")
    }

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("docs/a.md"), "# a").unwrap();
        std::fs::write(dir.path().join("docs/b.md"), "# b").unwrap();
        std::fs::write(dir.path().join("src/x.rs"), "fn x() {}").unwrap();
        std::fs::write(dir.path().join("v1.0"), "version").unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        (dir, ws)
    }

    // TC-753: extracts_workspace_links_only (FR-749)
    #[test]
    fn extracts_workspace_links_only() {
        let (dir, ws) = workspace();
        let root = file_uri_root(dir.path());
        let text = format!(
            "see {root}/docs/a.md and src/x.rs:12 then [b](docs/b.md), \
             [spaced](docs/my%20file.md) v1.0 /etc/passwd missing.md https://x.y \
             file://docs/a.md file://host/docs/a.md file://localhostdocs/a.md \
             {root}/docs/%2e%2e/secret.md",
        );
        std::fs::write(dir.path().join("docs/my file.md"), "# spaced").unwrap();
        assert_eq!(
            extract_doc_links(&ws, &text),
            vec![
                "docs/a.md",
                "src/x.rs",
                "docs/b.md",
                "docs/my file.md",
                "v1.0"
            ]
        );
    }

    #[test]
    fn find_link_spans_reports_byte_ranges() {
        let (_dir, ws) = workspace();
        let line = "open docs/a.md now";
        assert_eq!(
            find_link_spans(&ws, line),
            vec![(5..14, "docs/a.md".to_owned())]
        );

        let punc = "see (src/x.rs:12), then [b](docs/b.md).";
        let spans = find_link_spans(&ws, punc);
        let paths: Vec<&str> = spans.iter().map(|(_, p)| p.as_str()).collect();
        assert_eq!(paths, vec!["src/x.rs", "docs/b.md"]);
        assert_eq!(&punc[spans[0].0.clone()], "src/x.rs");
        assert_eq!(&punc[spans[0].0.end..spans[0].0.end + 3], ":12");
    }

    #[test]
    fn extract_doc_links_caps_path_resolutions() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..5 {
            std::fs::write(dir.path().join(format!("f{index}.txt")), "x").unwrap();
        }
        let ws = Workspace::new(dir.path()).unwrap();
        let mut tokens = String::new();
        for index in 0..2000 {
            tokens.push_str(&format!("f{}.txt ", index % 5));
        }
        let start = std::time::Instant::now();
        let links = extract_doc_links(&ws, &tokens);
        assert_eq!(links.len(), 5);
        assert!(
            start.elapsed().as_secs() < 2,
            "resolution cap should keep turn-end path bounded"
        );
    }
}
