use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::HeaderName;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio_stream::Stream;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::WatchStream;

use crate::web::DEFAULT_PORT;
use crate::web::auth::{BindCheck, check_bind, dashboard_url, token_matches};
use crate::web::hub::DashboardHub;

#[derive(Clone)]
pub struct ServerConfig {
    pub bind: IpAddr,
    pub port: u16,
    pub allow_remote: bool,
    pub token: String,
}

#[derive(Clone)]
struct AppState {
    hub: DashboardHub,
    token: Arc<String>,
    allowed_hosts: Option<Arc<Vec<String>>>,
}

pub struct DashboardServer {
    pub addr: SocketAddr,
    pub url: String,
    pub warning: Option<String>,
    handle: tokio::task::JoinHandle<()>,
}

impl DashboardServer {
    pub fn abort(&self) {
        self.handle.abort();
    }
}

async fn get_index() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'self'",
            ),
            (HeaderName::from_static("referrer-policy"), "no-referrer"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        crate::web::assets::INDEX_HTML,
    )
}

async fn get_app_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'self'",
            ),
            (HeaderName::from_static("referrer-policy"), "no-referrer"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        crate::web::assets::APP_JS,
    )
}

async fn get_app_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'self'",
            ),
            (HeaderName::from_static("referrer-policy"), "no-referrer"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        crate::web::assets::APP_CSS,
    )
}

async fn get_state(State(state): State<AppState>) -> Response {
    let snapshot = state.hub.snapshot();
    match serde_json::to_string(&snapshot) {
        Ok(json) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
                (HeaderName::from_static("x-content-type-options"), "nosniff"),
            ],
            json,
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
                (HeaderName::from_static("x-content-type-options"), "nosniff"),
            ],
            r#"{"error":"internal"}"#,
        )
            .into_response(),
    }
}

async fn get_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = WatchStream::new(state.hub.subscribe()).filter_map(|snapshot| {
        serde_json::to_string(&snapshot)
            .ok()
            .map(|json| Ok(Event::default().event("snapshot").data(json)))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

async fn security_and_auth_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    // 1. Host header validation
    let host_header = match req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        Some(h) => h,
        None => return StatusCode::FORBIDDEN.into_response(),
    };

    if let Some(ref allowed) = state.allowed_hosts
        && !allowed.iter().any(|h| h == host_header)
    {
        return StatusCode::FORBIDDEN.into_response();
    }

    // 2. Origin validation (when present)
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let origin_str = match origin.to_str() {
            Ok(s) => s,
            Err(_) => return StatusCode::FORBIDDEN.into_response(),
        };
        let expected_origin = format!("http://{host_header}");
        if origin_str != expected_origin {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    // 3. /api/* auth validation
    let path = req.uri().path();
    let is_api_path = path.starts_with("/api/")
        || path == "/api"
        || path.starts_with("//api/")
        || path.contains("/api/");

    if is_api_path {
        let mut provided_token: Option<&str> = None;

        if let Some(auth_val) = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        {
            let mut parts = auth_val.splitn(2, ' ');
            let scheme = parts.next().unwrap_or_default();
            let tok = parts.next().unwrap_or_default();
            if scheme.eq_ignore_ascii_case("bearer") && !tok.trim().is_empty() {
                provided_token = Some(tok.trim());
            }
        }

        if provided_token.is_none()
            && let Some(query) = req.uri().query()
        {
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=')
                    && k == "token"
                {
                    if !v.is_empty() {
                        provided_token = Some(v);
                    }
                    break;
                }
            }
        }

        let is_valid = match provided_token {
            Some(tok) => token_matches(&state.token, tok),
            None => false,
        };

        if !is_valid {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"error":"unauthorized"}"#,
            )
                .into_response();
        }
    }

    next.run(req).await
}

pub fn build_router(
    hub: DashboardHub,
    token: String,
    allowed_hosts: Option<Vec<String>>,
) -> Router {
    let state = AppState {
        hub,
        token: Arc::new(token),
        allowed_hosts: allowed_hosts.map(Arc::new),
    };

    Router::new()
        .route("/", get(get_index))
        .route("/assets/app.js", get(get_app_js))
        .route("/assets/app.css", get(get_app_css))
        .route("/api/state", get(get_state))
        .route("/api/events", get(get_events))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_and_auth_middleware,
        ))
        .with_state(state)
}

