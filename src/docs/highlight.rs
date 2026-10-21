//! Code view for text files in the dashboard Files viewer: syntax
//! highlighting (syntect `hl-*` classes, like Markdown code blocks) split
//! into one `<span class="code-line">` per source line so the viewer can
//! number lines with a CSS counter (not part of the copied text).
//!
//! Contract: `pub fn highlight_file(name: &str, text: &str) -> Option<CodeView>`
//! returns `None` above `MAX_CODE_VIEW_BYTES` (the viewer shows plain text).
//! All file text is HTML-escaped; the only markup is `<pre>`, `<code>` and
//! `<span class="...">`.

use std::path::Path;

use serde::{Deserialize, Serialize};
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::docs::markdown::syntax_set;

/// Above this size the viewer gets no code view and shows plain text.
pub const MAX_CODE_VIEW_BYTES: usize = 256 * 1024;
/// Lines longer than this (minified bundles, data blobs) make syntect slow;
/// such files get line numbers without syntax colors.
pub const MAX_HIGHLIGHT_LINE_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeView {
    /// syntect syntax name (e.g. `C++`, `Python`), `None` for plain text.
    pub language: Option<String>,
    /// `<pre class="code-view"><code>` with one `code-line` span per line.
    pub html: String,
}

pub fn highlight_file(name: &str, text: &str) -> Option<CodeView> {
    if text.len() > MAX_CODE_VIEW_BYTES {
        return None;
    }
    let set = syntax_set();
    let long_lines = text
        .lines()
        .any(|line| line.len() > MAX_HIGHLIGHT_LINE_BYTES);
    let syntax = if long_lines {
        None
    } else {
        detect_syntax(set, name, text)
    };
    let body = syntax
        .and_then(|syntax| classed_html(set, syntax, text))
        .unwrap_or_else(|| escape_html(text));
    let language = syntax.map(|syntax| syntax.name.clone());
    let code_class = syntax
        .and_then(|syntax| syntax.file_extensions.first())
        .map(|ext| {
            let token: String = ext
                .to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            format!(" class=\"language-{token}\"")
        })
        .unwrap_or_default();
    Some(CodeView {
        html: format!(
            "<pre class=\"code-view\"><code{code_class}>{}</code></pre>",
            split_lines(&body)
        ),
        language,
    })
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

fn classed_html(set: &SyntaxSet, syntax: &SyntaxReference, text: &str) -> Option<String> {
    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        set,
        ClassStyle::SpacedPrefixed { prefix: "hl-" },
    );
    for line in LinesWithEndings::from(text) {
        generator
            .parse_html_for_line_which_includes_newline(line)
            .ok()?;
    }
    Some(generator.finalize())
}

/// Splits escaped/highlighted HTML at newlines into `code-line` spans,
/// closing the syntax spans still open at each line end and reopening them
/// on the next line so every line is a self-contained element. The text is
/// already escaped, so `<` only ever starts one of syntect's span tags.
fn split_lines(html: &str) -> String {
    let mut pieces: Vec<String> = html.split('\n').map(str::to_owned).collect();
    // After the file's final newline only closing tags remain: they belong
    // to the last line instead of starting an empty extra line.
    if pieces.len() > 1 && pieces.last().is_some_and(|last| !has_text(last)) {
        let tail = pieces.pop().unwrap_or_default();
        if let Some(last) = pieces.last_mut() {
            last.push_str(&tail);
        }
    }
    let mut out = String::with_capacity(html.len() + pieces.len() * 40);
    let mut open: Vec<String> = Vec::new();
    for piece in &pieces {
        out.push_str("<span class=\"code-line\">");
        for tag in &open {
            out.push_str(tag);
        }
        out.push_str(piece);
        track_tags(piece, &mut open);
        for _ in 0..open.len() {
            out.push_str("</span>");
        }
        out.push_str("\n</span>");
    }
    out
}

