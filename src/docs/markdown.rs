//! GitHub-style Markdown rendering (FR-743). OWNER: deepseek.
//!
//! Contract: `pub fn render_markdown(source: &str) -> String` -> sanitized HTML.

use std::borrow::Cow;
use std::sync::OnceLock;

use ammonia::Builder;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd, html};
use regex::Regex;
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Highlighting is skipped (plain escaped text) above this size per block.
const MAX_HIGHLIGHT_BYTES: usize = 200 * 1024;
const USER_CONTENT_ID_PREFIX: &str = "user-content-";

/// Renders Markdown to sanitized, GitHub-flavoured HTML (FR-743).
pub fn render_markdown(source: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES;
    let events = transform(Parser::new_ext(source, options));
    let mut rendered = String::new();
    html::push_html(&mut rendered, events.into_iter());
    let rendered = convert_table_align(&rendered);
    let sanitized = sanitizer().clean(&rendered).to_string();
    add_target_blank(&sanitized)
}

/// Rewrites the event stream: fenced code is highlighted, and bare URLs in
/// ordinary text become autolinks (GFM). Code and explicit links are left alone.
fn transform<'a, I>(mut iter: I) -> Vec<Event<'a>>
where
    I: Iterator<Item = Event<'a>>,
{
    let mut out = Vec::new();
    let mut link_depth = 0usize;
    while let Some(event) = iter.next() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let mut code = String::new();
                for inner in iter.by_ref() {
                    match inner {
                        Event::Text(text) => code.push_str(&text),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split([' ', '\t'])
                        .next()
                        .filter(|token| !token.is_empty())
                        .map(str::to_owned),
                    CodeBlockKind::Indented => None,
                };
                out.push(Event::Html(
                    highlight_code(&code, language.as_deref()).into(),
                ));
            }
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) => {
                link_depth += 1;
                out.push(event);
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                link_depth = link_depth.saturating_sub(1);
                out.push(event);
            }
            Event::Text(text) if link_depth == 0 => push_text_with_links(&mut out, &text),
            other => out.push(other),
        }
    }
    out
}

/// Splits `text` into text/link events, turning bare `http(s)` URLs into links.
fn push_text_with_links<'a>(out: &mut Vec<Event<'a>>, text: &str) {
    let finder = linkify::LinkFinder::new();
    let mut last = 0usize;
    for link in finder.links(text) {
        if !matches!(link.kind(), linkify::LinkKind::Url) {
            continue;
        }
        let (start, end) = (link.start(), link.end());
        if start < last {
            continue;
        }
        if start > last {
            out.push(Event::Text(text[last..start].to_owned().into()));
        }
        let url = link.as_str();
        out.push(Event::Start(Tag::Link {
            link_type: LinkType::Autolink,
            dest_url: url.to_owned().into(),
            title: CowStr::Borrowed(""),
            id: CowStr::Borrowed(""),
        }));
        out.push(Event::Text(url.to_owned().into()));
        out.push(Event::End(TagEnd::Link));
        last = end;
    }
    if last < text.len() {
        out.push(Event::Text(text[last..].to_owned().into()));
    }
}

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn highlight_code(code: &str, language: Option<&str>) -> String {
    let plain = || plain_code(code, language);
    if code.len() > MAX_HIGHLIGHT_BYTES {
        return plain();
    }
    let set = syntax_set();
    let Some(syntax) = language.and_then(|lang| {
        set.find_syntax_by_token(lang)
            .or_else(|| set.find_syntax_by_extension(lang))
    }) else {
        return plain();
    };
    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        set,
        ClassStyle::SpacedPrefixed { prefix: "hl-" },
    );
    for line in LinesWithEndings::from(code) {
        if generator
            .parse_html_for_line_which_includes_newline(line)
            .is_err()
        {
            return plain();
        }
    }
    format!(
        "<pre><code{}>{}</code></pre>\n",
        code_class(language),
        generator.finalize()
    )
}

fn plain_code(code: &str, language: Option<&str>) -> String {
    format!(
        "<pre><code{}>{}</code></pre>\n",
        code_class(language),
        escape_html(code)
    )
}

