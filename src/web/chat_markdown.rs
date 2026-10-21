//! Sanitized Markdown for dashboard chat assistant bubbles (reuses docs renderer).

/// Renders assistant chat Markdown to sanitized HTML (GFM, external links hardened).
pub fn render_chat_markdown(source: &str) -> String {
    crate::docs::markdown::render_markdown(source)
}

#[cfg(test)]
mod tests {
    use super::render_chat_markdown;

    // TC-1401 / FR-1401: chat markdown renders common GFM and strips scripts.
    #[test]
    fn tc_1401_chat_markdown_renders_gfm_and_sanitizes() {
        let source = r"## Summary

**bold** and [link](https://example.com)

| a | b |
|---|---|
| 1 | 2 |

```rust
fn main() {}
```

<script>alert(1)</script>
";
        let html = render_chat_markdown(source);
        assert!(html.contains("<h2>"), "heading: {html}");
        assert!(html.contains("<strong>bold</strong>"), "bold: {html}");
        assert!(html.contains("<table>"), "table: {html}");
        assert!(
            html.contains("href=\"https://example.com\""),
            "link: {html}"
        );
        assert!(
            html.contains("rel=\"noopener noreferrer\""),
            "link rel: {html}"
        );
        assert!(html.contains("<pre"), "code block: {html}");
        assert!(
            !html.to_lowercase().contains("<script"),
            "script stripped: {html}"
        );
    }
}
