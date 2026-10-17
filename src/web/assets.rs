pub const INDEX_HTML: &str = include_str!("assets/index.html");
pub const APP_JS: &str = include_str!("assets/app.js");
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
}