fn code_class(language: Option<&str>) -> String {
    match language {
        Some(lang)
            if !lang.is_empty()
                && lang
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.')) =>
        {
            format!(" class=\"language-{lang}\"")
        }
        _ => String::new(),
    }
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
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

/// pulldown-cmark emits table alignment as inline `style`; the sanitizer drops
/// `style`, so turn it into the `align` attribute (FR-743).
fn convert_table_align(html: &str) -> String {
    html.replace(" style=\"text-align: left\"", " align=\"left\"")
        .replace(" style=\"text-align: center\"", " align=\"center\"")
        .replace(" style=\"text-align: right\"", " align=\"right\"")
}

fn class_token_allowed(token: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^(?:hl-|language-|footnote)").expect("static class token regex")
    });
    re.is_match(token)
}

fn prefix_fragment_href(value: &str) -> Cow<'_, str> {
    if !value.starts_with('#') {
        return Cow::Borrowed(value);
    }
    if value.starts_with('#') && value[1..].starts_with(USER_CONTENT_ID_PREFIX) {
        return Cow::Borrowed(value);
    }
    Cow::Owned(format!(
        "#{}{}",
        USER_CONTENT_ID_PREFIX,
        value.trim_start_matches('#')
    ))
}

fn sanitizer() -> Builder<'static> {
    let mut builder = Builder::default();
    builder
        .add_tags(&[
            "input", "section", "del", "sup", "sub", "span", "code", "pre", "div", "table",
            "thead", "tbody", "tfoot", "tr", "th", "td",
        ])
        .add_generic_attributes(&["id"])
        .add_tag_attributes("span", &["class"])
        .add_tag_attributes("code", &["class"])
        .add_tag_attributes("pre", &["class"])
        .add_tag_attributes("div", &["class"])
        .add_tag_attributes("section", &["class"])
        .add_tag_attributes("sup", &["class"])
        .add_tag_attributes("input", &["checked"])
        .add_tag_attribute_values("input", "type", &["checkbox"])
        .set_tag_attribute_value("input", "disabled", "disabled")
        .url_schemes(["http", "https", "mailto"].into_iter().collect())
        .link_rel(Some("noopener noreferrer"))
        .id_prefix(Some(USER_CONTENT_ID_PREFIX))
        .attribute_filter(|element, attribute, value| match attribute {
            "class" => {
                let kept = value
                    .split_whitespace()
                    .filter(|token| class_token_allowed(token))
                    .collect::<Vec<_>>()
                    .join(" ");
                if kept.is_empty() {
                    None
                } else {
                    Some(kept.into())
                }
            }
            "href" if element == "a" => Some(prefix_fragment_href(value).into_owned().into()),
            _ => Some(value.into()),
        });
    builder
}

