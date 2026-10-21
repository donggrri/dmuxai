//! Sanitized Markdown for dashboard chat assistant bubbles (reuses docs renderer).

use std::hash::{Hash, Hasher};

/// Cache key for rendered assistant HTML (`message_id` + content hash).
pub fn cache_key(message_id: &str, content: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{message_id}:{:016x}", hasher.finish())
}

/// Renders assistant chat Markdown to sanitized HTML (GFM, external links hardened).
pub fn render_chat_markdown(source: &str) -> String {
    crate::docs::markdown::render_markdown(source)
}

#[cfg(test)]
mod tests {
    use super::{cache_key, render_chat_markdown};

    // TC-1401 / FR-1401: chat markdown renders common GFM and strips scripts.
    #[test]
    fn cache_key_changes_when_content_changes() {
        let a = cache_key("m1", "hello");
        let b = cache_key("m1", "hello");
        let c = cache_key("m1", "hello!");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

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