fn track_tags(line: &str, open: &mut Vec<String>) {
    let mut rest = line;
    while let Some(start) = rest.find('<') {
        let after = &rest[start..];
        let Some(end) = after.find('>') else {
            break;
        };
        let tag = &after[..=end];
        if tag.starts_with("</") {
            open.pop();
        } else {
            open.push(tag.to_owned());
        }
        rest = &after[end + 1..];
    }
}

fn has_text(piece: &str) -> bool {
    let mut in_tag = false;
    for ch in piece.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => return true,
            _ => {}
        }
    }
    false
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
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
    out
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

    /// Every tag is balanced inside each `code-line`.
    fn assert_balanced_lines(html: &str) {
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
                assert_eq!(depth, 1, "code-line opens inside <code> only: {html}");
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
                "int",
            ),
            (
                "tool.py",
                "def greet(name):\n    \"\"\"Doc\n    string\"\"\"\n    return name\n",
                "Python",
                "def",
            ),
            (
                "build.sh",
                "#!/bin/sh\nfor f in *.c; do\n  echo \"$f\"\ndone\n",
                "Bourne Again Shell (bash)",
                "for",
            ),
        ] {
            let view = highlight_file(name, text).expect("code view");
            assert_eq!(view.language.as_deref(), Some(language), "{name}");
            assert!(view.html.contains("hl-"), "{name}: {}", view.html);
            assert!(view.html.contains(keyword), "{name}");
            assert_eq!(lines(&view.html), 4, "{name}: {}", view.html);
            assert_balanced_lines(&view.html);
        }
        // `<vector>` is escaped, never markup.
        let cpp = highlight_file("a.cpp", "#include <vector>\n").unwrap();
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
    }

    #[test]
    fn known_names_and_shebangs_pick_a_syntax() {
        let make = highlight_file("Makefile", "all:\n\tcc -o app main.c\n").unwrap();
        assert_eq!(make.language.as_deref(), Some("Makefile"));
        let script = highlight_file("scripts/run", "#!/usr/bin/env python3\nprint(1)\n").unwrap();
        assert_eq!(script.language.as_deref(), Some("Python"));
    }

    #[test]
    fn unknown_extension_is_plain_text_with_lines() {
        let view = highlight_file("notes.zzz", "a < b\n\nlast").unwrap();
        assert_eq!(view.language, None);
        assert!(!view.html.contains("hl-"));
        assert_eq!(lines(&view.html), 3);
        assert!(view.html.contains("a &lt; b"));
        assert_balanced_lines(&view.html);
    }

    #[test]
    fn over_the_cap_falls_back_to_plain_text() {
        let big = "x = 1\n".repeat(MAX_CODE_VIEW_BYTES / 6 + 10);
        assert!(big.len() > MAX_CODE_VIEW_BYTES);
        assert_eq!(highlight_file("big.py", &big), None);
        // Very long lines keep line numbers but skip syntax colors.
        let long = format!(
            "x = '{}'\nprint(x)\n",
            "a".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1)
        );
        let view = highlight_file("long.py", &long).unwrap();
        assert_eq!(view.language, None);
        assert_eq!(lines(&view.html), 2);
    }

    #[test]
    fn markup_in_content_is_escaped() {
        let evil = "</code></pre><script>alert(1)</script>\n<img src=x onerror=alert(2)>\n\"'&\n";
        for name in ["evil.py", "evil.zzz", "evil.html.txt"] {
            let view = highlight_file(name, evil).unwrap();
            assert!(!view.html.contains("<script"), "{name}: {}", view.html);
            assert!(!view.html.contains("<img"), "{name}: {}", view.html);
            assert!(
                !view.html.contains("</code></pre><"),
                "{name}: {}",
                view.html
            );
            assert!(
                text_of(&view.html).contains("&lt;/code&gt;&lt;/pre&gt;&lt;script&gt;"),
                "{name}"
            );
            assert_balanced_lines(&view.html);
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
}
