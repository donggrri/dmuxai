//! Code view for text files in the dashboard Files viewer: syntax
//! highlighting with one `<span class="code-line">` per source line so the
//! viewer can number lines with a CSS counter (not part of the copied text).
//!
//! Contract: `pub fn highlight_file(name: &str, text: &str) -> Option<CodeView>`.
//! `None` means "no highlighted view": the viewer then shows the raw text
//! with a plain line-number gutter. That happens for files without a known
//! syntax and whenever a budget below is exceeded.
//!
//! The output is flat: every token is at most one `<span class="hl-...">`
//! inside its line (the class of the innermost styled scope), never nested
//! and never reopened across lines, so its size is bounded by the budgets.
//! All file text is HTML-escaped; the only markup is `<pre>`, `<code>` and
//! `<span class="...">`. CR LF and lone CR line ends are shown as one line
//! break; callers keep the raw text for editing.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::docs::markdown::syntax_set;

/// Largest input that is highlighted. Measured in a release build: 64 KiB
/// of C++ takes about 0.25 s and gives about 0.6 MB of HTML (syntect with
/// the pure-Rust regex engine runs at roughly 250 KB/s); 96 KiB already took
/// 0.39 s. Larger files get the plain view.
pub const MAX_HIGHLIGHT_BYTES: usize = 64 * 1024;
/// Largest number of lines that is highlighted (every line costs a
/// `code-line` element in the browser).
pub const MAX_HIGHLIGHT_LINES: usize = 3_000;
/// A line longer than this (minified bundles, data blobs) disables
/// highlighting for the file; syntect gets slow on such lines.
pub const MAX_HIGHLIGHT_LINE_BYTES: usize = 4 * 1024;
/// Hard cap on the generated HTML; above it the viewer shows plain text.
pub const MAX_CODE_HTML_BYTES: usize = 2 * 1024 * 1024;
/// Hard cap on syntax spans in one file.
pub const MAX_SPANS: usize = 60_000;
/// Per-line span budget; the rest of a busier line is emitted unstyled.
pub const MAX_SPANS_PER_LINE: usize = 200;

/// Classes styled by docs.css, in the order of their CSS rules.
const STYLED: &[&str] = &[
    "keyword",
    "string",
    "comment",
    "constant",
    "entity",
    "storage",
    "support",
    "variable",
    "punctuation",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeView {
    /// syntect syntax name (e.g. `C++`, `Python`).
    pub language: String,
    /// `<pre class="code-view"><code class="language-…">` with one
    /// `code-line` span per line.
    pub html: String,
}

/// Highlighted view of `text`, or `None` for the plain line-numbered view.
/// Results are cached by file name and content hash.
pub fn highlight_file_cached(name: &str, sha256: &str, text: &str) -> Option<Arc<CodeView>> {
    type Cache = Mutex<VecDeque<((String, String), Option<Arc<CodeView>>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    const CACHE_ENTRIES: usize = 16;
    let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
    let key = (name.to_owned(), sha256.to_owned());
    if let Some((_, hit)) = cache.lock().unwrap().iter().find(|(k, _)| *k == key) {
        return hit.clone();
    }
    let view = highlight_file(name, text).map(Arc::new);
    let mut guard = cache.lock().unwrap();
    guard.push_front((key, view.clone()));
    guard.truncate(CACHE_ENTRIES);
    view
}

pub fn highlight_file(name: &str, text: &str) -> Option<CodeView> {
    if text.len() > MAX_HIGHLIGHT_BYTES {
        return None;
    }
    let text = normalize_line_ends(text);
    let lines: Vec<&str> = split_view_lines(&text);
    if lines.len() > MAX_HIGHLIGHT_LINES
        || lines
            .iter()
            .any(|line| line.len() > MAX_HIGHLIGHT_LINE_BYTES)
    {
        return None;
    }
    let set = syntax_set();
    let syntax = detect_syntax(set, name, &text)?;
    let body = highlight_lines(set, syntax, &lines)?;
    let token: String = syntax
        .file_extensions
        .first()
        .map(|ext| {
            ext.to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect()
        })
        .unwrap_or_default();
    let code_open = if token.is_empty() {
        "<code>".to_owned()
    } else {
        format!("<code class=\"language-{token}\">")
    };
    Some(CodeView {
        language: syntax.name.clone(),
        html: format!("<pre class=\"code-view\">{code_open}{body}</code></pre>"),
    })
}

/// CR LF and lone CR become LF (display only).
fn normalize_line_ends(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\r') {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Lines as the viewer shows them: a final line end does not start an
/// extra empty line; an empty file is one empty line.
fn split_view_lines(text: &str) -> Vec<&str> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n').collect()
}

fn detect_syntax<'a>(set: &'a SyntaxSet, name: &str, text: &str) -> Option<&'a SyntaxReference> {
    let file_name = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name);
    let lower = file_name.to_ascii_lowercase();
    let extension = Path::new(&lower).extension().and_then(|e| e.to_str());
    let by_name = match lower.as_str() {
        "makefile" | "gnumakefile" => set.find_syntax_by_extension("Makefile"),
        "dockerfile" => set.find_syntax_by_extension("Dockerfile"),
        _ => None,
    };
    let plain = |syntax: &&SyntaxReference| syntax.name != "Plain Text";
    by_name
        .filter(plain)
        .or_else(|| {
            extension
                .and_then(|ext| {
                    set.find_syntax_by_extension(ext)
                        .or_else(|| set.find_syntax_by_token(ext))
                })
                .filter(plain)
        })
        .or_else(|| {
            let first = text.lines().next().unwrap_or("");
            if first.starts_with("#!") {
                set.find_syntax_by_first_line(first).filter(plain)
            } else {
                None
            }
        })
}

