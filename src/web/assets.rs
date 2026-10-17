pub const INDEX_HTML: &str = include_str!("assets/index.html");
pub const APP_JS: &str = include_str!("assets/app.js");
pub const MEMORY_JS: &str = include_str!("assets/memory.js");
pub const APP_CSS: &str = include_str!("assets/app.css");

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
        // localStorage holds only the two theme keys; every set/remove targets them.
        assert!(js.contains("'dmuxai.theme'"));
        assert!(js.contains("'dmuxai.accent'"));
        let mut local_storage_writes = 0;
        for line in js.lines() {
            let line = line.trim();
            if line.contains("window.localStorage.setItem(")
                || line.contains("window.localStorage.removeItem(")
            {
                local_storage_writes += 1;
                assert!(
                    line.contains("THEME_KEY") || line.contains("ACCENT_KEY"),
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
}