/// Adds `target="_blank"` to absolute `http(s)` links after sanitization.
fn add_target_blank(html: &str) -> String {
    static ANCHOR: OnceLock<Regex> = OnceLock::new();
    static EXTERNAL: OnceLock<Regex> = OnceLock::new();
    let re = ANCHOR.get_or_init(|| Regex::new(r"<a\s[^>]*>").expect("static anchor regex"));
    let external = EXTERNAL.get_or_init(|| {
        Regex::new(r#"(?i)href="(?:https?:)?//"#).expect("static external href regex")
    });
    re.replace_all(html, |caps: &regex::Captures<'_>| {
        let tag = caps.get(0).map(|m| m.as_str()).unwrap_or_default();
        let is_external = external.is_match(tag);
        if is_external && !tag.contains("target=") {
            format!("{} target=\"_blank\">", &tag[..tag.len() - 1])
        } else {
            tag.to_owned()
        }
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-744: renders_gfm_features (FR-743)
    #[test]
    fn renders_gfm_features() {
        let source = "\
# Title

| left | right |
| :--- | ---: |
| a | b |

- [ ] todo
- [x] done

~~gone~~

Bare https://example.com/x link

Footnote reference[^1].

[^1]: the note

```rust
fn main() { let x = 1; }
```
";
        let html = render_markdown(source);

        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<th"), "{html}");
        assert!(html.contains("align=\"left\""), "{html}");
        assert!(html.contains("type=\"checkbox\""), "{html}");
        assert!(html.contains("disabled"), "{html}");
        assert!(html.contains("<del>"), "{html}");
        assert!(html.contains("<a href=\"https://example.com/x\""), "{html}");
        assert!(html.contains("rel=\"noopener noreferrer\""), "{html}");
        assert!(html.contains("target=\"_blank\""), "{html}");
        assert!(html.contains("footnote-reference"), "{html}");
        assert!(html.contains("hl-"), "{html}");

        static EXTERNAL_ANCHOR: OnceLock<Regex> = OnceLock::new();
        let anchor_re = EXTERNAL_ANCHOR
            .get_or_init(|| Regex::new(r"<a\s[^>]*>").expect("static anchor regex for tests"));
        for tag in anchor_re.find_iter(&html) {
            let tag = tag.as_str();
            if tag.contains("target=\"_blank\"") {
                assert!(
                    tag.contains("rel=\"noopener noreferrer\""),
                    "target without rel on same tag: {tag}"
                );
            }
        }
    }

    // TC-745: sanitizes_script_and_handlers (FR-743)
    #[test]
    fn sanitizes_script_and_handlers() {
        let source = "\
<script>alert('xss')</script>

<img src=\"x\" onerror=\"alert(1)\">

[click](javascript:alert(1))

<iframe src=\"https://evil.example\"></iframe>

<style>body { display: none }</style>

[case](JaVaScRiPt:alert(1))

[entity](jav&#x61;script:alert(1))

[colon](javascript&#58;alert(1))

[data](data:text/html;base64,PHNjcmlwdD4=)

[vbs](vbscript:alert(1))

<SCRIPT>alert(1)</SCRIPT>

<img src=x onmouseover=alert(1)>

<a onfocus=alert(1)>x</a>

<input type=\"checkbox\" name=evil value=1 onchange=alert(1)>

<span class=\"md-body hijack\" id=\"doc-title\">x</span>
";
        let html = render_markdown(source);
        let lower = html.to_ascii_lowercase();

        for forbidden in [
            "<script",
            "onerror",
            "onmouseover",
            "onfocus",
            "onchange",
            "javascript:",
            "vbscript:",
            "data:text/html",
            "<iframe",
            "<style",
            "alert(1)",
            "display: none",
            "name=",
            "value=",
            "class=\"md-body",
            "id=\"doc-title",
        ] {
            assert!(!lower.contains(forbidden), "{forbidden} survived in {html}");
        }
    }

    #[test]
    fn sanitizes_entity_and_case_encoded_urls() {
        let source = "[x](JaVaScRiPt:alert(1))\n[x](javascript&#58;alert(1))";
        let html = render_markdown(source);
        let lower = html.to_ascii_lowercase();
        assert!(!lower.contains("javascript:"), "{html}");
        assert!(!lower.contains("alert"), "{html}");
    }

    #[test]
    fn input_only_allows_checkbox() {
        let source = "<input type=\"checkbox\" checked name=n value=v onchange=x>";
        let html = render_markdown(source);
        let lower = html.to_ascii_lowercase();
        assert!(lower.contains("type=\"checkbox\""), "{html}");
        assert!(!lower.contains("name="), "{html}");
        assert!(!lower.contains("value="), "{html}");
        assert!(!lower.contains("onchange"), "{html}");
    }

    #[test]
    fn code_fence_info_cannot_inject_attributes() {
        let source = "````\n```\" onmouseover=alert(1)\nx\n```\n````\n";
        let html = render_markdown(source);
        for open in html.split("<code").skip(1) {
            let tag = open.split('>').next().unwrap_or("");
            assert!(
                !tag.to_ascii_lowercase().contains("onmouseover"),
                "injected handler in code open tag: {tag}"
            );
        }
    }

    #[test]
    fn skips_highlight_for_huge_code_blocks() {
        let body = "x".repeat(MAX_HIGHLIGHT_BYTES + 1);
        let source = format!("```rust\n{body}\n```");
        let html = render_markdown(&source);
        assert!(!html.contains("hl-"), "{html}");
        assert!(html.contains("<pre><code"), "{html}");
        assert!(html.contains(&body[..16]), "{html}");
    }

    #[test]
    fn external_protocol_relative_links_open_in_new_tab() {
        let source = "see //example.com/path and HTTP://EXAMPLE.COM/y";
        let html = render_markdown(source);
        assert!(html.contains("target=\"_blank\""), "{html}");
        static EXTERNAL_ANCHOR: OnceLock<Regex> = OnceLock::new();
        let anchor_re = EXTERNAL_ANCHOR
            .get_or_init(|| Regex::new(r"<a\s[^>]*>").expect("static anchor regex for tests"));
        for tag in anchor_re.find_iter(&html) {
            let tag = tag.as_str();
            if tag.contains("target=\"_blank\"") {
                assert!(
                    tag.contains("rel=\"noopener noreferrer\""),
                    "target without rel on same tag: {tag}"
                );
            }
        }
    }
}