/// The styled class for a scope: its first atom (`keyword` in
/// `keyword.control.c++`) when docs.css styles it.
fn scope_class(
    scope: Scope,
    memo: &mut HashMap<Scope, Option<&'static str>>,
) -> Option<&'static str> {
    *memo.entry(scope).or_insert_with(|| {
        let name = scope.build_string();
        let first = name.split('.').next().unwrap_or("");
        STYLED.iter().copied().find(|styled| *styled == first)
    })
}

/// The class of the innermost styled scope on the stack.
fn stack_class(
    stack: &ScopeStack,
    memo: &mut HashMap<Scope, Option<&'static str>>,
) -> Option<&'static str> {
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(|scope| scope_class(*scope, memo))
}

fn highlight_lines(set: &SyntaxSet, syntax: &SyntaxReference, lines: &[&str]) -> Option<String> {
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut memo = HashMap::new();
    let mut out = String::with_capacity(lines.iter().map(|l| l.len() + 40).sum());
    let mut spans_total = 0usize;
    let mut with_newline = String::new();
    for line in lines {
        with_newline.clear();
        with_newline.push_str(line);
        with_newline.push('\n');
        let ops = state.parse_line(&with_newline, set).ok()?;
        out.push_str("<span class=\"code-line\">");
        let mut spans_line = 0usize;
        let mut cursor = 0usize;
        let mut pending: Option<&'static str> = stack_class(&stack, &mut memo);
        let mut run = String::new();
        let mut run_class: Option<&'static str> = None;
        let line_len = line.len();
        let mut segments: Vec<(usize, usize, Option<&'static str>)> = Vec::new();
        for (pos, op) in &ops {
            let pos = (*pos).min(line_len);
            if pos > cursor {
                segments.push((cursor, pos, pending));
                cursor = pos;
            }
            stack.apply(op).ok()?;
            pending = stack_class(&stack, &mut memo);
        }
        if cursor < line_len {
            segments.push((cursor, line_len, pending));
        }
        for (start, end, class) in segments {
            if class != run_class {
                flush_run(
                    &mut run,
                    run_class,
                    &mut out,
                    &mut spans_line,
                    &mut spans_total,
                );
                run_class = class;
            }
            escape_into(&line[start..end], &mut run);
        }
        flush_run(
            &mut run,
            run_class,
            &mut out,
            &mut spans_line,
            &mut spans_total,
        );
        out.push_str("\n</span>");
        if spans_total > MAX_SPANS || out.len() > MAX_CODE_HTML_BYTES {
            return None;
        }
    }
    Some(out)
}

/// Emits a run of escaped text, styled while the line's span budget lasts.
fn flush_run(
    run: &mut String,
    class: Option<&'static str>,
    out: &mut String,
    spans_line: &mut usize,
    spans_total: &mut usize,
) {
    if run.is_empty() {
        return;
    }
    match class {
        Some(class) if *spans_line < MAX_SPANS_PER_LINE => {
            out.push_str("<span class=\"hl-");
            out.push_str(class);
            out.push_str("\">");
            out.push_str(run);
            out.push_str("</span>");
            *spans_line += 1;
            *spans_total += 1;
        }
        _ => out.push_str(run),
    }
    run.clear();
}

fn escape_into(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The escaped text with all tags removed.
    fn text_of(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(ch),
                _ => {}
            }
        }
        out
    }

    fn lines(html: &str) -> usize {
        html.matches("<span class=\"code-line\">").count()
    }

    /// Tags are balanced, `code-line` spans sit directly in `<code>` and
    /// syntax spans are never nested.
    fn assert_flat_and_balanced(html: &str) {
        let inner = html
            .strip_prefix("<pre class=\"code-view\">")
            .and_then(|rest| rest.strip_suffix("</pre>"))
            .expect("pre wrapper");
        let mut depth: i64 = 0;
        let mut rest = inner;
        while let Some(start) = rest.find('<') {
            let after = &rest[start..];
            let end = after.find('>').expect("closed tag");
            let tag = &after[..=end];
            if tag.starts_with("<span class=\"code-line\">") {
                assert_eq!(depth, 1, "code-line directly inside <code>: {html}");
            } else if tag.starts_with("<span class=\"hl-") {
                assert_eq!(depth, 2, "syntax spans are flat: {html}");
            }
            if tag.starts_with("</") {
                depth -= 1;
            } else {
                depth += 1;
            }
            assert!(depth >= 0, "unbalanced: {html}");
            rest = &after[end + 1..];
        }
        assert_eq!(depth, 0, "unbalanced: {html}");
    }

    #[test]
    fn cpp_python_shell_are_highlighted_per_line() {
        for (name, text, language, keyword) in [
            (
                "src/main.cpp",
                "#include <vector>\n/* multi\n   line */\nint main() { return 0; }\n",
                "C++",
                "<span class=\"hl-storage\">int</span>",
            ),
            (
                "tool.py",
                "def greet(name):\n    \"\"\"Doc\n    string\"\"\"\n    return name\n",
                "Python",
                "<span class=\"hl-storage\">def</span>",
            ),
            (
                "build.sh",
                "#!/bin/sh\nfor f in *.c; do\n  echo \"$f\"\ndone\n",
                "Bourne Again Shell (bash)",
                "<span class=\"hl-keyword\">for</span>",
            ),
        ] {
            let view = highlight_file(name, text).expect("code view");
            assert_eq!(view.language, language, "{name}");
            assert!(view.html.contains(keyword), "{name}: {}", view.html);
            assert_eq!(lines(&view.html), 4, "{name}: {}", view.html);
            assert_flat_and_balanced(&view.html);
        }
        let cpp = highlight_file("a.cpp", "#include <vector>\n/* a\n b */\n").unwrap();
        assert!(!cpp.html.contains("<vector"), "{}", cpp.html);
        assert!(
            text_of(&cpp.html).contains("&lt;vector&gt;"),
            "{}",
            cpp.html
        );
        assert!(
            cpp.html.contains("<code class=\"language-cpp\">"),
            "{}",
            cpp.html
        );
        // A comment spanning lines is styled on both lines.
        assert_eq!(
            cpp.html.matches("<span class=\"hl-comment\">").count(),
            2,
            "{}",
            cpp.html
        );
    }

    #[test]
    fn known_names_and_shebangs_pick_a_syntax() {
        let make = highlight_file("Makefile", "all:\n\tcc -o app main.c\n").unwrap();
        assert_eq!(make.language, "Makefile");
        let script = highlight_file("scripts/run", "#!/usr/bin/env python3\nprint(1)\n").unwrap();
        assert_eq!(script.language, "Python");
    }

    #[test]
    fn unknown_syntax_gets_the_plain_view() {
        assert_eq!(highlight_file("notes.zzz", "a < b\n"), None);
        assert_eq!(highlight_file("notes.log", "a < b\n"), None);
    }

    #[test]
    fn input_caps_fall_back_to_plain() {
        let big = "x = 1\n".repeat(MAX_HIGHLIGHT_BYTES / 6 + 10);
        assert!(big.len() > MAX_HIGHLIGHT_BYTES);
        assert_eq!(highlight_file("big.py", &big), None);
        let many = "\n".repeat(MAX_HIGHLIGHT_LINES + 1);
        assert_eq!(highlight_file("blank.py", &many), None);
        let long = format!("x = '{}'\n", "a".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1));
        assert_eq!(highlight_file("long.py", &long), None);
    }

    // Review blocker: '[' nesting made the old output grow quadratically
    // (a 256 KiB file produced ~780 MB). The flat output stays bounded.
    #[test]
    fn pathological_nesting_is_bounded() {
        let nested = "[\n".repeat(MAX_HIGHLIGHT_LINES);
        let view = highlight_file("nested.json", &nested).expect("within the line cap");
        assert_eq!(lines(&view.html), MAX_HIGHLIGHT_LINES);
        assert_flat_and_balanced(&view.html);
        assert!(
            view.html.len() < nested.len() * 40,
            "{} bytes for {} input bytes",
            view.html.len(),
            nested.len()
        );
        // Inputs just under every input cap still respect the output caps.
        for (name, unit) in [
            ("a.json", "[{\"a\":[1,{\"b\":[\"c\"]}]},\n"),
            ("a.cpp", "int f(int a){return a<1?(a+2)*3:f(a-1);} // c\n"),
            ("a.py", "x = [f(a, b) for a, b in zip(\"ab\", 'cd')]  # c\n"),
        ] {
            let text = unit.repeat((MAX_HIGHLIGHT_BYTES - 1) / unit.len());
            if let Some(view) = highlight_file(name, &text) {
                assert!(view.html.len() <= MAX_CODE_HTML_BYTES + 64 * 1024, "{name}");
                assert!(
                    view.html.matches("<span class=\"hl-").count()
                        <= MAX_SPANS + MAX_SPANS_PER_LINE
                );
                assert_flat_and_balanced(&view.html);
            }
        }
    }

    #[test]
    fn per_line_span_budget() {
        let busy = format!("{}\n", "1,".repeat(MAX_SPANS_PER_LINE * 2));
        let view = highlight_file("busy.json", &busy).unwrap();
        assert!(view.html.matches("<span class=\"hl-").count() <= MAX_SPANS_PER_LINE);
        assert!(text_of(&view.html).contains(&busy[..busy.len() - 1]));
    }

    #[test]
    fn crlf_and_lone_cr_are_single_line_breaks() {
        let crlf = highlight_file("w.py", "a = 1\r\nb = 2\r\n").unwrap();
        assert_eq!(lines(&crlf.html), 2, "{}", crlf.html);
        assert!(!crlf.html.contains('\r'), "{}", crlf.html);
        let cr = highlight_file("m.py", "a = 1\rb = 2\r").unwrap();
        assert_eq!(lines(&cr.html), 2, "{}", cr.html);
        assert!(!cr.html.contains('\r'));
    }

    #[test]
    fn markup_in_content_is_escaped() {
        let evil = "</code></pre><script>alert(1)</script>\n<img src=x onerror=alert(2)>\n\"'&\n";
        for name in ["evil.py", "evil.cpp", "evil.sh"] {
            let view = highlight_file(name, evil).unwrap();
            assert!(!view.html.contains("<script"), "{name}: {}", view.html);
            assert!(!view.html.contains("<img"), "{name}: {}", view.html);
            assert!(
                text_of(&view.html).contains("&lt;/code&gt;&lt;/pre&gt;&lt;script&gt;"),
                "{name}"
            );
            assert_flat_and_balanced(&view.html);
        }
    }

    #[test]
    fn empty_and_unterminated_files() {
        assert_eq!(lines(&highlight_file("e.rs", "").unwrap().html), 1);
        assert_eq!(lines(&highlight_file("e.rs", "fn a() {}").unwrap().html), 1);
        assert_eq!(
            lines(&highlight_file("e.rs", "fn a() {}\n").unwrap().html),
            1
        );
        assert_eq!(lines(&highlight_file("e.rs", "\n\n").unwrap().html), 2);
    }

    #[test]
    fn cache_returns_the_same_view() {
        let a = highlight_file_cached("c.py", "sha-1", "x = 1\n").unwrap();
        let b = highlight_file_cached("c.py", "sha-1", "ignored on a hit\n").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(highlight_file_cached("c.zzz", "sha-2", "x\n"), None);
    }
}
