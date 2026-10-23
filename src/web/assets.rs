pub const INDEX_HTML: &str = include_str!("assets/index.html");
pub const APP_JS: &str = include_str!("assets/app.js");
pub const MEMORY_JS: &str = include_str!("assets/memory.js");
pub const SKILLS_JS: &str = include_str!("assets/skills.js");
pub const AGENT_PROMPTS_JS: &str = include_str!("assets/agent-prompts.js");
pub const SESSION_JS: &str = include_str!("assets/session.js");
pub const WORKSPACE_JS: &str = include_str!("assets/workspace.js");
pub const SIDEBAR_RESIZE_JS: &str = include_str!("assets/sidebar-resize.js");
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
        assert_ne!(APP_CSS.len(), 0);

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
            // Only images may also come from blob: object URLs (chat attachments).
            "default-src 'self'; img-src 'self' blob:"
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
        assert!(INDEX_HTML.contains("id=\"memory-toggle\""));
        assert!(INDEX_HTML.contains("id=\"memory-popover\""));
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
        // UX-780: composer agent picker and Cancel while busy.
        assert!(SESSION_JS.contains("session-agent-picker"));
        assert!(SESSION_JS.contains("ui.agentPicker.disabled = viewOnly"));
        assert!(SESSION_JS.contains("ui.cancel.hidden = !busy"));
        assert!(SESSION_JS.contains("/api/command"));
        assert!(SESSION_JS.contains("/api/prompt"));
        // CSP: no inline handlers or HTML injection.
        assert!(SESSION_JS.contains("content_html"));
        assert!(SESSION_JS.contains("msg-md"));
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
        assert!(
            INDEX_HTML
                .contains(r#"<section id="docs" class="panel panel-collapsible center-view""#)
        );

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

    // TC-840: todo_sidebar_assets (SPEC-todo-sidebar FR-840..FR-842, UX-840)
    #[test]
    fn todo_sidebar_assets() {
        // Header button with badge, wired to the shared sidebar.
        assert!(INDEX_HTML.contains(r#"id="todos-toggle""#));
        assert!(INDEX_HTML.contains(r#"id="todos-count""#));
        // UX-840: the count is part of the button's accessible name.
        assert!(INDEX_HTML.contains(r#"aria-label="To-dos, 0 open""#));
        assert!(APP_JS.contains("'To-dos, ' + open + ' open'"));
        let toggle_start = INDEX_HTML.find(r#"id="todos-toggle""#).unwrap();
        let toggle_tag = &INDEX_HTML[INDEX_HTML[..toggle_start].rfind('<').unwrap()..];
        let toggle_tag = &toggle_tag[..toggle_tag.find('>').unwrap()];
        assert!(toggle_tag.contains(r#"aria-controls="sidebar""#));
        assert!(toggle_tag.contains(r#"aria-expanded="false""#));
        // The whole to-do UI lives inside the To-dos tab panel of the sidebar.
        assert!(INDEX_HTML.contains(r#"id="tab-todos""#));
        let panel = INDEX_HTML
            .find(r#"id="tab-panel-todos""#)
            .expect("to-dos tab panel");
        let sidebar = INDEX_HTML.find(r#"<aside id="sidebar""#).unwrap();
        let sidebar_end = INDEX_HTML[sidebar..].find("</aside>").unwrap() + sidebar;
        assert!(panel > sidebar && panel < sidebar_end);
        for id in [
            "todo-form",
            "todo-title",
            "todo-jira",
            "import-panel",
            "todo-list",
            "todo-filter-state",
            "todo-search",
            "todo-view-only",
        ] {
            let at = INDEX_HTML
                .find(&format!(r#"id="{id}""#))
                .unwrap_or_else(|| panic!("missing #{id}"));
            assert!(
                at > panel && at < sidebar_end,
                "#{id} must be in the to-dos panel"
            );
        }
        assert!(!INDEX_HTML.contains(r#"<section id="todos""#));
        // State filter and search.
        assert!(INDEX_HTML.contains(r#"<option value="done">"#));
        assert!(INDEX_HTML.contains(r#"<option value="all">"#));
        assert!(APP_JS.contains("function todoMatchesFilter("));
        assert!(APP_JS.contains("function updateTodoCount("));
        assert!(APP_JS.contains("bindTodoFilter();"));
        // Tabs and Esc handling in the shared sidebar shell.
        assert!(DOCS_JS.contains("function setActiveTab("));
        assert!(DOCS_JS.contains("function toggleSidebarTab("));
        assert!(DOCS_JS.contains("dmuxai.sidebar.tab."));
        assert!(DOCS_JS.contains("function escapeClosesSidebar("));
        assert!(DOCS_JS.contains("tag === 'TEXTAREA'"));
        assert!(DOCS_JS.contains(r#"[aria-modal="true"]"#));
        // Hidden tab panels really hide (the panel class sets display: flex).
        assert!(DOCS_CSS.contains(".sidebar-tab-panel[hidden]"));
        assert!(DOCS_CSS.contains(".todos-count"));
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

            // SPEC-graph-swimlane UX-880: pending card borders and dashed
            // arrows >= 3:1, and graph text >= 4.5:1, on the canvas, the card
            // fill and the lane label background.
            for surface in ["graph-canvas", "graph-node-fill", "bg-chip"] {
                let bg = var_value(&vars, surface);
                assert!(
                    contrast(var_value(&vars, "graph-pending"), bg) >= 3.0,
                    "{theme}: --graph-pending/--{surface} below 3.0"
                );
                for text in ["text", "muted", "faint"] {
                    assert!(
                        contrast(var_value(&vars, text), bg) >= 4.5,
                        "{theme}: --{text}/--{surface} below 4.5"
                    );
                }
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

    // TC-704 / UX-702..UX-703, NFR-700 (drawing per SPEC-graph-swimlane): the
    // embedded page builds the inline SVG timeline (g.node[data-id] cards,
    // path.edge lines, state- classes), a
    // percentage progress bar with a timing-guarded ETA, a persisted List/Graph
    // toggle, a running pulse with a reduced-motion override, and no external
    // resources.
    #[test]
    fn graph_view_svg_structure() {
        // The renderer creates SVG nodes in the SVG namespace.
        assert!(APP_JS.contains("document.createElementNS(SVG_NS"));
        assert!(APP_JS.contains("var SVG_NS = 'http://www.w3.org/2000/svg';"));
        // Cards: g.node[data-id] with state- classes, focusable and labelled.
        assert!(APP_JS.contains("svgEl('g')"));
        assert!(APP_JS.contains("'node card state-'"));
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
        assert!(APP_CSS.contains(".graph-svg .node.state-completed .card-box"));
        assert!(APP_CSS.contains(".graph-svg .node.state-running .card-box"));
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

    // UX-702/UX-703: bar segment order, card state words, auto-fit, and the
    // canvas bounds of the swimlane view.
    #[test]
    fn graph_view_progress_labels_fit_and_routing() {
        // Progress bar order: completed, running, failed, then pending last.
        assert!(APP_JS.contains(
            "'completed', 'running', 'failed', 'pending', 'ready', 'waiting_gate', 'skipped'"
        ));

        // Short runner names in the detail panel, full names on lanes, and
        // state words on the card chips.
        assert!(APP_JS.contains("function runnerShortName"));
        assert!(APP_JS.contains("replace(/^opencode-/i, '')"));
        assert!(APP_JS.contains("=== 'agy'"));
        assert!(APP_JS.contains("function runnerLaneName"));
        assert!(APP_JS.contains("STATION_STATE_WORDS"));
        assert!(APP_JS.contains("completed: 'done'"));
        assert!(APP_JS.contains("waiting_gate: 'waiting'"));
        assert!(APP_JS.contains("stationStateWord"));

        // Auto-fit on first render; user pan/zoom preserved; live rebuilds.
        assert!(APP_JS.contains("function autoFitGraph"));
        assert!(APP_JS.contains("function graphGeometryKey"));
        assert!(APP_JS.contains("function syncGraphCanvas"));
        assert!(APP_JS.contains("function updateRunningGraphCards"));
        assert!(APP_JS.contains("function tickDashboard"));
        assert!(APP_JS.contains("function graphCanvasHeight"));
        assert!(APP_JS.contains("graphUserInteracted"));

        // Canvas bounds.
        assert!(APP_CSS.contains("min-height: 180px;"));
        assert!(APP_CSS.contains("max-height: 70vh;"));
    }

    // TC-880 (SPEC-graph-swimlane FR-880, FR-881, FR-883, FR-885): the
    // swimlane renderer pieces exist, the subway station code is gone, and the
    // detail panel is an overlay drawer.
    #[test]
    fn graph_swimlane_assets() {
        for needle in [
            "function buildSwimlaneGeometry",
            "function graphEstimate",
            "function graphTickStep",
            "function graphArrowPath",
            "function graphFreeColumn",
            "function graphCard(",
            "function graphServerNow",
            "LANE_LABEL_W = 170",
            "LANE_ROW_H = 104",
            "CARD_H = 56",
            "SUBROW_GAP = 24",
            "ARROW_MIN_GAP = 24",
            "GRAPH_TEXT_FLOOR = 11",
            "'lane lane-c'",
            "'card-tail'",
            "'now-line'",
            "'graph-future'",
            "est. start +",
            "window.dmuxaiGraph",
        ] {
            assert!(APP_JS.contains(needle), "app.js is missing {needle:?}");
        }
        for gone in [
            "function graphStation",
            "function longEdgePath",
            "station-spinner",
        ] {
            assert!(
                !APP_JS.contains(gone),
                "subway code left in app.js: {gone:?}"
            );
        }
        assert!(!APP_CSS.contains(".station"), "subway CSS left in app.css");
        // FR-885: overlay drawer, not a flex sibling that squeezes the canvas.
        let detail = APP_CSS
            .find("\n.graph-detail {")
            .expect("graph-detail rule");
        let rule = &APP_CSS[detail..detail + 400];
        assert!(rule.contains("position: absolute;"));
        assert!(rule.contains("right: 0;"));
        // Running tail, pulsing dot and its reduced-motion override.
        assert!(APP_CSS.contains(".graph-svg .stripe"));
        assert!(APP_CSS.contains(".graph-svg .run-dot"));
        assert!(APP_CSS.contains(".reduced-motion .graph-svg .run-dot"));
        // UX-880: dedicated pending color in both palettes.
        assert!(APP_CSS.matches("--graph-pending:").count() >= 3);
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
        assert!(APP_CSS.contains(".graph-svg .run-dot"));
        assert!(APP_CSS.contains(".chip-failed"));
        assert!(APP_CSS.contains(".chip-cancelled"));
        // The reduced-motion block must come after the base spinner rules so
        // it wins the cascade, and it must stop the graph spinner.
        let base_spinner = APP_CSS
            .find(".graph-svg .run-dot {")
            .expect("base run-dot rule");
        let rm_block = APP_CSS
            .rfind("@media (prefers-reduced-motion: reduce) {")
            .expect("reduced-motion block");
        assert!(
            rm_block > base_spinner,
            "reduced-motion block must follow spinner rules"
        );
        let rm_tail = &APP_CSS[rm_block..];
        let rm_spinner = rm_tail
            .find(".graph-svg .run-dot {")
            .expect("reduced-motion overrides run-dot");
        assert!(
            rm_tail[rm_spinner..]
                .trim_start_matches(".graph-svg .run-dot {")
                .trim_start()
                .starts_with("animation: none")
        );
        assert!(APP_CSS.contains(".reduced-motion .graph-svg .run-dot"));
        assert!(APP_JS.contains("classList.toggle('reduced-motion'"));
        // Pill and chip elapsed use the same clock-skew correction as nodes.
        assert!(APP_JS.contains("function runElapsedMs"));
        assert!(!APP_JS.contains("now - a.run.started_at_ms"));
        assert!(APP_JS.contains("function tickDashboard"));
        assert!(APP_JS.contains("run-dot"));
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

    #[test]
    // TC-983 / FR-981: dashboard_shows_agent_activity_detail
    fn dashboard_shows_agent_activity_detail() {
        let key_at = APP_JS
            .find("function computeAgentsKey")
            .expect("computeAgentsKey");
        let render_at = APP_JS.find("function renderAgents").expect("renderAgents");
        let status_at = APP_JS
            .find("function updateRunStatus")
            .expect("updateRunStatus");
        let tab_at = APP_JS
            .find("function updateTabTitle")
            .expect("updateTabTitle");
        assert!(key_at < render_at && render_at < status_at && status_at < tab_at);

        let key_body = &APP_JS[key_at..render_at];
        assert!(
            key_body.contains("a.activity"),
            "computeAgentsKey must include a.activity"
        );

        let chip_body = &APP_JS[render_at..status_at];
        assert!(
            chip_body.contains("agent.activity"),
            "agent chip must render agent.activity"
        );

        let pill_body = &APP_JS[status_at..tab_at];
        assert!(
            pill_body.contains("agent.activity"),
            "run-status pill must render agent.activity"
        );
        assert!(pill_body.contains("run-status-activity"));
        assert!(pill_body.contains("textContent"));
        assert!(!pill_body.contains("innerHTML"));
        assert!(APP_CSS.contains(".run-status-activity"));
    }

    // TC-1653 / FR-1653: header shows Waiting for approval with theme-safe styles.
    #[test]
    fn tc_1653_waiting_for_approval_status_assets() {
        assert!(APP_CSS.contains(".run-status-awaiting-approval"));
        assert!(APP_CSS.contains("--badge-waiting_gate-text"));
        assert!(APP_JS.contains("turn.state === 'awaiting_approval'"));
        assert!(APP_JS.contains("Waiting for approval"));
        assert!(SESSION_JS.contains("awaiting_approval"));
    }

    // TC-850: prompt_centering_assets (SPEC-centered-prompt FR-850, UX-850)
    #[test]
    fn prompt_centering_assets() {
        // Dock custom properties default to 0px. Above 900px an open sidebar
        // reserves its width. The default width is still 320px (FR-851); the
        // dock follows --left/right-sidebar-width so a resize stays centered.
        assert!(DOCS_CSS.contains("--left-dock: 0px;"));
        assert!(DOCS_CSS.contains("--right-dock: 0px;"));
        assert!(DOCS_CSS.contains("--left-sidebar-width: 320px;"));
        assert!(DOCS_CSS.contains("--right-sidebar-width: 320px;"));
        assert!(DOCS_CSS.contains("@media (min-width: 901px)"));
        assert!(DOCS_CSS.contains("body.left-sidebar-open"));
        assert!(DOCS_CSS.contains("--left-dock: var(--left-sidebar-width);"));
        assert!(DOCS_CSS.contains("body.sidebar-open"));
        assert!(DOCS_CSS.contains("--right-dock: var(--right-sidebar-width);"));

        // Dock space is reserved on body and main stays centered via margin-inline: auto.
        assert!(DOCS_CSS.contains("padding-left: var(--left-dock);"));
        assert!(DOCS_CSS.contains("padding-right: var(--right-dock);"));
        assert!(DOCS_CSS.contains("margin-inline: auto;"));
        // SPEC-dashboard-fullwidth FR-1200 replaced the 880px column.
        assert!(!APP_CSS.contains("max-width: 880px;"));

        // The old per-state margin rule is gone.
        assert!(!DOCS_CSS.contains("margin-right: 320px"));
        assert!(!APP_CSS.contains("margin-right: 320px"));

        // Transition and reduced-motion override (media query + .reduced-motion class).
        assert!(DOCS_CSS.contains("transition: padding 0.2s ease-in-out;"));
        assert!(DOCS_CSS.contains("@media (prefers-reduced-motion: reduce)"));
        assert!(DOCS_CSS.contains(".reduced-motion body"));
        assert!(DOCS_CSS.contains("transition: none;"));
    }

    // TC-862: session_sidebar_assets (SPEC-session-sidebar FR-861, FR-862, FR-863, UX-860)
    #[test]
    fn session_sidebar_assets() {
        assert!(INDEX_HTML.contains("id=\"sessions-toggle\""));
        assert!(INDEX_HTML.contains("aria-controls=\"sessions-sidebar\""));
        assert!(INDEX_HTML.contains("id=\"sessions-sidebar\""));
        assert!(INDEX_HTML.contains("aria-label=\"Sessions\""));
        assert!(INDEX_HTML.contains("data-parity=\"conversations\""));
        assert!(INDEX_HTML.contains("id=\"sessions-new\""));
        assert!(INDEX_HTML.contains("id=\"sessions-filter\""));
        assert!(INDEX_HTML.contains("id=\"sessions-list\""));

        // Toggle button appears before the Files button in the header.
        let sessions_pos = INDEX_HTML
            .find("id=\"sessions-toggle\"")
            .expect("sessions-toggle button");
        let files_pos = INDEX_HTML
            .find("id=\"sidebar-toggle-files\"")
            .expect("sidebar-toggle-files button");
        assert!(
            sessions_pos < files_pos,
            "#sessions-toggle must appear before #sidebar-toggle-files"
        );

        // Old in-panel aside.session-conversations is removed.
        assert!(!SESSION_JS.contains("session-conversations"));
        assert!(!SESSION_CSS.contains(".session-conversations"));
        assert!(!SESSION_CSS.contains("minmax(0, 1fr) 240px"));

        // Sidebar wiring, persistence, and reduced-motion spinner.
        assert!(SESSION_JS.contains("dmuxai.sessions.open."));
        assert!(SESSION_JS.contains("left-sidebar-open"));
        assert!(SESSION_JS.contains("aria-current"));
        assert!(SESSION_JS.contains("load_conversation"));
        assert!(SESSION_JS.contains("new_conversation"));
        assert!(SESSION_JS.contains("spinner"));
        assert!(DOCS_CSS.contains(".sessions-sidebar"));
        assert!(APP_CSS.contains(".spinner"));
        assert!(APP_CSS.contains("@media (prefers-reduced-motion: reduce)"));
    }

    // TC-872: workspace_dir_assets (SPEC-workspace-dir FR-871, FR-872)
    #[tokio::test]
    async fn workspace_dir_assets() {
        assert!(INDEX_HTML.contains("id=\"workspace-button\""));
        assert!(INDEX_HTML.contains("aria-haspopup=\"dialog\""));
        assert!(INDEX_HTML.contains("title=\"\""));
        assert!(INDEX_HTML.contains("id=\"workspace-dialog\""));
        assert!(INDEX_HTML.contains("role=\"dialog\""));
        assert!(INDEX_HTML.contains("aria-modal=\"true\""));
        assert!(INDEX_HTML.contains("id=\"workspace-path\""));
        assert!(INDEX_HTML.contains("id=\"workspace-reason\""));
        assert!(INDEX_HTML.contains("id=\"workspace-recent\""));
        assert!(INDEX_HTML.contains("id=\"workspace-browser\""));
        assert!(INDEX_HTML.contains("id=\"workspace-switch\""));
        assert!(INDEX_HTML.contains("id=\"workspace-open\""));
        assert!(WORKSPACE_JS.contains("/api/workspace/open"));
        assert!(INDEX_HTML.contains("/assets/workspace.js"));

        assert!(WORKSPACE_JS.contains("function middleEllipsis"));
        assert!(WORKSPACE_JS.contains("function abbreviateHome"));
        assert!(WORKSPACE_JS.contains("/api/workspace/dirs"));
        assert!(WORKSPACE_JS.contains("/api/workspace/validate"));
        assert!(WORKSPACE_JS.contains("/api/workspace/switch"));
        assert!(!WORKSPACE_JS.contains("innerHTML"));
        assert!(!WORKSPACE_JS.contains("onclick"));

        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let router = build_router(hub, "testtoken123".to_string(), None);
        let req = axum::http::Request::builder()
            .uri("/assets/workspace.js")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let headers = resp.headers().clone();
        assert!(
            headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/javascript")
        );
        assert_eq!(headers["content-security-policy"], "default-src 'self'");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), WORKSPACE_JS.as_bytes());
    }

    // TC-970: sidebar_resize_assets (FR-970, FR-971, FR-973, FR-974, UX-970, UX-971)
    #[tokio::test]
    async fn sidebar_resize_assets() {
        assert!(INDEX_HTML.contains("id=\"sessions-resize\""));
        assert!(INDEX_HTML.contains("id=\"sidebar-resize\""));
        assert!(INDEX_HTML.contains("class=\"sidebar-resize-handle\""));
        assert!(INDEX_HTML.contains("role=\"separator\""));
        assert!(INDEX_HTML.contains("aria-orientation=\"vertical\""));
        assert!(INDEX_HTML.contains("tabindex=\"0\""));
        assert!(INDEX_HTML.contains("aria-controls=\"sessions-sidebar\""));
        assert!(INDEX_HTML.contains("aria-controls=\"sidebar\""));
        assert!(INDEX_HTML.contains("aria-label=\"Resize sessions sidebar\""));
        assert!(INDEX_HTML.contains("aria-label=\"Resize files sidebar\""));
        assert!(INDEX_HTML.contains("aria-valuenow=\"320\""));
        assert!(INDEX_HTML.contains("aria-valuemin=\"180\""));
        // aria-valuemax in the HTML is a static placeholder; the script overwrites it.
        assert!(INDEX_HTML.contains("<script src=\"/assets/sidebar-resize.js\" defer></script>"));
        let workspace_js = INDEX_HTML
            .find("/assets/workspace.js")
            .expect("workspace.js script");
        let resize_js = INDEX_HTML
            .find("/assets/sidebar-resize.js")
            .expect("sidebar-resize.js script");
        assert!(
            workspace_js < resize_js,
            "sidebar-resize.js must be linked after workspace.js"
        );

        let sessions_at = INDEX_HTML
            .find("id=\"sessions-sidebar\"")
            .expect("sessions sidebar");
        let sessions_end = INDEX_HTML
            .find("id=\"sessions-backdrop\"")
            .expect("sessions backdrop");
        let sessions_handle = INDEX_HTML
            .find("id=\"sessions-resize\"")
            .expect("sessions handle");
        assert!(
            sessions_at < sessions_handle && sessions_handle < sessions_end,
            "#sessions-resize must sit inside #sessions-sidebar"
        );
        let files_at = INDEX_HTML.find("id=\"sidebar\"").expect("files sidebar");
        let files_end = INDEX_HTML
            .find("id=\"sidebar-backdrop\"")
            .expect("files backdrop");
        let files_handle = INDEX_HTML
            .find("id=\"sidebar-resize\"")
            .expect("files handle");
        assert!(
            files_at < files_handle && files_handle < files_end,
            "#sidebar-resize must sit inside #sidebar"
        );

        assert!(DOCS_CSS.contains("--left-sidebar-width: 320px;"));
        assert!(DOCS_CSS.contains("--right-sidebar-width: 320px;"));
        assert!(DOCS_CSS.contains("width: var(--left-sidebar-width);"));
        assert!(DOCS_CSS.contains("width: var(--right-sidebar-width);"));
        assert!(DOCS_CSS.contains("--left-dock: var(--left-sidebar-width);"));
        assert!(DOCS_CSS.contains("--right-dock: var(--right-sidebar-width);"));
        assert!(DOCS_CSS.contains("cursor: col-resize;"));
        assert!(DOCS_CSS.contains(":focus-visible"));
        let resizing_at = DOCS_CSS
            .find("body.sidebar-resizing")
            .expect("body.sidebar-resizing rule");
        let resizing_end = resizing_at
            + DOCS_CSS[resizing_at..]
                .find('}')
                .expect("end of sidebar-resizing block");
        let resizing_css = &DOCS_CSS[resizing_at..=resizing_end];
        assert!(
            resizing_css.contains("transition: none;"),
            "sidebar-resizing block disables transition: {resizing_css}"
        );
        assert!(
            resizing_css.contains("user-select: none;"),
            "sidebar-resizing block suppresses selection: {resizing_css}"
        );
        let instant_at = DOCS_CSS
            .find("body.sidebar-resize-instant")
            .expect("sidebar-resize-instant rule");
        let instant_end = instant_at
            + DOCS_CSS[instant_at..]
                .find('}')
                .expect("end of sidebar-resize-instant block");
        let instant_css = &DOCS_CSS[instant_at..=instant_end];
        assert!(
            instant_css.contains("transition: none;"),
            "sidebar-resize-instant block disables transition: {instant_css}"
        );
        let narrow = DOCS_CSS
            .find("@media (max-width: 900px)")
            .expect("narrow media");
        let narrow_end = DOCS_CSS[narrow..]
            .find("\nbody {")
            .expect("body rule follows the narrow media query")
            + narrow;
        let narrow_css = &DOCS_CSS[narrow..narrow_end];
        assert!(
            narrow_css.contains("width: min(320px, 85vw);"),
            "narrow drawers keep min(320px, 85vw): {narrow_css}"
        );
        let handle_at = narrow_css
            .find(".sidebar-resize-handle")
            .expect("narrow layout must mention the handle");
        let handle_end = handle_at
            + narrow_css[handle_at..]
                .find('}')
                .expect("end of narrow handle rule");
        let handle_css = &narrow_css[handle_at..=handle_end];
        assert!(
            handle_css.contains("display: none;"),
            "@media (max-width: 900px) must hide .sidebar-resize-handle: {handle_css}"
        );

        assert!(SIDEBAR_RESIZE_JS.contains("dmuxai.sidebar.width.left"));
        assert!(SIDEBAR_RESIZE_JS.contains("dmuxai.sidebar.width.right"));
        assert!(SIDEBAR_RESIZE_JS.contains("var MIN_WIDTH = 180"));
        assert!(SIDEBAR_RESIZE_JS.contains("Math.floor(vw * 0.5)"));
        assert!(SIDEBAR_RESIZE_JS.contains("var SHIFT_STEP = 64"));
        assert!(SIDEBAR_RESIZE_JS.contains("key === 'Home'"));
        assert!(SIDEBAR_RESIZE_JS.contains("key === 'End'"));
        assert!(SIDEBAR_RESIZE_JS.contains("key === 'Enter'"));
        assert!(SIDEBAR_RESIZE_JS.contains("dblclick"));
        assert!(SIDEBAR_RESIZE_JS.contains("addEventListener('pointercancel'"));
        assert!(SIDEBAR_RESIZE_JS.contains("addEventListener('lostpointercapture'"));
        assert!(SIDEBAR_RESIZE_JS.contains("document.documentElement.clientWidth"));
        assert!(SIDEBAR_RESIZE_JS.contains("removeItem"));
        assert!(SIDEBAR_RESIZE_JS.contains("ArrowRight"));
        assert!(SIDEBAR_RESIZE_JS.contains("ArrowLeft"));
        assert!(SIDEBAR_RESIZE_JS.contains("setPointerCapture"));
        assert!(SIDEBAR_RESIZE_JS.contains("sidebar-resizing"));
        assert!(SIDEBAR_RESIZE_JS.contains("MutationObserver"));
        assert!(SIDEBAR_RESIZE_JS.contains("shiftKey"));
        assert!(!SIDEBAR_RESIZE_JS.contains("innerHTML"));
        assert!(!SIDEBAR_RESIZE_JS.contains("onclick"));

        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let router = build_router(hub, "testtoken123".to_string(), None);
        let req = axum::http::Request::builder()
            .uri("/assets/sidebar-resize.js")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let headers = resp.headers().clone();
        assert!(
            headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/javascript")
        );
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(headers["content-security-policy"], "default-src 'self'");
        assert_eq!(headers["referrer-policy"], "no-referrer");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), SIDEBAR_RESIZE_JS.as_bytes());
    }

    fn js_between<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
        let from = src
            .find(start)
            .unwrap_or_else(|| panic!("missing JS marker {start:?}"));
        let tail = &src[from..];
        let to = tail
            .find(end)
            .unwrap_or_else(|| panic!("missing JS marker {end:?} after {start:?}"));
        &tail[..to]
    }

    // TC-952 (FR-945, UX-933): route selector beside send; POST /api/prompt `route`.
    #[test]
    fn prompt_route_selector_present() {
        assert!(SESSION_JS.contains("var PROMPT_ROUTES = ["));
        for label in ["Auto", "Plan", "Direct"] {
            assert!(
                SESSION_JS.contains(&format!("label: '{label}'")),
                "missing route label {label}"
            );
        }
        for value in ["auto", "plan", "direct"] {
            assert!(
                SESSION_JS.contains(&format!("value: '{value}'")),
                "missing route value {value}"
            );
        }
        assert!(SESSION_JS.contains("picker('Route', 'route'"));
        assert!(SESSION_JS.contains("data-parity"));
        assert!(SESSION_JS.contains("body.route = ui.route.value"));
        assert!(SESSION_JS.contains("ui.route.value !== 'auto'"));
        assert!(SESSION_JS.contains("/api/prompt"));
        assert!(SESSION_CSS.contains(".session-send-col"));
    }

    // TC-957 (UX-930): `#graph-judge-line` from `graph.triage.line`, text nodes only.
    #[test]
    fn judge_line_asset() {
        assert!(INDEX_HTML.contains("id=\"graph-judge-line\""));
        assert!(APP_JS.contains("function renderGraphJudgeLine"));
        assert!(APP_JS.contains("function fillJudgeLineContent"));
        assert!(APP_JS.contains("graph.triage"));
        assert!(APP_JS.contains("triage.line"));
        assert!(APP_JS.contains("judge-fallback-word"));
        assert!(APP_JS.contains("replaceChildren"));
        assert!(APP_JS.contains("createTextNode"));
        assert!(APP_CSS.contains(".graph-judge-line"));
        assert!(APP_CSS.contains(".judge-fallback-word"));

        let judge_js = js_between(
            APP_JS,
            "function fillJudgeLineContent",
            "function renderGraphJudgeLine",
        );
        assert!(
            !judge_js.contains("innerHTML"),
            "judge line must not use innerHTML"
        );
        let render_js = js_between(
            APP_JS,
            "function renderGraphJudgeLine",
            "// UX-932 / FR-965",
        );
        assert!(
            !render_js.contains("innerHTML"),
            "renderGraphJudgeLine must not use innerHTML"
        );
    }

    // TC-960 (UX-932, UX-933, INT-930): approval panel, plan commands, parity ids.
    #[test]
    fn approval_panel_asset() {
        assert!(INDEX_HTML.contains("id=\"graph-approval\""));
        assert!(SESSION_JS.contains("parityButton"));
        assert!(SESSION_JS.contains("classList.add"));
        assert!(APP_JS.contains("planGateParityButton('Approve', 'plan-approve', 'btn-primary'"));
        assert!(APP_JS.contains("planGateParityButton('Edit', 'plan-edit', ''"));
        assert!(APP_JS.contains("planGateParityButton('Reject', 'plan-reject', 'btn-danger'"));
        assert!(APP_JS.contains("viewOnly: isDashboardViewOnly()"));
        assert!(APP_JS.contains("syncApprovalPanelButtonStates"));
        assert!(APP_JS.contains("panel.querySelectorAll('button[data-plan-gate]')"));
        assert!(APP_JS.contains("position: idx"));
        assert!(APP_JS.contains("if (idx > 0) {"));
        assert!(INDEX_HTML.contains("data-parity=\"plan-history\""));
        assert!(APP_JS.contains("historyBtn.disabled = false"));
        assert!(INDEX_HTML.contains("data-parity=\"plan-replan\""));
        assert!(APP_JS.contains("window.dmuxaiPlanGate"));
        assert!(APP_JS.contains("function renderApprovalPanel"));
        assert!(APP_JS.contains("approvalPanelRenderKey"));
        assert!(APP_JS.contains("approvalPanelLastKey"));
        assert!(APP_JS.contains("approvalPanelFocusFeedback"));
        let approval_render_js = js_between(
            APP_JS,
            "function renderApprovalPanel",
            "// UX-936: task checklist below the swimlane",
        );
        assert!(
            !approval_render_js.contains("|| editOpen"),
            "do not refocus feedback textarea on every render when editOpen"
        );
        assert!(APP_JS.contains("command: 'plan_decision'"));
        assert!(APP_JS.contains("command: 'plan_task_edit'"));
        assert!(APP_JS.contains("graph.phase !== 'awaiting_approval'"));
        assert!(APP_JS.contains("graph-approval-header"));
        assert!(APP_JS.contains("graph-approval-error"));
        assert!(APP_JS.contains("action: 'rename'"));
        assert!(APP_JS.contains("action: 'split'"));
        assert!(APP_JS.contains("action: 'merge'"));
        assert!(APP_JS.contains("action: 'delete'"));
        assert!(APP_JS.contains("action: 'move'"));
        assert!(APP_JS.contains("position: idx + 2"));
        assert!(APP_JS.contains("parse_graph_task_edit"));
        assert!(APP_CSS.contains(".graph-approval"));

        let plan_gate_js = js_between(APP_JS, "// UX-932 / FR-965", "var lastRenderedAgentsKey");
        assert!(
            !plan_gate_js.contains("innerHTML"),
            "plan approval and task-edit paths must not use innerHTML"
        );
        assert!(
            plan_gate_js.contains("textContent") || plan_gate_js.contains("createTextNode"),
            "plan gate UI must render via text nodes"
        );
    }

    // TC-996 (UX-936): task checklist panel below the swimlane.
    #[test]
    fn task_checklist_asset() {
        assert!(INDEX_HTML.contains("id=\"task-checklist\""));
        assert!(INDEX_HTML.contains("data-parity=\"task-checklist\""));
        assert!(INDEX_HTML.contains("graph-stage-layout"));
        let canvas_pos = INDEX_HTML
            .find("id=\"graph-canvas\"")
            .expect("graph-canvas in index");
        let checklist_pos = INDEX_HTML
            .find("id=\"task-checklist\"")
            .expect("task-checklist in index");
        assert!(
            canvas_pos < checklist_pos,
            "task checklist must follow graph canvas in index.html"
        );
        assert!(APP_JS.contains("function renderTaskChecklist"));
        assert!(APP_JS.contains("function taskChecklistStatusIcon"));
        assert!(APP_JS.contains("graph.tasks"));
        assert!(APP_JS.contains("node-task-highlight"));
        assert!(APP_JS.contains("task.done_check"));
        assert!(APP_JS.contains("function taskChecklistStatusClass"));
        assert!(APP_JS.contains("task-checklist-row-expanded"));
        assert!(APP_JS.contains("tasksForChecklist"));
        assert!(APP_JS.contains("data-task-id"));
        assert!(APP_JS.contains("function tasksForChecklist"));
        assert!(APP_JS.contains("list.insertBefore(rowNode"));
        assert!(!APP_JS.contains("taskChecklistEverVisible"));
        assert!(!APP_CSS.contains("max-width: 280px"));
        assert!(APP_CSS.contains("flex-direction: column"));
        assert!(
            !APP_CSS.contains(".graph-stage-layout {\n    flex-direction: row"),
            "graph-stage-layout must not switch to row at wide widths"
        );
        assert!(
            APP_CSS.contains(
                "grid-template-columns: repeat(auto-fill, minmax(min(240px, 100%), 1fr))"
            )
        );
        assert!(!APP_JS.contains("autoFitGraphAfterTaskChecklistLayout"));
        for status in ["pending", "running", "done", "failed"] {
            assert!(
                APP_CSS.contains(&format!(".task-checklist-status-{status}")),
                "missing status style for {status}"
            );
        }
        for icon in ["\\u25CB", "\\u25D0", "\\u2713", "\\u2717"] {
            assert!(
                APP_JS.contains(icon),
                "taskChecklistStatusIcon must define icon {icon}"
            );
        }
        assert!(APP_CSS.contains(".task-checklist"));
        assert!(APP_CSS.contains(".graph-stage-layout"));
        assert!(APP_CSS.contains(".node-task-highlight"));

        let checklist_js = js_between(
            APP_JS,
            "// UX-936: task checklist below the swimlane",
            "// UX-934: replan + revision history in the graph header.",
        );
        assert!(
            !checklist_js.contains("innerHTML"),
            "task checklist must not use innerHTML"
        );
        assert!(
            checklist_js.contains("task-checklist-label") && checklist_js.contains("done_check"),
            "task title and done_check must render via DOM helpers"
        );
        // TC-1310 / UX-1310: a selected task shows the run prompt and node progress.
        assert!(checklist_js.contains("task-checklist-prompt"));
        assert!(checklist_js.contains("user_prompt"));
        assert!(checklist_js.contains("task-checklist-progress"));
        assert!(checklist_js.contains("agent.activity"));
        assert!(checklist_js.contains("agentActivityForRunner(node.runner)"));
        assert!(
            !checklist_js.contains("if (status === 'running') {\n    var activity"),
            "agent activity must not be gated on node running (UX-1310)"
        );
        assert!(checklist_js.contains("\\u00b7 unknown"));
        assert!(
            checklist_js.contains("checklistStatusFromNode(node)")
                && checklist_js.contains("status: checklistStatusFromNode(node)"),
            "single-node fallback task status must derive from node status"
        );
        assert!(
            checklist_js.contains("\\u2014"),
            "missing runner/model/elapsed must use em dash (UX-1310)"
        );
        assert!(
            APP_CSS
                .contains(".task-checklist-row.task-checklist-row-expanded .task-checklist-prompt")
        );
        assert!(APP_JS.contains("if (taskChecklistSelectedId) { renderTaskChecklist(graph); }"));
    }

    // TC-979 (UX-934, UX-935): revision badges on nodes and plan-history list.
    #[test]
    fn revision_badges_and_history_asset() {
        assert!(APP_JS.contains("node.rev_change"));
        assert!(APP_JS.contains("'card-rev-badge card-rev-badge-' + revChange"));
        assert!(APP_JS.contains("revChange === 'added'"));
        assert!(APP_JS.contains("revChange === 'changed'"));
        assert!(APP_JS.contains("function graphNodesRevKey"));
        assert!(INDEX_HTML.contains("id=\"graph-replan\""));
        assert!(INDEX_HTML.contains("data-parity=\"plan-replan\""));
        assert!(INDEX_HTML.contains("id=\"plan-history\""));
        assert!(INDEX_HTML.contains("data-parity=\"plan-history\""));
        assert!(INDEX_HTML.contains("id=\"graph-plan-history\""));
        assert!(APP_JS.contains("function renderPlanHistoryPanel"));
        assert!(APP_JS.contains("graph.plan_history"));
        assert!(APP_JS.contains("command: 'plan_replan'"));
        assert!(APP_JS.contains("panel.scrollTop = scrollTop"));
        for theme in ["dark", "light"] {
            assert!(
                APP_CSS.contains(&format!(":root[data-theme=\"{theme}\"]")),
                "missing {theme} palette for rev badges"
            );
        }
        assert!(APP_CSS.contains(".card-rev-badge-added"));
        assert!(APP_CSS.contains(".card-rev-badge-changed"));
        assert!(APP_CSS.contains(".graph-plan-history-list"));

        let history_js = js_between(
            APP_JS,
            "function renderPlanHistoryPanel",
            "function renderGraphHeaderActions",
        );
        assert!(
            !history_js.contains("innerHTML"),
            "plan history panel must not use innerHTML"
        );
        assert!(history_js.contains("createTextNode"));
    }

    /// The declarations of the first top-level `selector {` rule in `css`.
    fn css_rule<'a>(css: &'a str, selector: &str) -> &'a str {
        let open = format!("\n{selector} {{\n");
        let start = css
            .find(&open)
            .unwrap_or_else(|| panic!("missing CSS rule {selector}"))
            + open.len();
        let end = css[start..].find("\n}").expect("rule closes") + start;
        &css[start..end]
    }

    // TC-1200 (SPEC-dashboard-fullwidth FR-1200, FR-1201): the center column
    // has no width cap and no centered fixed container; the graph re-lays out
    // when the column width changes without a window resize.
    #[test]
    fn center_column_uses_full_width() {
        let main_rule = css_rule(APP_CSS, "main");
        assert!(main_rule.contains("max-width: none;"), "{main_rule}");
        assert!(main_rule.contains("margin: 0;"), "{main_rule}");
        assert!(!main_rule.contains("margin: 0 auto"), "{main_rule}");
        for css in [APP_CSS, DOCS_CSS, SESSION_CSS] {
            assert!(!css.contains("max-width: 880px"));
        }
        // Narrow windows keep the existing small-screen padding.
        assert!(APP_CSS.contains("main { padding: 0 10px 28px; }"));
        // Docked sidebars still reserve their width (SPEC-centered-prompt).
        assert!(DOCS_CSS.contains("padding-left: var(--left-dock);"));
        assert!(DOCS_CSS.contains("padding-right: var(--right-dock);"));
        // The chat transcript grows with the viewport instead of a fixed 26rem.
        assert!(SESSION_CSS.contains("height: clamp(26rem, 55vh, 48rem);"));
        assert!(APP_JS.contains("new window.ResizeObserver("));
    }

    // TC-1201 (SPEC-dashboard-fullwidth FR-1202, FR-1203, UX-1200): the center
    // Documents panel is collapsed in the served HTML, with an accessible
    // toggle button.
    #[test]
    fn documents_panel_is_collapsed_by_default() {
        let start = INDEX_HTML.find("<section id=\"docs\"").expect("docs panel");
        let end = INDEX_HTML[start..].find("</section>").expect("closes") + start;
        let panel = &INDEX_HTML[start..end];
        assert!(panel.contains("data-collapsed=\"true\""), "{panel}");
        assert!(panel.contains("<button id=\"docs-toggle\" type=\"button\""));
        assert!(panel.contains("aria-expanded=\"false\""));
        assert!(panel.contains("aria-controls=\"docs-body\""));
        assert!(panel.contains("<div id=\"docs-body\" class=\"panel-body\" hidden>"));
        // The change list lives inside the collapsed body.
        let body = panel.find("id=\"docs-body\"").unwrap();
        assert!(panel.find("id=\"docs-changes\"").unwrap() > body);
        assert!(APP_CSS.contains(".panel-body[hidden]"));
        assert!(APP_CSS.contains(".panel-toggle:focus-visible"));
        assert!(APP_CSS.contains(".panel-toggle[aria-expanded=\"true\"] .panel-caret"));
    }

    // TC-1202 (SPEC-dashboard-fullwidth FR-1203, FR-1204): the toggle flips
    // aria-expanded and hidden together, defaults to collapsed, keeps the state
    // in sessionStorage only, and the header shows the change count.
    #[test]
    fn documents_toggle_script() {
        let toggle_js = js_between(DOCS_JS, "var DOCS_EXPANDED_KEY", "function docBadge");
        assert!(toggle_js.contains("'dmuxai.docsExpanded'"));
        assert!(toggle_js.contains("window.sessionStorage.getItem(DOCS_EXPANDED_KEY) === '1'"));
        assert!(toggle_js.contains("return false;"));
        assert!(!toggle_js.contains("localStorage"));
        assert!(toggle_js.contains("toggle.setAttribute('aria-expanded'"));
        assert!(toggle_js.contains("body.hidden = !expanded;"));
        assert!(!toggle_js.contains("innerHTML"));
        assert!(DOCS_JS.contains("initDocsToggle();"));
        assert!(DOCS_JS.contains("setDocsCount(shown);"));
    }

    // Chat "copy reply": agent messages get one copy button that copies the
    // raw Markdown (`m.content`), with an execCommand fallback and hover CSS.
    #[test]
    fn copy_reply_button_assets() {
        assert!(SESSION_JS.contains("if (m.role === 'agent') {\n          var actionsBar"));
        assert!(SESSION_JS.contains("node.dmuxRaw = String(m.content || '')"));
        assert!(SESSION_JS.contains("copyText(n.dmuxRaw || '')"));
        assert!(SESSION_JS.contains("navigator.clipboard.writeText(text)"));
        assert!(SESSION_JS.contains("window.isSecureContext"));
        assert!(SESSION_JS.contains("document.execCommand('copy')"));
        assert!(SESSION_JS.contains("'aria-label', 'Copy reply'"));
        assert!(SESSION_JS.contains("'aria-label', 'Copied'"));
        assert!(SESSION_CSS.contains(".msg-agent:hover .msg-actions"));
        assert!(SESSION_CSS.contains("@media (hover: none) { .msg-actions { opacity: 1; } }"));
    }

    // TC-1206 (SPEC-dashboard-fullwidth FR-1206..FR-1210): ChatGPT-style chat
    // and the single place an agent turn's text is rendered.
    #[test]
    fn chatgpt_style_chat_assets() {
        let text_js = js_between(
            SESSION_JS,
            "function renderMessagePlain",
            "function picker(",
        );
        assert!(SESSION_JS.contains("function renderMessageText"));
        assert!(SESSION_JS.contains("content_html"));
        assert!(SESSION_JS.contains("innerHTML"));
        assert!(text_js.contains("el('pre', 'msg-code')"));
        assert!(text_js.contains("'msg-inline-code'"));
        assert!(text_js.contains("document.createTextNode"));
        assert!(SESSION_JS.contains("e.key === 'Enter' && !e.shiftKey"));
        assert!(SESSION_JS.contains("node.setAttribute('data-msg-id', m.id);"));
        assert!(SESSION_JS.contains("'dmuxai:transcript-rendered'"));
        assert!(SESSION_CSS.contains("--chat-col: 860px;"));
        assert!(SESSION_CSS.contains(".session-transcript .msg-user { align-items: flex-end; }"));
        assert!(SESSION_CSS.contains("border-radius: 1.5rem;"));
        assert!(SESSION_CSS.contains("height: calc(100dvh - var(--top-h, 56px) - 32px);"));
        assert!(SESSION_CSS.contains(".msg-doc-card {"));

        let docs_js = js_between(DOCS_JS, "function fillTurnOutput", "function syncChatLinks");
        assert!(!docs_js.contains("innerHTML"));
        // The tail <pre> is only built after the in-chat branch returned.
        let in_chat = docs_js.find("if (inChat) {").expect("in-chat branch");
        let tail = docs_js.find("el('pre', 'doc-tail'").expect("tail pre");
        assert!(in_chat < tail);
        let card_js = js_between(DOCS_JS, "function docCard", "function syncChatLinks");
        assert!(!card_js.contains("output_tail"));
        assert!(!card_js.contains("turnTailText"));
        assert!(card_js.contains("'Open in Diff'"));
        assert!(DOCS_JS.contains("'Show in chat'"));
    }

    // TC-1209 (SPEC-dashboard-fullwidth FR-1211, FR-1212, FR-1213, FR-1214, FR-1215, FR-1216): Diff and Graph are
    // header-button center views, hidden in the served HTML.
    #[test]
    fn diff_and_graph_center_view_assets() {
        for (id, panel, view) in [
            ("diff-toggle", "docs", "diff"),
            ("graph-toggle", "graph", "graph"),
        ] {
            let start = INDEX_HTML
                .find(&format!("<button id=\"{id}\""))
                .unwrap_or_else(|| panic!("missing {id}"));
            let tag = &INDEX_HTML[start..INDEX_HTML[start..].find('>').unwrap() + start];
            assert!(
                tag.contains("class=\"btn sidebar-toggle view-toggle\""),
                "{tag}"
            );
            assert!(tag.contains(&format!("aria-controls=\"{panel}\"")), "{tag}");
            assert!(tag.contains("aria-expanded=\"false\""), "{tag}");
            assert!(tag.contains(&format!("data-view=\"{view}\"")), "{tag}");
            let section = format!("<section id=\"{panel}\" class=\"panel");
            let at = INDEX_HTML.find(&section).expect("view section");
            let open_tag = &INDEX_HTML[at..INDEX_HTML[at..].find('>').unwrap() + at];
            assert!(open_tag.contains("center-view"), "{open_tag}");
            assert!(open_tag.ends_with(" hidden"), "{open_tag}");
        }
        // The buttons sit right after To-dos in the header.
        let todos = INDEX_HTML.find("id=\"todos-toggle\"").unwrap();
        let diff = INDEX_HTML.find("id=\"diff-toggle\"").unwrap();
        let graph = INDEX_HTML.find("id=\"graph-toggle\"").unwrap();
        let status = INDEX_HTML.find("<nav id=\"status\"").unwrap();
        assert!(todos < diff && diff < graph && graph < status);
        assert!(INDEX_HTML.contains("id=\"graph-indicator\""));
        // FR-1215: the chat reminder is the inline approval card (FR-1221),
        // not a separate callout.
        assert!(!INDEX_HTML.contains("approval-callout"));

        let views_js = js_between(APP_JS, "var CENTER_VIEW_KEY", "function bindGraphControls");
        assert!(views_js.contains("'dmuxai.centerView'"));
        assert!(views_js.contains("window.sessionStorage"));
        assert!(!views_js.contains("localStorage"));
        assert!(views_js.contains("toggle.setAttribute('aria-expanded'"));
        assert!(views_js.contains("e.key !== 'Escape'"));
        assert!(views_js.contains("graph.phase === 'awaiting_approval'"));
        assert!(views_js.contains("approvalAutoOpened[key]"));
        assert!(!views_js.contains("innerHTML"));
        assert!(APP_JS.contains("bindCenterViews();"));
        assert!(APP_JS.contains("syncCenterViewsWithSnapshot(snap);"));
        assert!(DOCS_CSS.contains(".center-view[hidden]"));
        assert!(DOCS_CSS.contains("left: calc(var(--left-dock, 0px) + 14px);"));
        assert!(DOCS_CSS.contains("right: calc(var(--right-dock, 0px) + 14px);"));
        assert!(DOCS_CSS.contains(".view-indicator-approval"));
        assert!(DOCS_CSS.contains(".todos-count[hidden]"));
        assert!(DOCS_JS.contains("window.dmuxaiSetCenterView('diff')"));
        assert!(DOCS_JS.contains("$('diff-count')"));
    }

    // Top bar Chat button: a view tab that returns to the chat from the
    // Diff/Graph views and the Memory popover.
    #[test]
    fn chat_button_assets() {
        let start = INDEX_HTML
            .find("<button id=\"chat-toggle\"")
            .expect("chat button");
        let tag = &INDEX_HTML[start..INDEX_HTML[start..].find('>').unwrap() + start];
        assert!(tag.contains("aria-pressed=\"true\""), "{tag}");
        assert!(tag.contains("aria-controls=\"session\""), "{tag}");
        // Placed after To-dos and before Diff/Graph/Memory.
        let todos = INDEX_HTML.find("id=\"todos-toggle\"").unwrap();
        let diff = INDEX_HTML.find("id=\"diff-toggle\"").unwrap();
        assert!(todos < start && start < diff);
        for id in [
            "diff-toggle",
            "graph-toggle",
            "memory-toggle",
            "skills-toggle",
        ] {
            let at = INDEX_HTML.find(&format!("<button id=\"{id}\"")).unwrap();
            let tag = &INDEX_HTML[at..INDEX_HTML[at..].find('>').unwrap() + at];
            assert!(tag.contains("aria-pressed=\"false\""), "{tag}");
        }
        let views_js = js_between(APP_JS, "var CENTER_VIEW_KEY", "function bindGraphControls");
        assert!(views_js.contains("toggle.setAttribute('aria-pressed'"));
        assert!(views_js.contains("function syncChatTab"));
        assert!(views_js.contains("function showChat"));
        assert!(views_js.contains("window.dmuxaiCloseMemory"));
        assert!(views_js.contains("'dmuxai:memory-popover'"));
        assert!(views_js.contains("preventScroll: true"));
        assert!(MEMORY_JS.contains("window.dmuxaiCloseMemory"));
        assert!(MEMORY_JS.contains("'dmuxai:memory-popover'"));
        assert!(MEMORY_JS.contains("toggle.setAttribute('aria-pressed'"));
        assert!(views_js.contains("window.dmuxaiCloseSkills"));
        assert!(views_js.contains("'dmuxai:skills-popover'"));
        assert!(SKILLS_JS.contains("window.dmuxaiCloseSkills"));
        assert!(SKILLS_JS.contains("'dmuxai:skills-popover'"));
        assert!(SKILLS_JS.contains("toggle.setAttribute('aria-pressed'"));
        assert!(DOCS_CSS.contains(".sidebar-toggle[aria-pressed=\"true\"]"));
    }

    // TC-1212 (SPEC-dashboard-fullwidth FR-1217, FR-1218, FR-1219, FR-1220,
    // FR-1221, UX-1202): inline agent-processing rows in the chat.
    #[test]
    fn chat_inline_activity_assets() {
        let aux = js_between(
            SESSION_JS,
            "function fmtElapsed",
            "// ---- ChatGPT-style layout",
        );
        // Rows are built with DOM APIs and stay collapsed until clicked.
        assert!(!aux.contains("innerHTML"));
        assert!(aux.contains("head.setAttribute('aria-expanded', open ? 'true' : 'false')"));
        assert!(aux.contains("detail.hidden = !open;"));
        // FR-1217: "Running X · done/total steps · elapsed", live-updated.
        assert!(aux.contains("'Running ' + running.join(', ')"));
        assert!(aux.contains("' steps'"));
        assert!(SESSION_JS.contains("window.setInterval(tickLiveRows, 1000)"));
        // FR-1218: a step opens Graph focused on that node.
        assert!(aux.contains("window.dmuxaiFocusGraphNode(String(n.id))"));
        assert!(APP_JS.contains("window.dmuxaiFocusGraphNode = function (id)"));
        // FR-1219: consecutive activity lines fold into one actions row.
        assert!(aux.contains("' action' : ' actions'"));
        assert!(aux.contains("msg-activity-grouped"));
        // FR-1221: inline approval card with Approve / Edit / Reject.
        for label in [
            "'Approve', 'approve'",
            "'Edit', 'edit'",
            "'Reject', 'reject'",
        ] {
            assert!(aux.contains(label), "{label}");
        }
        assert!(aux.contains("window.dmuxaiChatPlanGate"));
        assert!(APP_JS.contains("window.dmuxaiChatPlanGate = {"));
        // The plan-gate API used elsewhere is still defined once.
        assert_eq!(APP_JS.matches("window.dmuxaiPlanGate = {").count(), 1);
        // FR-1220: doc card file rows open Diff at that file.
        assert!(DOCS_JS.contains("msg-doc-card-file-btn"));
        assert!(DOCS_JS.contains("function revealFile(turnId, path)"));
        // UX-1202: muted one-line rows in both themes, reduced motion honoured.
        assert!(SESSION_CSS.contains(".session-transcript > li.chat-aux {"));
        assert!(SESSION_CSS.contains(".chat-aux-approval"));
        assert!(SESSION_CSS.contains("li.chat-aux { margin-bottom: .5rem; color: var(--muted)"));
        assert!(SESSION_CSS.contains("@media (prefers-reduced-motion: reduce) { .chat-aux-caret"));
    }

    // TC-1219 (SPEC-dashboard-fullwidth FR-1223, FR-1224): one run row per
    // exchange from the turn bands; older runs open through `show_graph_run`.
    #[test]
    fn per_turn_run_row_assets() {
        let aux = js_between(
            SESSION_JS,
            "function fmtElapsed",
            "// ---- ChatGPT-style layout",
        );
        assert!(aux.contains("Array.isArray(graph.turns) ? graph.turns : []"));
        assert!(aux.contains("li.getAttribute('data-msg-id') === String(band.message_id)"));
        assert!(aux.contains("function bandRunRow(band)"));
        assert!(aux.contains("'Open in Graph'"));
        assert!(aux.contains("window.dmuxaiShowGraphRun(runId, nodeId)"));
        // Cancelled, rejected and stored mid-run phases never show a spinner.
        assert!(
            aux.contains("var LIVE_PHASES = ['active', 'waiting_gate', 'triage', 'planning'];")
        );
        assert!(!aux.contains("innerHTML"));
        assert!(APP_JS.contains("command: 'show_graph_run', run_id: runId"));
        assert!(APP_JS.contains("applyPendingRunFocus(snap);"));
        assert!(SESSION_CSS.contains(".chat-aux-open-run {"));
    }

    // TC-1225 / SPEC-dashboard-fullwidth FR-1225..FR-1227
    #[test]
    fn sessions_grouped_by_workspace_assets() {
        let list = js_between(
            SESSION_JS,
            "var SESSIONS_GROUP_VISIBLE = 3;",
            "// ---- palette",
        );
        assert!(list.contains("SESSIONS_COLLAPSED_KEY = 'dmuxai.sessions.collapsed'"));
        assert!(list.contains("window.sessionStorage.setItem(SESSIONS_COLLAPSED_KEY"));
        assert!(list.contains("function compactTime(ms)"));
        assert!(list.contains("expanded ? 'Less' : 'More'"));
        assert!(list.contains("head.title = path;"));
        assert!(
            list.contains("command: 'prepare_open_conversation', workspace: group.path, id: c.id")
        );
        assert!(list.contains("'This workspace folder no longer exists'"));
        assert!(list.contains("if (!list.length && (q || !group.current)) { return; }"));
        assert!(!list.contains("innerHTML"));
        assert!(WORKSPACE_JS.contains("window.dmuxaiSwitchWorkspace = function (path)"));
        assert!(WORKSPACE_JS.contains("apiPost('/api/workspace/switch', { path: path })"));
        assert!(DOCS_CSS.contains(".sessions-group-name {"));
        assert!(DOCS_CSS.contains(".sessions-group-label {"));
    }

    // TC-1473 (FR-1473): "Open in new window" needs a validated path.
    #[test]
    fn tc_1473_open_button_requires_validated_path() {
        let start = WORKSPACE_JS.find("function updateOpenButton").unwrap();
        let body = &WORKSPACE_JS[start..start + 400];
        assert!(body.contains("!state.validPath"), "{body}");
        assert!(!body.contains("|| !target"), "{body}");
    }
}
