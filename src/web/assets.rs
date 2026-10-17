pub const INDEX_HTML: &str = include_str!("assets/index.html");
pub const APP_JS: &str = include_str!("assets/app.js");
pub const MEMORY_JS: &str = include_str!("assets/memory.js");
pub const SESSION_JS: &str = include_str!("assets/session.js");
pub const SESSION_CSS: &str = include_str!("assets/session.css");
pub const APP_CSS: &str = include_str!("assets/app.css");
pub const DOCS_JS: &str = include_str!("assets/docs.js");
pub const DOCS_CSS: &str = include_str!("assets/docs.css");
pub const FAVICON_IDLE_SVG: &str = include_str!("assets/favicon-idle.svg");
pub const FAVICON_RUNNING_SVG: &str = include_str!("assets/favicon-running.svg");
pub const FAVICON_DONE_SVG: &str = include_str!("assets/favicon-done.svg");
pub const FAVICON_FAILED_SVG: &str = include_str!("assets/favicon-failed.svg");

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::web::hub::DashboardHub;
    use crate::web::server::build_router;
    use crate::web::snapshot::SnapshotSource;

    // TC-529: static_page_is_embedded_and_hardened
    #[tokio::test]
    async fn static_page_is_embedded_and_hardened() {
        assert!(INDEX_HTML.contains("/assets/app.js"));
        assert!(APP_JS.contains("/api/events"));
        assert!(!APP_CSS.is_empty());

        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let token = "testtoken123".to_string();
        let router = build_router(
            hub,
            token,
            Some(vec![
                "localhost:7420".to_string(),
                "127.0.0.1:7420".to_string(),
            ]),
        );

        // GET /
        let req_root = axum::http::Request::builder()
            .uri("/")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp_root = router.clone().oneshot(req_root).await.unwrap();
        assert_eq!(resp_root.status(), axum::http::StatusCode::OK);
        assert_eq!(
            resp_root
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .unwrap(),
            "no-store"
        );
        assert_eq!(
            resp_root.headers().get("content-security-policy").unwrap(),
            "default-src 'self'"
        );
        assert_eq!(
            resp_root.headers().get("referrer-policy").unwrap(),
            "no-referrer"
        );
        assert_eq!(
            resp_root.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert!(
            resp_root
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("text/html")
        );
        let body_root = resp_root.into_body().collect().await.unwrap().to_bytes();
        let body_root_str = String::from_utf8_lossy(&body_root);
        assert!(body_root_str.contains("/assets/app.js"));

        // GET /assets/app.js
        let req_js = axum::http::Request::builder()
            .uri("/assets/app.js")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp_js = router.clone().oneshot(req_js).await.unwrap();
        assert_eq!(resp_js.status(), axum::http::StatusCode::OK);
        assert_eq!(
            resp_js
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .unwrap(),
            "no-store"
        );
        assert_eq!(
            resp_js.headers().get("content-security-policy").unwrap(),
            "default-src 'self'"
        );
        let body_js = resp_js.into_body().collect().await.unwrap().to_bytes();
        let body_js_str = String::from_utf8_lossy(&body_js);
        assert!(body_js_str.contains("/api/events"));
        assert!(body_js_str.contains("/api/state"));
        assert!(body_js_str.contains("token"));

        // GET /assets/app.css
        let req_css = axum::http::Request::builder()
            .uri("/assets/app.css")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp_css = router.oneshot(req_css).await.unwrap();
        assert_eq!(resp_css.status(), axum::http::StatusCode::OK);
        assert_eq!(
            resp_css
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .unwrap(),
            "no-store"
        );
        assert!(
            resp_css
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("text/css")
        );
    }

    // TC-731 (UX-728): Memory panel markup and client API paths.
    #[test]
    fn memory_panel_assets_present() {
        assert!(INDEX_HTML.contains("id=\"memory-panel\""));
        assert!(INDEX_HTML.contains("/assets/memory.js"));
        assert!(MEMORY_JS.contains("/api/memory"));
        assert!(MEMORY_JS.contains("/api/memory/save"));
        assert!(MEMORY_JS.contains("/approve"));
        assert!(MEMORY_JS.contains("/reject"));
    }

    // TC-786 (FR-782, FR-783, UX-780, NFR-780): the session module is wired
    // into the page and carries the prompt, palette and busy-state behavior.
    #[test]
    fn session_module_has_parity_controls() {
        assert!(INDEX_HTML.contains("id=\"session\""));
        assert!(INDEX_HTML.contains("/assets/session.js"));
        assert!(INDEX_HTML.contains("/assets/session.css"));
        // Enter sends, Shift+Enter is a newline, IME composition never sends.
        assert!(SESSION_JS.contains("e.key === 'Enter' && !e.shiftKey"));
        assert!(SESSION_JS.contains("e.isComposing"));
        // Ctrl+K and "/" in an empty prompt open the palette; Esc closes it.
        assert!(SESSION_JS.contains("e.key === 'k'"));
        assert!(SESSION_JS.contains("ui.input.value === '/'"));
        assert!(SESSION_JS.contains("e.key === 'Escape'"));
        // UX-780: pickers are disabled and Cancel is shown while busy.
        assert!(SESSION_JS.contains("ui.mode.disabled = viewOnly || busy"));
        assert!(SESSION_JS.contains("ui.model.disabled = viewOnly || busy"));
        assert!(SESSION_JS.contains("ui.cancel.hidden = !busy"));
        assert!(SESSION_JS.contains("/api/command"));
        assert!(SESSION_JS.contains("/api/prompt"));
        // CSP: no inline handlers or HTML injection.
        assert!(!SESSION_JS.contains("innerHTML"));
        assert!(!SESSION_JS.contains("onclick"));
        assert!(SESSION_CSS.starts_with("/* session"));
        // NFR-780: app.js only re-dispatches snapshots; session.js reuses them
        // instead of opening a second SSE stream.
        assert!(!APP_JS.contains("/assets/session.js"));
        assert!(APP_JS.contains("new CustomEvent('dmuxai:snapshot'"));
        assert!(SESSION_JS.contains("addEventListener('dmuxai:snapshot'"));
        assert!(!SESSION_JS.contains("EventSource"));
    }

    // TC-786 (NFR-780): the module and its stylesheet are served with the
    // same hardened headers as the other assets, without a token.
    #[tokio::test]
    async fn session_assets_are_served_hardened() {
        let hub = crate::web::DashboardHub::new(
            crate::web::SnapshotSource::Tui,
            "/workspace".to_string(),
        );
        let router = build_router(hub, "testtoken123".to_string(), None);
        for (uri, content_type, body) in [
            ("/assets/session.js", "text/javascript", SESSION_JS),
            ("/assets/session.css", "text/css", SESSION_CSS),
        ] {
            let req = axum::http::Request::builder()
                .uri(uri)
                .header("Host", "localhost:7420")
                .body(axum::body::Body::empty())
                .unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), axum::http::StatusCode::OK, "{uri}");
            let headers = resp.headers().clone();
            assert!(
                headers["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with(content_type)
            );
            assert_eq!(headers["content-security-policy"], "default-src 'self'");
            assert_eq!(headers["x-content-type-options"], "nosniff");
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(bytes.as_ref(), body.as_bytes());
        }
    }

    // TC-543: static_page_has_todo_panel (SPEC-web-dashboard UX-540)
    #[test]
    fn static_page_has_todo_panel() {
        assert!(INDEX_HTML.contains("id=\"todos\""));
        assert!(INDEX_HTML.contains("id=\"todo-form\""));
        assert!(APP_JS.contains("/api/todos"));
        // OQ-521: serve mode is view-only; the page explains how to run to-dos.
        assert!(INDEX_HTML.contains("id=\"todo-view-only\""));
        assert!(INDEX_HTML.contains("dmuxai --web"));
        assert!(APP_JS.contains("=== 'files'"));
    }

    // TC-607: static_page_has_theme_controls (SPEC-theme-colors UX-600, FR-603)
    #[tokio::test]
    async fn static_page_has_theme_controls() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let router = build_router(hub, "testtoken123".to_string(), None);

        async fn get(router: &axum::Router, uri: &str) -> String {
            let req = axum::http::Request::builder()
                .uri(uri)
                .header("Host", "localhost:7420")
                .body(axum::body::Body::empty())
                .unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), axum::http::StatusCode::OK);
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            String::from_utf8_lossy(&body).into_owned()
        }

        let html = get(&router, "/").await;
        let js = get(&router, "/assets/app.js").await;
        let css = get(&router, "/assets/app.css").await;

        // Header has a labeled Light/Dark/System control plus an accent picker
        // and a reset button.
        assert!(html.contains("id=\"theme-select\""));
        assert!(html.contains("for=\"theme-select\""));
        for value in ["light", "dark", "system"] {
            assert!(html.contains(&format!("value=\"{value}\"")));
        }
        assert!(html.contains("id=\"accent-picker\""));
        assert!(html.contains("type=\"color\""));
        assert!(html.contains("id=\"accent-reset\""));

        // JS applies the choice to <html data-theme=...> and persists it, and
        // "system" follows prefers-color-scheme.
        assert!(js.contains("dataset.theme"));
        // The token lives in sessionStorage only.
        assert!(js.contains("var TOKEN_KEY = 'dmuxai.token';"));
        assert!(js.contains("window.sessionStorage.getItem(key)"));
        assert!(js.contains("window.sessionStorage.setItem(key, value)"));
        assert!(js.contains("storageGet(TOKEN_KEY)"));
        // localStorage holds only the theme/accent keys and the graph view mode;
        // every set/remove targets one of them.
        assert!(js.contains("'dmuxai.theme'"));
        assert!(js.contains("'dmuxai.accent'"));
        assert!(js.contains("'dmuxai.graphView'"));
        let mut local_storage_writes = 0;
        for line in js.lines() {
            let line = line.trim();
            if line.contains("window.localStorage.setItem(")
                || line.contains("window.localStorage.removeItem(")
            {
                local_storage_writes += 1;
                assert!(
                    line.contains("THEME_KEY")
                        || line.contains("ACCENT_KEY")
                        || line.contains("GRAPH_VIEW_KEY"),
                    "unexpected localStorage key in app.js: {line}"
                );
            }
        }
        // setItem(THEME_KEY), setItem(ACCENT_KEY) and removeItem(ACCENT_KEY) at least.
        assert!(
            local_storage_writes >= 3,
            "expected localStorage writes for theme/accent only, found {local_storage_writes}"
        );
        // Accent values are guarded to #rrggbb before they touch the DOM.
        assert!(js.contains("/^#[0-9a-fA-F]{6}$/.test(value)"));
        // CSP: styles are set via element style / setProperty, no inline handlers.
        assert!(js.contains("setProperty"));
        assert!(!html.contains("onclick="));
        assert!(!html.contains("<style"));

        // CSS defines both palettes plus the pre-init OS default (no dark flash).
        assert!(css.contains(":root:not([data-theme])"));
        assert!(css.contains("prefers-color-scheme"));
        assert!(css.contains(":root[data-theme=\"dark\"]"));
        assert!(css.contains(":root[data-theme=\"light\"]"));
        // The palettes are chosen by data-theme set from JS, never hardcoded in HTML.
        assert!(!html.contains("data-theme"));
    }

    // TC-757: static_page_has_docs_panel (SPEC-docs-preview UX-753, INT-754)
    #[test]
    fn static_page_has_docs_panel() {
        // index only gains the docs stylesheet, the deferred docs module and
        // the docs panel section.
        assert!(INDEX_HTML.contains(r#"<link rel="stylesheet" href="/assets/docs.css">"#));
        assert!(INDEX_HTML.contains(r#"<script src="/assets/docs.js" defer></script>"#));
        assert!(INDEX_HTML.contains(r#"<section id="docs" class="panel">"#));

        // FR-746: the HTML preview iframe is set with an empty sandbox
        // attribute (setAttribute('sandbox', '')) and never grants scripts or
        // same-origin power.
        assert!(DOCS_JS.contains("setAttribute('sandbox', '')"));
        assert!(!DOCS_JS.contains("allow-same-origin"));
        assert!(!DOCS_JS.contains("allow-scripts"));

        // FR-743 render endpoints (GET by path and POST live-preview), the
        // 409 conflict path, and the concurrency base hash all appear.
        assert!(DOCS_JS.contains("/api/docs/render"));
        assert!(DOCS_JS.contains("409"));
        assert!(DOCS_JS.contains("base_sha256"));

        // Review round: the change log's output_tail is a JSON array of
        // lines and must be joined with real newlines before rendering.
        assert!(DOCS_JS.contains("Array.isArray(turn.output_tail)"));
        assert!(DOCS_JS.contains(".join('\\n')"));
        // The sanitized markdown must keep its .md-body wrapper so the
        // GitHub-like typography and hl-* rules in docs.css stay in effect
        // (finding 1), and only .md-body links are routed (finding 2, so
        // /docs/view and /docs/download anchors keep working).
        assert!(DOCS_JS.contains("body.className = 'md-body';"));
        assert!(DOCS_JS.contains("sanitizeInto(body, html);"));
        assert!(DOCS_JS.contains("a.closest('.md-body')"));
        // Ticket URLs carry only expiring /docs/view or /docs/download
        // capabilities; the dashboard token must never be woven into them.
        for line in DOCS_JS.lines() {
            if line.contains("/docs/view") || line.contains("/docs/download") {
                assert!(
                    !line.contains("token"),
                    "docs.js must not put the token into a ticket URL: {line}"
                );
            }
        }
    }

    // TC-759: docs_module_exposes_api (SPEC-docs-preview INT-754)
    #[test]
    fn docs_module_exposes_api() {
        assert!(DOCS_JS.contains("window.DmuxDocs = {"));
        for member in [
            "open: open",
            "close: close",
            "linkify: linkify",
            "changes: changes",
        ] {
            assert!(DOCS_JS.contains(member), "missing DmuxDocs member {member}");
        }
        assert!(DOCS_JS.contains("function sanitizeInto("));
        let inner_html_assigns = DOCS_JS.matches(".innerHTML =").count();
        assert_eq!(
            inner_html_assigns, 1,
            "server HTML must use a single innerHTML assignment inside sanitizeInto"
        );
    }

    // TC-833: docs_tree_sidebar_assets (SPEC-docs-tree FR-832, FR-833, UX-830)
    #[test]
    fn docs_tree_sidebar_assets() {
        // 1. index.html contains sidebar container, toggle button, role="tree", tabs
        assert!(INDEX_HTML.contains(r#"<aside id="sidebar" class="sidebar""#));
        assert!(INDEX_HTML.contains(r#"aria-controls="sidebar""#));
        assert!(INDEX_HTML.contains("Files"));
        assert!(INDEX_HTML.contains(r#"role="tree""#));
        assert!(INDEX_HTML.contains("tab-files"));

        // 2. docs.js contains lazy-expand fetch of /api/docs/tree, role="treeitem",
        // aria-expanded, localStorage persistence, keyboard handlers, and ticketUrl
        assert!(DOCS_JS.contains("/api/docs/tree"));
        assert!(DOCS_JS.contains("setAttribute('role', 'treeitem')"));
        assert!(DOCS_JS.contains("setAttribute('aria-expanded'"));
        assert!(DOCS_JS.contains("localStorage.getItem('dmuxai.sidebar."));
        assert!(DOCS_JS.contains("key === 'ArrowDown'"));
        assert!(DOCS_JS.contains("key === 'ArrowUp'"));
        assert!(DOCS_JS.contains("key === 'ArrowLeft'"));
        assert!(DOCS_JS.contains("key === 'ArrowRight'"));
        assert!(DOCS_JS.contains("key === 'Enter'"));
        assert!(DOCS_JS.contains("function ticketUrl("));
        // Review fixes: changes panel always refreshes, nested focus restore
        // never steals focus, filter reports truncation, failed loads retry.
        assert!(DOCS_JS.contains("pendingFocusPath"));
        assert!(DOCS_JS.contains("mayRestore"));
        assert!(DOCS_JS.contains("file list truncated"));
        assert!(DOCS_JS.contains("documents unavailable: "));
        assert!(DOCS_JS.contains("group._failed"));

        // 3. docs.css contains sidebar container and narrow-width drawer media query
        assert!(DOCS_CSS.contains(".sidebar"));
        assert!(DOCS_CSS.contains("@media (max-width: 900px)"));
    }

    // TC-608: theme_palettes_meet_wcag_contrast (SPEC-theme-colors NFR-600)
    #[test]
    fn theme_palettes_meet_wcag_contrast() {
        type Rgb = [u8; 3];

        fn palette_vars(css: &str, theme: &str) -> Vec<(String, Rgb)> {
            // The dark palette also applies to :root:not([data-theme]) (pre-init
            // OS default), so accept selector lists before the brace.
            let marker = format!(":root[data-theme=\"{theme}\"]");
            let start = css.find(&marker).unwrap_or_else(|| {
                panic!("missing :root[data-theme=\"{theme}\"] palette in APP_CSS")
            });
            let open = css[start + marker.len()..]
                .find('{')
                .expect("palette block is missing {{")
                + start
                + marker.len();
            let body = &css[open + 1..];
            let end = body.find('}').expect("palette block is not closed");

            let mut vars = Vec::new();
            for line in body[..end].lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if !line.starts_with("--") {
                    // Plain declarations such as `color-scheme: dark;` are allowed.
                    continue;
                }
                // Palette variables must be #rrggbb; anything else fails the test
                // instead of being silently skipped by the parser.
                let (name, value) = line
                    .split_once(':')
                    .unwrap_or_else(|| panic!("palette variable has no value: {line:?}"));
                let hex = value.trim().trim_end_matches(';').trim();
                let hex = hex
                    .strip_prefix('#')
                    .unwrap_or_else(|| panic!("palette values must be #rrggbb hex, got {line:?}"));
                assert!(
                    hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()),
                    "palette values must be #rrggbb hex, got {line:?}"
                );
                vars.push((
                    name.trim_start_matches("--").trim().to_string(),
                    [
                        u8::from_str_radix(&hex[0..2], 16).unwrap(),
                        u8::from_str_radix(&hex[2..4], 16).unwrap(),
                        u8::from_str_radix(&hex[4..6], 16).unwrap(),
                    ],
                ));
            }
            vars
        }

        fn var_value(vars: &[(String, Rgb)], name: &str) -> Rgb {
            vars.iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("missing --{name} in palette"))
                .1
        }

        // WCAG 2.1 relative luminance and contrast ratio.
        fn linear(c: u8) -> f64 {
            let c = f64::from(c) / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }

        fn luminance(rgb: Rgb) -> f64 {
            0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
        }

        fn contrast(a: Rgb, b: Rgb) -> f64 {
            let (lo, hi) = (luminance(a), luminance(b));
            let (lo, hi) = if lo > hi { (hi, lo) } else { (lo, hi) };
            (hi + 0.05) / (lo + 0.05)
        }

        let statuses = [
            "pending",
            "ready",
            "running",
            "waiting_gate",
            "completed",
            "failed",
            "skipped",
        ];

        for theme in ["dark", "light"] {
            let vars = palette_vars(APP_CSS, theme);

            // NFR-600: body text and muted text on the background >= 4.5:1.
            let bg = var_value(&vars, "bg");
            assert!(
                contrast(var_value(&vars, "text"), bg) >= 4.5,
                "{theme}: --text/--bg below 4.5"
            );
            assert!(
                contrast(var_value(&vars, "muted"), bg) >= 4.5,
                "{theme}: --muted/--bg below 4.5"
            );

            // NFR-600: badge text on its badge background >= 3:1.
            for status in statuses {
                let text = var_value(&vars, &format!("badge-{status}-text"));
                let badge_bg = var_value(&vars, &format!("badge-{status}-bg"));
                assert!(
                    contrast(text, badge_bg) >= 3.0,
                    "{theme}: badge-{status} text below 3.0"
                );
            }

            // NFR-600: button text on the button background >= 3:1.
            let btn_bg = var_value(&vars, "btn-bg");
            assert!(contrast(var_value(&vars, "btn-text"), btn_bg) >= 3.0);
            assert!(contrast(var_value(&vars, "btn-primary-text"), btn_bg) >= 3.0);

            // NFR-600: text on the accent is black or white, whichever gives
            // the higher WCAG contrast.
            let accent = var_value(&vars, "accent");
            let on_accent = var_value(&vars, "on-accent");
            assert!(on_accent == [0, 0, 0] || on_accent == [255, 255, 255]);
            let best = contrast([0, 0, 0], accent).max(contrast([255, 255, 255], accent));
            assert!(
                (contrast(on_accent, accent) - best).abs() < 1e-9,
                "{theme}: --on-accent is not the higher-contrast choice on --accent"
            );

            // UX-561: Jira chip text on its chip background >= 4.5:1 and the
            // key link on the strip background >= 4.5:1 (both palettes).
            for chip in ["todo", "progress", "done"] {
                let chip_text = var_value(&vars, &format!("jira-chip-{chip}-text"));
                let chip_bg = var_value(&vars, &format!("jira-chip-{chip}-bg"));
                assert!(
                    contrast(chip_text, chip_bg) >= 4.5,
                    "{theme}: jira chip {chip} text below 4.5"
                );
            }
            let strip_bg = var_value(&vars, "jira-strip-bg");
            assert!(
                contrast(var_value(&vars, "jira-key"), strip_bg) >= 4.5,
                "{theme}: --jira-key on --jira-strip-bg below 4.5"
            );
        }

        // NFR-600: the JS on-accent algorithm must compare both black and white
        // contrast ratios, not hardcode one of them (structural check).
        assert!(APP_JS.contains("var onBlack"));
        assert!(APP_JS.contains("var onWhite"));
        assert!(APP_JS.contains("onBlack >= onWhite ? '#000000' : '#ffffff'"));
        // Relative luminance must weight the sRGB channels, not assume gray.
        assert!(APP_JS.contains("0.2126"));
        assert!(APP_JS.contains("0.7152"));
        assert!(APP_JS.contains("0.0722"));

        // Table-driven mirror of app.js onAccentFor/relativeLuminance/contrastRatio:
        // keep the two implementations in sync (update app.js and this table
        // together) so a future JS change cannot silently regress NFR-600.
        fn js_linear(c: f64) -> f64 {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }

        fn js_relative_luminance(hex: &str) -> f64 {
            let channel = |lo: usize| -> f64 {
                js_linear(u8::from_str_radix(&hex[1 + lo..3 + lo], 16).unwrap() as f64 / 255.0)
            };
            0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4)
        }

        fn js_on_accent(hex: &str) -> &'static str {
            let l = js_relative_luminance(hex);
            let on_black = (l + 0.05) / 0.05;
            let on_white = 1.05 / (l + 0.05);
            if on_black >= on_white {
                "#000000"
            } else {
                "#ffffff"
            }
        }

        // Hand-verified expected choices: dark accents get white text, light
        // accents black; one row sits near the black/white flip point.
        let accents = [
            ("#0b57d0", "#ffffff"),
            ("#0a3d91", "#ffffff"),
            ("#4aa8ff", "#000000"),
            ("#ff8800", "#000000"),
            ("#808080", "#000000"),
            ("#757575", "#ffffff"),
            ("#000000", "#ffffff"),
            ("#ffffff", "#000000"),
        ];
        for (accent_hex, expected) in accents {
            assert_eq!(js_on_accent(accent_hex), expected, "accent {accent_hex}");
            // The chosen text color is exactly the higher-contrast side.
            let l = js_relative_luminance(accent_hex);
            let best = ((l + 0.05) / 0.05).max(1.05 / (l + 0.05));
            let chosen_black = expected == "#000000";
            let chosen_ratio = if chosen_black {
                (l + 0.05) / 0.05
            } else {
                1.05 / (l + 0.05)
            };
            assert!((chosen_ratio - best).abs() < 1e-9, "accent {accent_hex}");
            // Any accent keeps text readable: the better side is always >= ~4.58:1.
            assert!(best >= 4.5, "accent {accent_hex}");
        }
    }

    // TC-572: static_page_has_todo_links (SPEC-web-dashboard UX-561)
    #[test]
    fn static_page_has_todo_links() {
        assert!(INDEX_HTML.contains("id=\"todos\""));
        assert!(INDEX_HTML.contains("id=\"todo-list\""));
        // UX-561: form has the Jira ticket field and preview element.
        assert!(INDEX_HTML.contains("id=\"todo-jira\""));
        assert!(INDEX_HTML.contains("id=\"todo-jira-preview\""));
        // JS builds the strip with the key link (noopener + http/https check).
        assert!(APP_JS.contains("'todo-jira-strip'"));
        assert!(APP_JS.contains("noopener noreferrer"));
        assert!(APP_JS.contains("'^https?:\\\\/\\\\/.+'i") || APP_JS.contains("https?:"));
        // Status chip category classes (todo/progress/done/neutral).
        assert!(APP_JS.contains("todo-jira-chip-todo"));
        assert!(APP_JS.contains("todo-jira-chip-progress"));
        assert!(APP_JS.contains("todo-jira-chip-done"));
        assert!(APP_JS.contains("todo-jira-chip-neutral"));
        // Assignee or Unassigned, imported/manual tag, dashed + Jira affordance.
        assert!(APP_JS.contains("'Unassigned'"));
        assert!(APP_JS.contains("'imported'"));
        assert!(APP_JS.contains("manual"));
        assert!(APP_JS.contains("'+ Jira'"));
        assert!(APP_CSS.contains("todo-jira-strip"));
        assert!(APP_CSS.contains(".todo-jira-chip-todo"));
        // The collapsed "+ Jira" form must stay hidden (display rule must not
        // override the `hidden` attribute).
        assert!(APP_CSS.contains(".todo-jira-form[hidden]"));
    }

    // TC-568: static_page_has_import_panel (SPEC-web-dashboard UX-560)
    #[test]
    fn static_page_has_import_panel() {
        assert!(INDEX_HTML.contains("id=\"import-form\""));
        assert!(APP_JS.contains("/api/import"));
        assert!(APP_JS.contains("/api/mcp/servers"));
    }

    // TC-704 / UX-700..UX-703, NFR-700: the embedded page builds the inline SVG
    // subway map (g.node[data-id] stations, path.edge lines, state- classes), a
    // percentage progress bar with a timing-guarded ETA, a persisted List/Graph
    // toggle, a running pulse with a reduced-motion override, and no external
    // resources.
    #[test]
    fn graph_view_svg_structure() {
        // The renderer creates SVG nodes in the SVG namespace.
        assert!(APP_JS.contains("document.createElementNS(SVG_NS"));
        assert!(APP_JS.contains("var SVG_NS = 'http://www.w3.org/2000/svg';"));
        // Stations: g.node[data-id] with circle stations and state- classes,
        // focusable and labelled.
        assert!(APP_JS.contains("svgEl('g')"));
        assert!(APP_JS.contains("'node state-'"));
        assert!(APP_JS.contains("svgEl('circle')"));
        assert!(APP_JS.contains("setAttribute('data-id'"));
        assert!(APP_JS.contains("setAttribute('role', 'button')"));
        assert!(APP_JS.contains("setAttribute('aria-label'"));
        assert!(APP_JS.contains("svgEl('title')"));
        assert!(APP_JS.contains("function nodeTitleText"));
        assert!(APP_JS.contains("svgEl('path')"));
        // Edges: path.edge lines colored by the source state.
        assert!(APP_JS.contains("'edge edge-'"));
        // Toggle default and persistence.
        assert!(APP_JS.contains("var GRAPH_VIEW_KEY = 'dmuxai.graphView';"));
        assert!(APP_JS.contains("window.localStorage.setItem(GRAPH_VIEW_KEY"));
        assert!(APP_JS.contains("layers > 1 ? 'graph' : 'list'"));
        // Progress bar percentage plus a timing-guarded ETA.
        assert!(APP_JS.contains("$('graph-percent')"));
        assert!(APP_JS.contains("Math.round((completed / total) * 100) + '%'"));
        assert!(APP_JS.contains("estimateEtaMs"));
        assert!(APP_JS.contains("finished_at_ms"));
        assert!(APP_JS.contains("$('graph-eta')"));
        // Pan/zoom and the details panel.
        assert!(APP_JS.contains("GRAPH_MIN_SCALE"));
        assert!(APP_JS.contains("GRAPH_MAX_SCALE"));
        assert!(APP_JS.contains("onGraphWheel"));
        assert!(APP_JS.contains("selectGraphNode"));
        assert!(APP_JS.contains("closeGraphDetail"));
        assert!(APP_JS.contains("'Escape'"));

        // The HTML carries the containers the renderer fills.
        assert!(INDEX_HTML.contains("id=\"graph-canvas\""));
        assert!(INDEX_HTML.contains("id=\"graph-view-toggle\""));
        assert!(INDEX_HTML.contains("id=\"view-list\""));
        assert!(INDEX_HTML.contains("id=\"view-graph\""));
        assert!(INDEX_HTML.contains("id=\"graph-detail\""));
        assert!(INDEX_HTML.contains("id=\"graph-fit\""));
        assert!(INDEX_HTML.contains("class=\"graph-legend\""));

        // CSS: state classes, edge colors, pulse keyframes, reduced-motion.
        assert!(APP_CSS.contains(".graph-svg .node.state-completed .station"));
        assert!(APP_CSS.contains(".graph-svg .node.state-running .ring"));
        assert!(APP_CSS.contains("@keyframes dmuxai-node-pulse"));
        assert!(APP_CSS.contains("@media (prefers-reduced-motion: reduce)"));
        assert!(APP_CSS.contains(".graph-svg .edge-completed"));
        assert!(APP_CSS.contains(".graph-svg .edge-failed"));
        assert!(APP_CSS.contains("var(--ok)"));
        assert!(APP_CSS.contains("var(--accent)"));
        assert!(APP_CSS.contains("var(--err)"));

        // CSP-safe: no inline handlers and no <style> blocks in the page.
        assert!(!INDEX_HTML.contains("onclick="));
        assert!(!INDEX_HTML.contains("<style"));

        // No external resource URLs (CDN scripts/styles/fonts/images).
        for asset in [INDEX_HTML, APP_JS, APP_CSS] {
            for needle in [
                "src=\"http",
                "src='http",
                "href=\"http",
                "href='http",
                "@import",
                "url(http",
            ] {
                assert!(
                    !asset.contains(needle),
                    "unexpected external resource reference {needle:?}"
                );
            }
        }
        // app.js only mentions http twice: the inert SVG namespace and the
        // localhost example in the missing-token message.
        assert_eq!(
            APP_JS.matches("http://").count(),
            2,
            "unexpected http:// URLs in app.js"
        );
    }

    // UX-700/UX-703: bar segment order, station sublabels, auto-fit and lane
    // routing match mockup #5.
    #[test]
    fn graph_view_progress_labels_fit_and_routing() {
        // Progress bar order: completed, running, failed, then pending last.
        assert!(APP_JS.contains(
            "'completed', 'running', 'failed', 'pending', 'ready', 'waiting_gate', 'skipped'"
        ));

        // Station sublabels: short runner names and a state word when elapsed is
        // unknown, plus the failed-attempt suffix.
        assert!(APP_JS.contains("function runnerShortName"));
        assert!(APP_JS.contains("replace(/^opencode-/i, '')"));
        assert!(APP_JS.contains("=== 'agy'") || APP_JS.contains("=== \"agy\""));
        assert!(APP_JS.contains("STATION_STATE_WORDS"));
        assert!(APP_JS.contains("completed: 'done'"));
        assert!(APP_JS.contains("waiting_gate: 'waiting'"));
        assert!(APP_JS.contains("stationStateWord"));
        assert!(APP_JS.contains(r"'\u00d7' + node.attempt"));

        // Auto-fit on first render / node-set change; user pan/zoom preserved.
        assert!(APP_JS.contains("function autoFitGraph"));
        assert!(APP_JS.contains("function graphGeometryKey"));
        assert!(APP_JS.contains("function syncGraphCanvas"));
        assert!(APP_JS.contains("function updateRunningGraphStationMeta"));
        assert!(APP_JS.contains("function tickDashboard"));
        assert!(APP_JS.contains("function graphCanvasHeight"));
        assert!(APP_JS.contains("graphUserInteracted"));

        // Long edges detour onto a half-row lane; short edges stay straight.
        assert!(APP_JS.contains("function longEdgePath"));
        assert!(APP_JS.contains("GRAPH_ROW_H / 2"));
        assert!(APP_JS.contains("span > 1"));

        // Canvas bounds.
        assert!(APP_CSS.contains("min-height: 320px;"));
        assert!(APP_CSS.contains("max-height: 70vh;"));
    }

    #[test]
    // TC-823 / FR-823, UX-821, NFR-820: dashboard_has_run_status_indicators
    fn dashboard_has_run_status_indicators() {
        assert!(INDEX_HTML.contains("id=\"run-status\""));
        assert!(INDEX_HTML.contains("role=\"status\""));
        assert!(INDEX_HTML.contains("id=\"favicon\""));
        assert!(APP_CSS.contains(".run-status"));
        assert!(APP_CSS.contains(".spinner"));
        assert!(APP_CSS.contains("@media (prefers-reduced-motion: reduce)"));
        assert!(APP_CSS.contains(".station-spinner"));
        assert!(APP_CSS.contains(".chip-failed"));
        assert!(APP_CSS.contains(".chip-cancelled"));
        // The reduced-motion block must come after the base spinner rules so
        // it wins the cascade, and it must stop the graph spinner.
        let base_spinner = APP_CSS
            .find(".graph-svg .station-spinner {")
            .expect("base station-spinner rule");
        let rm_block = APP_CSS
            .rfind("@media (prefers-reduced-motion: reduce) {")
            .expect("reduced-motion block");
        assert!(
            rm_block > base_spinner,
            "reduced-motion block must follow spinner rules"
        );
        let rm_tail = &APP_CSS[rm_block..];
        let rm_spinner = rm_tail
            .find(".graph-svg .station-spinner {")
            .expect("reduced-motion overrides station-spinner");
        assert!(
            rm_tail[rm_spinner..]
                .trim_start_matches(".graph-svg .station-spinner {")
                .trim_start()
                .starts_with("animation: none")
        );
        assert!(APP_CSS.contains(".reduced-motion .graph-svg .station-spinner"));
        assert!(APP_JS.contains("classList.toggle('reduced-motion'"));
        // Pill and chip elapsed use the same clock-skew correction as nodes.
        assert!(APP_JS.contains("function runElapsedMs"));
        assert!(!APP_JS.contains("now - a.run.started_at_ms"));
        assert!(APP_JS.contains("function tickDashboard"));
        assert!(APP_JS.contains("station-spinner"));
        assert!(APP_JS.contains("updateFavicon"));
        assert!(APP_JS.contains("baseDocumentTitle"));
        assert!(APP_JS.contains("window.addEventListener('focus'"));
        assert!(APP_JS.contains("started_at_ms"));
        assert!(APP_JS.contains("document.title"));
        assert!(APP_JS.contains("running"));
        assert!(APP_JS.contains("done"));
        assert!(APP_JS.contains("failed"));
        assert!(APP_JS.contains("cancelled"));
    }
}