pub async fn start_server(
    config: ServerConfig,
    hub: DashboardHub,
) -> std::io::Result<DashboardServer> {
    let bind_check = check_bind(config.bind, config.allow_remote)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let warning = match bind_check {
        BindCheck::Remote { warning } => Some(warning),
        BindCheck::Loopback => None,
    };

    let bind_addr = SocketAddr::new(config.bind, config.port);
    let listener = match tokio::net::TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && config.port == DEFAULT_PORT => {
            tokio::net::TcpListener::bind(SocketAddr::new(config.bind, 0)).await?
        }
        Err(e) => return Err(e),
    };

    let addr = listener.local_addr()?;
    let port = addr.port();

    let allowed_hosts = if config.bind.is_loopback() {
        Some(vec![
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ])
    } else {
        None
    };

    let url = dashboard_url(addr, &config.token);
    let router = build_router(hub, config.token, allowed_hosts);

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    Ok(DashboardServer {
        addr,
        url,
        warning,
        handle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    use crate::graph::{GraphEdge, GraphNodeStatus, GraphRunPhase, NodeRole};
    use crate::web::auth::generate_token;
    use crate::web::snapshot::{GraphCounts, GraphView, NodeView, SnapshotSource};

    fn parse_sse_revisions(buf_str: &str) -> Vec<u64> {
        let mut revs = Vec::new();
        for line in buf_str.lines() {
            if let Some(json_str) = line.strip_prefix("data:") {
                let json_str = json_str.trim();
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str)
                    && let Some(rev) = val.get("revision").and_then(|r| r.as_u64())
                {
                    revs.push(rev);
                }
            }
        }
        revs
    }

    // TC-522: api_requires_token
    #[tokio::test]
    async fn api_requires_token() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/test".to_string());
        let token = "test-token-secret-12345".to_string();
        let allowed = Some(vec!["localhost:7420".to_string()]);
        let router = build_router(hub, token.clone(), allowed);

        // 1. GET /api/state without token -> 401
        let req_no_token = axum::http::Request::builder()
            .uri("/api/state")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_no_token).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(!body.contains("revision"));
        assert!(body.contains("unauthorized"));

        // 2. GET /api/state with wrong Bearer token -> 401
        let req_wrong_token = axum::http::Request::builder()
            .uri("/api/state")
            .header("Host", "localhost:7420")
            .header("Authorization", "Bearer wrong-token")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_wrong_token).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(!body.contains("revision"));

        // 3. GET /api/state with valid Bearer token -> 200
        let req_bearer = axum::http::Request::builder()
            .uri("/api/state")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_bearer).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CACHE_CONTROL)
                .unwrap(),
            "no-store"
        );
        assert_eq!(
            resp.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains(r#""schema":"dmuxai.dashboard/v1""#));

        // 4. GET /api/state with valid ?token= -> 200
        let req_query = axum::http::Request::builder()
            .uri(format!("/api/state?token={token}"))
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_query).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CACHE_CONTROL)
                .unwrap(),
            "no-store"
        );
        assert_eq!(
            resp.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains(r#""schema":"dmuxai.dashboard/v1""#));

        // 5. GET /api/state with token followed by 256 'a' -> 401
        let long_token = format!("{}{}", token, "a".repeat(256));
        let req_long = axum::http::Request::builder()
            .uri(format!("/api/state?token={long_token}"))
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_long).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 6. GET /api/state with empty ?token= -> 401
        let req_empty_token = axum::http::Request::builder()
            .uri("/api/state?token=")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_empty_token).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 7. GET /api/state with BEARER <token> (case-insensitive scheme) -> 200
        let req_bearer_caps = axum::http::Request::builder()
            .uri("/api/state")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("BEARER {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_bearer_caps).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 8. Negative test: //api/state without token -> != 200 and no "revision"
        let req_double_slash = axum::http::Request::builder()
            .uri("//api/state")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_double_slash).await.unwrap();
        assert_ne!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(!body.contains("revision"));

        // 9. Negative test: /api/../api/state without token -> != 200 and no "revision"
        let req_dotdot = axum::http::Request::builder()
            .uri("/api/../api/state")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.oneshot(req_dotdot).await.unwrap();
        assert_ne!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(!body.contains("revision"));
    }

    // TC-523: foreign_origin_and_host_are_rejected
    #[tokio::test]
    async fn foreign_origin_and_host_are_rejected() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/test".to_string());
        let token = "tok".to_string();
        let allowed = Some(vec![
            "127.0.0.1:7420".to_string(),
            "localhost:7420".to_string(),
        ]);
        let router = build_router(hub, token, allowed);

        // 1. Origin http://evil.example -> 403
        let req_bad_origin = axum::http::Request::builder()
            .uri("/")
            .header("Host", "127.0.0.1:7420")
            .header("Origin", "http://evil.example")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_bad_origin).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // 2. Host evil.example:7420 -> 403
        let req_bad_host = axum::http::Request::builder()
            .uri("/")
            .header("Host", "evil.example:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req_bad_host).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // 3. Origin http://localhost:7420 with matching Host -> 200
        let req_good = axum::http::Request::builder()
            .uri("/")
            .header("Host", "localhost:7420")
            .header("Origin", "http://localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.oneshot(req_good).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // TC-527: sse_streams_initial_and_updated_snapshots
    #[tokio::test]
    async fn sse_streams_initial_and_updated_snapshots() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let token = generate_token();
        let config = ServerConfig {
            bind: "127.0.0.1".parse().unwrap(),
            port: 0,
            allow_remote: false,
            token: token.clone(),
        };

        let server = start_server(config, hub.clone()).await.unwrap();

        let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
        let req = format!(
            "GET /api/events?token={token} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\n\r\n",
            server.addr
        );
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut buf = Vec::new();
        let mut temp_buf = [0u8; 1024];

        // Read initial snapshot event within 5s
        let initial_read = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let n = stream.read(&mut temp_buf).await.unwrap();
                if n == 0 {
                    panic!("stream closed prematurely");
                }
                buf.extend_from_slice(&temp_buf[..n]);
                let s = String::from_utf8_lossy(&buf);
                if s.contains("text/event-stream")
                    && s.contains("event: snapshot")
                    && parse_sse_revisions(&s).contains(&0)
                {
                    break;
                }
            }
        })
        .await;
        assert!(
            initial_read.is_ok(),
            "timed out waiting for initial snapshot event"
        );

        // Now publish a change
        let test_graph = GraphView {
            id: "run-sse-1".to_string(),
            phase: GraphRunPhase::Active,
            orchestrator_model: "gpt-4o".to_string(),
            user_prompt: "sse test".to_string(),
            current_nodes: vec!["n1".to_string()],
            waiting_gate_node: None,
            counts: GraphCounts {
                pending: 0,
                ready: 0,
                running: 1,
                waiting_gate: 0,
                completed: 0,
                failed: 0,
                skipped: 0,
            },
            nodes: vec![NodeView {
                id: "n1".to_string(),
                role: NodeRole::Implement,
                job: "sse job".to_string(),
                runner: "codex".to_string(),
                model_id: "m".to_string(),
                status: GraphNodeStatus::Running,
                attempt: 1,
                started_at_ms: None,
                finished_at_ms: None,
                elapsed_ms: None,
            }],
            edges: vec![GraphEdge {
                from: "a".to_string(),
                to: "b".to_string(),
            }],
        };
        hub.publish(Vec::new(), Some(test_graph));

        // Read updated snapshot event within 5s
        let update_read = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let n = stream.read(&mut temp_buf).await.unwrap();
                if n == 0 {
                    panic!("stream closed prematurely");
                }
                buf.extend_from_slice(&temp_buf[..n]);
                let s = String::from_utf8_lossy(&buf);
                if parse_sse_revisions(&s).contains(&1) {
                    break;
                }
            }
        })
        .await;
        assert!(
            update_read.is_ok(),
            "timed out waiting for updated snapshot event"
        );

        let s = String::from_utf8_lossy(&buf);
        let revs = parse_sse_revisions(&s);
        assert!(revs.contains(&0));
        assert!(revs.contains(&1));

        // Test server abort
        server.abort();
    }
}
