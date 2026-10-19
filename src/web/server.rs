use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Path, Query, Request, State};
use axum::http::header::HeaderName;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{UnboundedReceiverStream, WatchStream};

use crate::mcp::registry::{ImportError, McpRegistry};
use crate::memory::{MemoryError, MemoryHandle, Proposal, SaveOutcome};
use crate::todo::{NewTodo, TodoError, TodoPatch, TodoSource, TodoStatus};
use crate::web::DEFAULT_PORT;
use crate::web::auth::{BindCheck, check_bind, dashboard_url, token_matches};
use crate::web::commands::{DashboardCommand, RunMode, RunTodoError, SessionCommand};
use crate::web::hub::DashboardHub;

/// Injectable re-execution hook for `POST /api/workspace/switch` (FR-874, TC-875).
/// Arguments are `(executable, argv_after_arg0, dashboard_token)`.
pub type ExecFn =
    Arc<dyn Fn(&std::path::Path, &[String], &str) -> std::io::Result<()> + Send + Sync>;

/// Configuration and test hooks for workspace switching (FR-874).
#[derive(Clone, Default)]
pub struct WorkspaceSwitchConfig {
    /// Override for `std::env::current_exe()` (used by tests to simulate a missing binary).
    pub current_exe: Option<std::path::PathBuf>,
    /// Original CLI arguments after `argv[0]`.
    pub orig_args: Vec<String>,
    /// Bound port of the dashboard server (replaces `--port 0`).
    pub bound_port: Option<u16>,
    /// Override for platform support (`None` uses `cfg!(unix)`).
    pub unix_supported: Option<bool>,
    /// Optional exec hook called after preflight succeeds in router tests (TC-875).
    /// When `None`, sends `DashboardCommand::CommitSwitchWorkspace` so `main.rs` performs shutdown + exec.
    pub exec_hook: Option<ExecFn>,
    /// Optional exit hook called with `(exit_code, stderr_message)` when `exec_hook` returns `Err` (TC-875).
    pub on_exec_error: Option<Arc<dyn Fn(i32, String) + Send + Sync>>,
}

/// Formats the fatal stderr message when `exec` fails after clean shutdown (FR-874).
pub fn format_reexec_failure(
    _exe: &std::path::Path,
    args: &[String],
    error: &std::io::Error,
) -> String {
    let restart = std::iter::once("dmuxai".to_owned())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    format!("error: failed to re-execute dmuxai: {error}\nrestart command: {restart}\n")
}

/// Slice B dependencies injected into the router (FR-541, FR-542).
#[derive(Clone, Default)]
pub struct DashboardServices {
    pub todos: Option<Arc<crate::todo::TodoStore>>,
    pub commands: Option<mpsc::Sender<DashboardCommand>>,
    /// Merged MCP servers for `GET /api/mcp/servers` and `POST /api/import`
    /// (slice C); present in both `serve` and `--web`.
    pub mcp: Option<Arc<McpRegistry>>,
    /// True for a `dmuxai --web` server (in-process command channel by design),
    /// false for `dmuxai serve`. Lets a run request distinguish "this build has
    /// no to-do board because the store failed to open" (503) from "serves can
    /// never run to-dos" (409).
    pub web_attached: bool,
    /// Obsidian memory (SPEC-obsidian-memory FR-727); `None` = disabled.
    pub memory: Option<crate::memory::MemoryHandle>,
    /// Shared workspace event log for `GET /api/events/log` and SSE tailing.
    pub events_path: Option<std::path::PathBuf>,
    pub docs: Option<Arc<crate::web::docs_api::DocsService>>,
}

#[derive(Clone)]
pub struct ServerConfig {
    pub bind: IpAddr,
    pub port: u16,
    pub allow_remote: bool,
    pub token: String,
    pub services: DashboardServices,
}

#[derive(Clone)]
struct AppState {
    hub: DashboardHub,
    token: Arc<String>,
    allowed_hosts: Option<Arc<Vec<String>>>,
    services: DashboardServices,
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

    pub async fn stop(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

fn error_json(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({ "error": message }).to_string();
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        body,
    )
        .into_response()
}

fn json_ok<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_string(value) {
        Ok(body) => (
            status,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
                (HeaderName::from_static("x-content-type-options"), "nosniff"),
            ],
            body,
        )
            .into_response(),
        Err(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
}

fn todo_error_response(error: TodoError) -> Response {
    match error {
        TodoError::NotFound => error_json(StatusCode::NOT_FOUND, "to-do not found"),
        TodoError::Invalid(message) => error_json(StatusCode::BAD_REQUEST, &message),
        TodoError::Storage(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
}

fn todo_store(state: &AppState) -> Option<Arc<crate::todo::TodoStore>> {
    state.services.todos.clone()
}

fn refresh_todos(state: &AppState, store: &crate::todo::TodoStore) {
    state.hub.set_jira_base_url(store.jira_base_url());
    if let Ok(todos) = store.list() {
        state.hub.set_todos(todos);
    }
}

fn memory_handle(state: &AppState) -> Option<MemoryHandle> {
    state.services.memory.clone()
}

fn memory_not_configured() -> Response {
    error_json(StatusCode::CONFLICT, "memory is not configured")
}

fn memory_error_response(error: MemoryError) -> Response {
    match error {
        MemoryError::Rejected(reason) => error_json(StatusCode::BAD_REQUEST, &reason.to_string()),
        MemoryError::NotFound(_) => error_json(StatusCode::NOT_FOUND, "not found"),
        MemoryError::AlreadyDecided(status) => {
            error_json(StatusCode::CONFLICT, &format!("already {status}"))
        }
        MemoryError::CorruptQueue(_) | MemoryError::Io(_) => {
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

#[derive(serde::Serialize)]
struct MemoryGetEnabled {
    enabled: bool,
    project: String,
    dir: String,
    context: String,
    pending: Vec<Proposal>,
}

#[derive(serde::Deserialize)]
struct MemorySaveBody {
    text: String,
    #[serde(default)]
    target: Option<String>,
}

#[derive(serde::Serialize)]
struct MemorySaveResponse {
    saved: bool,
    target: String,
    message: String,
}

#[derive(serde::Serialize)]
struct MemoryActionResponse {
    ok: bool,
    message: String,
}

async fn get_memory(State(state): State<AppState>) -> Response {
    let Some(memory) = memory_handle(&state) else {
        return json_ok(StatusCode::OK, &serde_json::json!({ "enabled": false }));
    };
    let project = memory.project.clone();
    let context = memory.store.build_context(&project);
    let pending = match memory.store.list_pending() {
        Ok(items) => items,
        Err(error) => return memory_error_response(error),
    };
    json_ok(
        StatusCode::OK,
        &MemoryGetEnabled {
            enabled: true,
            project,
            dir: memory.dir.clone(),
            context,
            pending,
        },
    )
}

async fn post_memory_save(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<MemorySaveBody>,
) -> Response {
    let Some(memory) = memory_handle(&state) else {
        return memory_not_configured();
    };
    let target = body
        .target
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| memory.project_target());
    let today = crate::memory::today_utc();
    match memory
        .store
        .save(&target, &body.text, &memory.src(), &today)
    {
        Ok(SaveOutcome::Saved { target }) => {
            let message = format!("Saved to {target}");
            json_ok(
                StatusCode::OK,
                &MemorySaveResponse {
                    saved: true,
                    target,
                    message,
                },
            )
        }
        Ok(SaveOutcome::AlreadySaved { target }) => {
            let message = format!("Already in {target}");
            json_ok(
                StatusCode::OK,
                &MemorySaveResponse {
                    saved: false,
                    target,
                    message,
                },
            )
        }
        Err(error) => memory_error_response(error),
    }
}

async fn post_memory_proposal_approve(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(memory) = memory_handle(&state) else {
        return memory_not_configured();
    };
    let today = crate::memory::today_utc();
    match memory.store.approve(&id, &today) {
        Ok(SaveOutcome::Saved { target }) => {
            let message = format!("Approved {id}: saved to {target}");
            json_ok(StatusCode::OK, &MemoryActionResponse { ok: true, message })
        }
        Ok(SaveOutcome::AlreadySaved { target }) => {
            let message = format!("Approved {id}: already in {target}");
            json_ok(StatusCode::OK, &MemoryActionResponse { ok: true, message })
        }
        Err(error) => memory_error_response(error),
    }
}

async fn post_memory_proposal_reject(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(memory) = memory_handle(&state) else {
        return memory_not_configured();
    };
    match memory.store.reject(&id) {
        Ok(()) => json_ok(
            StatusCode::OK,
            &MemoryActionResponse {
                ok: true,
                message: format!("Rejected {id}"),
            },
        ),
        Err(error) => memory_error_response(error),
    }
}

/// JSON extractor that maps a missing/wrong content type to 415 and a bad
/// body to 400, both with an `{"error": "..."}` body (FR-541).
struct ApiJson<T>(T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => {
                let (status, message) = match rejection {
                    JsonRejection::MissingJsonContentType(_) => {
                        (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported media type")
                    }
                    JsonRejection::JsonSyntaxError(_) | JsonRejection::JsonDataError(_) => {
                        (StatusCode::BAD_REQUEST, "invalid json")
                    }
                    JsonRejection::BytesRejection(_) => {
                        (StatusCode::PAYLOAD_TOO_LARGE, "request too large")
                    }
                    _ => (StatusCode::BAD_REQUEST, "invalid request"),
                };
                Err(error_json(status, message))
            }
        }
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

async fn get_session_css() -> impl IntoResponse {
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
        crate::web::assets::SESSION_CSS,
    )
}

async fn get_memory_js() -> impl IntoResponse {
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
        crate::web::assets::MEMORY_JS,
    )
}

async fn get_session_js() -> impl IntoResponse {
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
        crate::web::assets::SESSION_JS,
    )
}

async fn get_workspace_js() -> impl IntoResponse {
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
        crate::web::assets::WORKSPACE_JS,
    )
}

async fn get_sidebar_resize_js() -> impl IntoResponse {
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
        crate::web::assets::SIDEBAR_RESIZE_JS,
    )
}

async fn get_favicon_idle() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'self'",
            ),
            (HeaderName::from_static("referrer-policy"), "no-referrer"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        crate::web::assets::FAVICON_IDLE_SVG,
    )
}

async fn get_favicon_running() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'self'",
            ),
            (HeaderName::from_static("referrer-policy"), "no-referrer"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        crate::web::assets::FAVICON_RUNNING_SVG,
    )
}

async fn get_favicon_done() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'self'",
            ),
            (HeaderName::from_static("referrer-policy"), "no-referrer"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        crate::web::assets::FAVICON_DONE_SVG,
    )
}

async fn get_favicon_failed() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'self'",
            ),
            (HeaderName::from_static("referrer-policy"), "no-referrer"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        crate::web::assets::FAVICON_FAILED_SVG,
    )
}

async fn get_state(State(state): State<AppState>) -> Response {
    let mut snapshot = state.hub.snapshot();
    snapshot.sent_at_ms = crate::web::snapshot::now_ms();
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

#[derive(serde::Deserialize, Default)]
struct EventsQueryParams {
    since: Option<String>,
    limit: Option<String>,
}

fn parse_events_since(raw: Option<&str>, default: u64) -> Result<u64, StatusCode> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => value.parse().map_err(|_| StatusCode::BAD_REQUEST),
    }
}

fn parse_events_limit(raw: Option<&str>, default: usize, cap: usize) -> Result<usize, StatusCode> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| StatusCode::BAD_REQUEST)
            .map(|parsed| parsed.min(cap)),
    }
}

fn state_change_sse_event(record: &crate::events::EventRecord) -> Result<Event, Infallible> {
    let data = serde_json::to_string(record).unwrap_or_else(|_| "{}".to_owned());
    Ok(Event::default()
        .event("state-change")
        .id(record.seq.to_string())
        .data(data))
}

async fn get_events_log(
    State(state): State<AppState>,
    Query(params): Query<EventsQueryParams>,
) -> Response {
    let since = match parse_events_since(params.since.as_deref(), 0) {
        Ok(since) => since,
        Err(_) => return error_json(StatusCode::BAD_REQUEST, "invalid since"),
    };
    let limit = match parse_events_limit(params.limit.as_deref(), 200, 1000) {
        Ok(limit) => limit,
        Err(_) => return error_json(StatusCode::BAD_REQUEST, "invalid limit"),
    };
    let Some(path) = state.services.events_path.as_ref() else {
        return json_ok(
            StatusCode::OK,
            &serde_json::json!({
                "events": [],
                "last_seq": since,
            }),
        );
    };
    let events = crate::events::read_since(path, since, Some(limit));
    let last_seq = events.last().map(|record| record.seq).unwrap_or(since);
    json_ok(
        StatusCode::OK,
        &serde_json::json!({
            "events": events,
            "last_seq": last_seq,
        }),
    )
}

async fn get_events(
    State(state): State<AppState>,
    Query(params): Query<EventsQueryParams>,
) -> Response {
    // Validate `since` even when no event log is configured.
    let requested_since = match params.since.as_deref() {
        None | Some("") => None,
        Some(value) => match value.parse::<u64>() {
            Ok(since) => Some(since),
            Err(_) => return error_json(StatusCode::BAD_REQUEST, "invalid since"),
        },
    };
    let tail = if let Some(path) = state.services.events_path.clone() {
        let replay_since = requested_since.unwrap_or_else(|| crate::events::last_event_seq(&path));
        Some((path, replay_since))
    } else {
        None
    };

    let (tx, rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let hub = state.hub.clone();
    let snapshot_tx = tx.clone();
    tokio::spawn(async move {
        let mut watch_stream = WatchStream::new(hub.subscribe());
        while let Some(mut snapshot) = watch_stream.next().await {
            snapshot.sent_at_ms = crate::web::snapshot::now_ms();
            if let Ok(json) = serde_json::to_string(&snapshot) {
                let event = Event::default().event("snapshot").data(json);
                if snapshot_tx.send(Ok(event)).is_err() {
                    break;
                }
            }
        }
    });

    if let Some((path, replay_since)) = tail {
        tokio::spawn(async move {
            let mut cursor = replay_since;
            for record in crate::events::read_since(&path, cursor, None) {
                cursor = record.seq;
                if tx.send(state_change_sse_event(&record)).is_err() {
                    return;
                }
            }
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            interval.tick().await;
            loop {
                // Stop polling once the client has gone away, even when no
                // new events arrive to surface the closed channel.
                if tx.is_closed() {
                    return;
                }
                for record in crate::events::read_since(&path, cursor, None) {
                    cursor = record.seq;
                    if tx.send(state_change_sse_event(&record)).is_err() {
                        return;
                    }
                }
                interval.tick().await;
            }
        });
    }

    let stream = UnboundedReceiverStream::new(rx);
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

#[derive(serde::Deserialize)]
struct NewTodoBody {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    external_key: Option<String>,
    #[serde(default)]
    link_url: Option<String>,
    /// Import-only (FR-563); presence of either field is a 400.
    #[serde(default)]
    external_status: Option<serde_json::Value>,
    #[serde(default)]
    external_assignee: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct PatchTodoBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    external_key: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    link_url: Option<Option<String>>,
    /// Import-only (FR-563); presence of either field is a 400.
    #[serde(default)]
    external_status: Option<serde_json::Value>,
    #[serde(default)]
    external_assignee: Option<serde_json::Value>,
}

/// FR-563: `external_status`/`external_assignee` are set by import only; a
/// user POST/PATCH that includes either is rejected. Returns the 400 response
/// to send, or `None` when the body is fine.
fn reject_import_only_fields(
    status: &Option<serde_json::Value>,
    assignee: &Option<serde_json::Value>,
) -> Option<Response> {
    if status.is_some() {
        return Some(error_json(
            StatusCode::BAD_REQUEST,
            "external_status is set by import",
        ));
    }
    if assignee.is_some() {
        return Some(error_json(
            StatusCode::BAD_REQUEST,
            "external_assignee is set by import",
        ));
    }
    None
}

/// Distinguishes an absent field (`None`) from an explicit `null`
/// (`Some(None)`) so PATCH can clear `external_key`/`link_url` (FR-563).
fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<String> = serde::Deserialize::deserialize(deserializer)?;
    Ok(Some(value))
}

#[derive(serde::Deserialize)]
struct RunTodoBody {
    mode: String,
    #[serde(default)]
    agent: Option<String>,
}

async fn list_todos(State(state): State<AppState>) -> Response {
    let Some(store) = todo_store(&state) else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "to-dos unavailable");
    };
    match store.list() {
        Ok(todos) => json_ok(StatusCode::OK, &todos),
        Err(error) => todo_error_response(error),
    }
}

async fn create_todo(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<NewTodoBody>,
) -> Response {
    let Some(store) = todo_store(&state) else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "to-dos unavailable");
    };
    if let Some(response) =
        reject_import_only_fields(&body.external_status, &body.external_assignee)
    {
        return response;
    }
    let new = NewTodo {
        title: body.title,
        body: body.body,
        source: TodoSource::Manual,
        source_ref: None,
    };
    match store.create_with_link(new, body.external_key, body.link_url) {
        Ok(todo) => {
            refresh_todos(&state, &store);
            json_ok(StatusCode::CREATED, &todo)
        }
        Err(error) => todo_error_response(error),
    }
}

async fn update_todo(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<PatchTodoBody>,
) -> Response {
    let Some(store) = todo_store(&state) else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "to-dos unavailable");
    };
    if let Some(response) =
        reject_import_only_fields(&body.external_status, &body.external_assignee)
    {
        return response;
    }
    let status = match body.status.as_deref() {
        None => None,
        Some("open") => Some(TodoStatus::Open),
        Some("done") => Some(TodoStatus::Done),
        Some("running") => Some(TodoStatus::Running),
        Some(_) => return error_json(StatusCode::BAD_REQUEST, "invalid status"),
    };
    let patch = TodoPatch {
        title: body.title,
        body: body.body,
        status,
        external_key: body.external_key,
        link_url: body.link_url,
    };
    match store.update(&id, patch) {
        Ok(todo) => {
            refresh_todos(&state, &store);
            json_ok(StatusCode::OK, &todo)
        }
        Err(error) => todo_error_response(error),
    }
}

async fn delete_todo(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(store) = todo_store(&state) else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "to-dos unavailable");
    };
    match store.delete(&id) {
        Ok(()) => {
            refresh_todos(&state, &store);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => todo_error_response(error),
    }
}

async fn run_todo(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<RunTodoBody>,
) -> Response {
    let mode = match body.mode.as_str() {
        "agent" => match body.agent {
            Some(agent) if !agent.trim().is_empty() => RunMode::Agent {
                agent: agent.trim().to_owned(),
            },
            _ => return error_json(StatusCode::BAD_REQUEST, "mode agent requires an agent id"),
        },
        "graph" => RunMode::Graph,
        _ => return error_json(StatusCode::BAD_REQUEST, "invalid mode"),
    };

    // `--web` without a store could not be made runnable; report the missing
    // board (503) rather than the `serve` "requires dmuxai --web" hint.
    if state.services.todos.is_none() && state.services.web_attached {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "to-do board unavailable");
    }

    let Some(sender) = state.services.commands.clone() else {
        return error_json(StatusCode::CONFLICT, "requires dmuxai --web");
    };

    let (reply, rx) = oneshot::channel();
    if sender
        .send(DashboardCommand::RunTodo {
            todo_id: id,
            mode,
            reply,
        })
        .await
        .is_err()
    {
        return error_json(StatusCode::CONFLICT, "requires dmuxai --web");
    }

    let result = match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Err(_) => return error_json(StatusCode::GATEWAY_TIMEOUT, "run request timed out"),
        Ok(Err(_)) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        Ok(Ok(Err(error))) => return run_error_response(error),
        Ok(Ok(Ok(todo))) => todo,
    };

    if let Some(store) = state.services.todos.clone()
        && let Ok(todos) = store.list()
    {
        state.hub.set_todos(todos);
    }
    json_ok(StatusCode::ACCEPTED, &result)
}

#[derive(serde::Deserialize)]
struct PromptBody {
    text: String,
    agent: Option<String>,
    mode: Option<String>,
}

async fn post_prompt(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<PromptBody>,
) -> Response {
    if body.text.trim().is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "text is required");
    }

    if let Some(ref mode) = body.mode {
        let m = mode.trim().to_ascii_lowercase();
        if !matches!(m.as_str(), "ask" | "plan" | "implement") {
            return error_json(StatusCode::BAD_REQUEST, "invalid mode");
        }
    }

    let Some(sender) = state.services.commands.clone() else {
        return error_json(StatusCode::CONFLICT, "requires dmuxai --web");
    };

    let (reply, rx) = oneshot::channel();
    if sender
        .send(DashboardCommand::SubmitPrompt {
            text: body.text,
            agent: body.agent,
            mode: body.mode,
            reply,
        })
        .await
        .is_err()
    {
        return error_json(StatusCode::CONFLICT, "requires dmuxai --web");
    }

    let result = match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Err(_) => return error_json(StatusCode::GATEWAY_TIMEOUT, "prompt accept timed out"),
        Ok(Err(_)) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        Ok(Ok(Err(error))) => return run_error_response(error),
        Ok(Ok(Ok(accepted))) => accepted,
    };

    json_ok(
        StatusCode::ACCEPTED,
        &serde_json::json!({
            "accepted": true,
            "conversation_id": result.conversation_id,
            "agent": result.agent,
        }),
    )
}

/// `POST /api/command`: picker and lifecycle actions (SPEC-dashboard-parity
/// FR-781). Answers with the updated session.
async fn post_command(
    State(state): State<AppState>,
    ApiJson(command): ApiJson<SessionCommand>,
) -> Response {
    let Some(sender) = state.services.commands.clone() else {
        return error_json(StatusCode::CONFLICT, "requires dmuxai --web");
    };
    let (reply, rx) = oneshot::channel();
    if sender
        .send(DashboardCommand::Session { command, reply })
        .await
        .is_err()
    {
        return error_json(StatusCode::CONFLICT, "requires dmuxai --web");
    }
    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Err(_) => error_json(StatusCode::GATEWAY_TIMEOUT, "command timed out"),
        Ok(Err(_)) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        Ok(Ok(Err(error))) => run_error_response(error),
        Ok(Ok(Ok(session))) => json_ok(
            StatusCode::OK,
            &serde_json::json!({ "ok": true, "session": session }),
        ),
    }
}

fn run_error_response(error: RunTodoError) -> Response {
    match error {
        RunTodoError::Busy(message) => error_json(StatusCode::CONFLICT, &message),
        RunTodoError::AlreadyRunning => {
            error_json(StatusCode::CONFLICT, "to-do is already running")
        }
        RunTodoError::Forbidden(message) => error_json(StatusCode::FORBIDDEN, &message),
        RunTodoError::UnknownAgent(message) => error_json(StatusCode::NOT_FOUND, &message),
        RunTodoError::NotFound => error_json(StatusCode::NOT_FOUND, "to-do not found"),
        RunTodoError::Missing(message) => error_json(StatusCode::NOT_FOUND, &message),
        RunTodoError::Invalid(message) => error_json(StatusCode::BAD_REQUEST, &message),
        RunTodoError::Internal(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
}

#[derive(serde::Deserialize)]
struct ImportBody {
    server: String,
    query: String,
}

/// GET /api/mcp/servers (FR-562): redacted merged server list.
async fn list_mcp_servers(State(state): State<AppState>) -> Response {
    let Some(registry) = state.services.mcp.clone() else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "mcp unavailable");
    };
    json_ok(StatusCode::OK, &registry.servers_view())
}

/// POST /api/import (FR-561): import Jira/Confluence items as to-dos.
async fn import_todos(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ImportBody>,
) -> Response {
    let Some(registry) = state.services.mcp.clone() else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "mcp unavailable");
    };
    let Some(store) = todo_store(&state) else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "to-dos unavailable");
    };
    match registry
        .import(body.server.trim(), body.query.trim(), &store)
        .await
    {
        Ok(result) => {
            refresh_todos(&state, &store);
            json_ok(StatusCode::OK, &result)
        }
        Err(error) => import_error_response(error),
    }
}

fn import_error_response(error: ImportError) -> Response {
    match error {
        ImportError::UnknownServer(server) => error_json(
            StatusCode::NOT_FOUND,
            &format!("unknown mcp server: {server}"),
        ),
        ImportError::WrongKind { server, kind } => error_json(
            StatusCode::BAD_REQUEST,
            &format!("mcp server {server} is kind {kind}, not jira/confluence"),
        ),
        ImportError::Invalid(message) => error_json(StatusCode::BAD_REQUEST, &message),
        ImportError::Tool { server, message } => {
            error_json(StatusCode::BAD_GATEWAY, &format!("{server}: {message}"))
        }
        ImportError::Storage(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
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

fn is_unix_switch_supported(state: &AppState) -> bool {
    state
        .hub
        .switch_config()
        .and_then(|c| c.unix_supported)
        .unwrap_or(cfg!(unix))
}

fn preflight_workspace_switch(
    exe_override: Option<&std::path::Path>,
    target: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let exe = match exe_override {
        Some(path) => path.to_path_buf(),
        None => std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?,
    };
    let meta = std::fs::metadata(&exe)
        .map_err(|_| format!("executable does not exist: {}", exe.display()))?;
    if !meta.is_file() {
        return Err(format!("executable is not a file: {}", exe.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(format!("executable is not executable: {}", exe.display()));
        }
    }
    crate::workspace::validate_workspace_dir(target)
        .map_err(|e| format!("target workspace invalid: {e}"))?;
    Ok(exe)
}

#[derive(serde::Serialize)]
struct WorkspaceInfoResponse {
    current: String,
    recent: Vec<String>,
    browse_roots: Vec<String>,
    can_switch: bool,
}

async fn get_workspace(State(state): State<AppState>) -> Response {
    let current = state.hub.snapshot().workspace;
    let recent = crate::workspace::load_recent_workspaces();
    let browse_roots = crate::workspace::resolve_browse_roots()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let can_switch = state.services.web_attached && is_unix_switch_supported(&state);
    json_ok(
        StatusCode::OK,
        &WorkspaceInfoResponse {
            current,
            recent,
            browse_roots,
            can_switch,
        },
    )
}

#[derive(serde::Deserialize, Default)]
struct WorkspaceDirsQuery {
    path: Option<String>,
    hidden: Option<String>,
}

#[derive(serde::Serialize)]
struct WorkspaceDirsResponse {
    path: String,
    parent: Option<String>,
    dirs: Vec<String>,
}

async fn get_workspace_dirs(
    State(state): State<AppState>,
    Query(params): Query<WorkspaceDirsQuery>,
) -> Response {
    if !state.services.web_attached {
        return error_json(StatusCode::FORBIDDEN, "view-only: start dmuxai --web");
    }
    let roots = crate::workspace::resolve_browse_roots();
    let raw_path = params
        .path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| roots.first().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| state.hub.snapshot().workspace);

    if raw_path.as_bytes().contains(&0) || raw_path.len() > 4096 {
        return error_json(StatusCode::BAD_REQUEST, "invalid path");
    }
    let expanded = crate::settings::expand_home(&raw_path);
    let metadata = match std::fs::metadata(&expanded) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return error_json(StatusCode::BAD_REQUEST, "does not exist");
        }
        Err(_) => return error_json(StatusCode::BAD_REQUEST, "invalid path"),
    };
    if !metadata.is_dir() {
        return error_json(StatusCode::BAD_REQUEST, "not a directory");
    }
    let canonical = match std::fs::canonicalize(&expanded) {
        Ok(c) => c,
        Err(_) => return error_json(StatusCode::BAD_REQUEST, "invalid path"),
    };
    if let Err(denied) = crate::workspace::check_workspace_denied(&canonical) {
        return error_json(StatusCode::FORBIDDEN, &denied.to_string());
    }
    if !roots.iter().any(|root| canonical.starts_with(root)) {
        return error_json(StatusCode::FORBIDDEN, "outside browse roots");
    }

    let entries = match std::fs::read_dir(&canonical) {
        Ok(rd) => rd,
        Err(_) => return error_json(StatusCode::FORBIDDEN, "not writable"),
    };
    let show_hidden = matches!(params.hidden.as_deref(), Some("1") | Some("true"));
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || (!show_hidden && name.starts_with('.')) {
            continue;
        }
        let Ok(canon_child) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        if !canon_child.is_dir() {
            continue;
        }
        if !roots.iter().any(|root| canon_child.starts_with(root)) {
            continue;
        }
        if crate::workspace::check_workspace_denied(&canon_child).is_err() {
            continue;
        }
        dirs.push(name);
    }
    dirs.sort();
    dirs.dedup();
    dirs.truncate(500);

    let parent = canonical
        .parent()
        .filter(|p| {
            roots.iter().any(|root| p.starts_with(root))
                && crate::workspace::check_workspace_denied(p).is_ok()
        })
        .map(|p| p.to_string_lossy().into_owned());

    json_ok(
        StatusCode::OK,
        &WorkspaceDirsResponse {
            path: canonical.to_string_lossy().into_owned(),
            parent,
            dirs,
        },
    )
}

#[derive(serde::Deserialize)]
struct WorkspacePathBody {
    path: String,
}

async fn post_workspace_validate(ApiJson(body): ApiJson<WorkspacePathBody>) -> Response {
    match crate::workspace::validate_workspace_dir(std::path::Path::new(&body.path)) {
        Ok(canonical) => json_ok(
            StatusCode::OK,
            &serde_json::json!({
                "ok": true,
                "path": canonical.to_string_lossy(),
            }),
        ),
        Err(reason) => json_ok(
            StatusCode::OK,
            &serde_json::json!({
                "ok": false,
                "reason": reason.to_string(),
            }),
        ),
    }
}

async fn post_workspace_switch(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<WorkspacePathBody>,
) -> Response {
    if !state.services.web_attached {
        return error_json(StatusCode::FORBIDDEN, "view-only: start dmuxai --web");
    }
    if !is_unix_switch_supported(&state) {
        return error_json(
            StatusCode::NOT_IMPLEMENTED,
            "workspace switch is not supported on this platform",
        );
    }
    let canonical = match crate::workspace::validate_workspace_dir(std::path::Path::new(&body.path))
    {
        Ok(path) => path,
        Err(reason) => return error_json(StatusCode::BAD_REQUEST, &reason.to_string()),
    };

    // Run the rest in its own task: once the App has set switch-pending, the
    // flag must be committed or cancelled even if the client disconnects and
    // axum drops this handler future.
    tokio::spawn(workspace_switch_task(state, canonical))
        .await
        .unwrap_or_else(|_| error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal"))
}

/// How long `POST /api/workspace/switch` waits for the App (shortened in tests).
const SWITCH_REPLY_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(10)
};

async fn workspace_switch_task(state: AppState, canonical: std::path::PathBuf) -> Response {
    let Some(sender) = state.services.commands.clone() else {
        return error_json(StatusCode::CONFLICT, "requires dmuxai --web");
    };
    let (reply, rx) = oneshot::channel();
    if sender
        .send(DashboardCommand::SwitchWorkspace {
            path: canonical.clone(),
            reply,
        })
        .await
        .is_err()
    {
        return error_json(StatusCode::CONFLICT, "requires dmuxai --web");
    }
    match tokio::time::timeout(SWITCH_REPLY_TIMEOUT, rx).await {
        Err(_) | Ok(Err(_)) => {
            // The App may still set switch-pending after we give up; cancel so
            // prompts are not refused until restart.
            let notice = "Workspace switch failed: the app did not respond".to_owned();
            let _ = tokio::time::timeout(
                SWITCH_REPLY_TIMEOUT,
                sender.send(DashboardCommand::CancelSwitchWorkspace {
                    notice: notice.clone(),
                }),
            )
            .await;
            return error_json(StatusCode::GATEWAY_TIMEOUT, &notice);
        }
        Ok(Ok(Err(error))) => return run_error_response(error),
        Ok(Ok(Ok(()))) => {}
    }

    let _ = crate::workspace::record_recent_workspace(&canonical);

    let cfg = state.hub.switch_config().unwrap_or_default();
    match preflight_workspace_switch(cfg.current_exe.as_deref(), &canonical) {
        Err(reason) => {
            let notice = format!("Workspace switch failed: {reason}");
            let _ = sender
                .send(DashboardCommand::CancelSwitchWorkspace {
                    notice: notice.clone(),
                })
                .await;
            state.hub.push_notice(notice.clone());
            tokio::task::yield_now().await;
            return error_json(StatusCode::INTERNAL_SERVER_ERROR, &notice);
        }
        Ok(exe) => {
            let reexec_args =
                crate::workspace::build_reexec_args(&cfg.orig_args, &canonical, cfg.bound_port);
            if let Some(exec_hook) = cfg.exec_hook {
                if let Err(err) = exec_hook(&exe, &reexec_args, &state.token) {
                    let msg = format_reexec_failure(&exe, &reexec_args, &err);
                    if let Some(on_err) = cfg.on_exec_error {
                        on_err(1, msg);
                    } else {
                        eprint!("{msg}");
                        std::process::exit(1);
                    }
                }
            } else {
                let canonical_for_commit = canonical.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = sender
                        .send(DashboardCommand::CommitSwitchWorkspace {
                            path: canonical_for_commit,
                        })
                        .await;
                });
            }
        }
    }

    json_ok(
        StatusCode::ACCEPTED,
        &serde_json::json!({
            "ok": true,
            "path": canonical.to_string_lossy(),
        }),
    )
}

pub fn build_router(
    hub: DashboardHub,
    token: String,
    allowed_hosts: Option<Vec<String>>,
) -> Router {
    build_router_with(hub, token, allowed_hosts, DashboardServices::default())
}

pub fn build_router_with(
    hub: DashboardHub,
    token: String,
    allowed_hosts: Option<Vec<String>>,
    services: DashboardServices,
) -> Router {
    let state = AppState {
        hub,
        token: Arc::new(token),
        allowed_hosts: allowed_hosts.map(Arc::new),
        services,
    };

    Router::new()
        .route("/", get(get_index))
        .route("/assets/app.js", get(get_app_js))
        .route("/assets/memory.js", get(get_memory_js))
        .route("/assets/session.js", get(get_session_js))
        .route("/assets/workspace.js", get(get_workspace_js))
        .route("/assets/sidebar-resize.js", get(get_sidebar_resize_js))
        .route("/assets/session.css", get(get_session_css))
        .route("/assets/app.css", get(get_app_css))
        .route("/assets/favicon-idle.svg", get(get_favicon_idle))
        .route("/assets/favicon-running.svg", get(get_favicon_running))
        .route("/assets/favicon-done.svg", get(get_favicon_done))
        .route("/assets/favicon-failed.svg", get(get_favicon_failed))
        .route("/api/state", get(get_state))
        .route("/api/workspace", get(get_workspace))
        .route("/api/workspace/dirs", get(get_workspace_dirs))
        .route("/api/workspace/validate", post(post_workspace_validate))
        .route("/api/workspace/switch", post(post_workspace_switch))
        .route("/api/memory", get(get_memory))
        .route("/api/memory/save", post(post_memory_save))
        .route(
            "/api/memory/proposals/{id}/approve",
            post(post_memory_proposal_approve),
        )
        .route(
            "/api/memory/proposals/{id}/reject",
            post(post_memory_proposal_reject),
        )
        .route("/api/events", get(get_events))
        .route("/api/events/log", get(get_events_log))
        .route("/api/todos", get(list_todos).post(create_todo))
        .route("/api/todos/{id}", patch(update_todo).delete(delete_todo))
        .route("/api/todos/{id}/run", post(run_todo))
        .route("/api/mcp/servers", get(list_mcp_servers))
        .route("/api/import", post(import_todos))
        .route("/api/prompt", post(post_prompt))
        .route("/api/command", post(post_command))
        .merge(crate::web::docs_api::router(state.services.docs.clone()))
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
    let mut switch_cfg = hub
        .switch_config()
        .unwrap_or_else(|| WorkspaceSwitchConfig {
            orig_args: std::env::args().skip(1).collect(),
            ..WorkspaceSwitchConfig::default()
        });
    switch_cfg.bound_port = Some(port);
    hub.set_switch_config(switch_cfg);
    let router = build_router_with(hub, config.token, allowed_hosts, config.services);

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

    use crate::graph::{GraphEdge, GraphNodeStatus, GraphRunPhase, NodeRole, PlannerCalls};
    use crate::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
    use crate::todo::{Todo, TodoStore};
    use crate::web::auth::generate_token;
    use crate::web::commands::PromptAccepted;
    use crate::web::snapshot::{GraphCounts, GraphView, NodeDetail, NodeView, SnapshotSource};

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
            services: DashboardServices::default(),
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
            layers: 1,
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
                deps: Vec::new(),
                layout: None,
                detail: NodeDetail::default(),
                rev_change: None,
                task: None,
            }],
            edges: vec![GraphEdge {
                from: "a".to_string(),
                to: "b".to_string(),
            }],
            triage: None,
            approval: None,
            plan_history: Vec::new(),
            tasks: Vec::new(),
            planner_calls: PlannerCalls::default(),
            planner_io: Vec::new(),
            turns: Vec::new(),
            earlier_turns: 0,
            follow_up: Vec::new(),
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

    // ---- slice B test helpers -------------------------------------------

    fn test_hub() -> DashboardHub {
        DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string())
    }

    fn allowed_localhost() -> Option<Vec<String>> {
        Some(vec!["localhost:7420".to_string()])
    }

    fn json_request(
        method: &str,
        uri: &str,
        token: &str,
        body: &str,
    ) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("Host", "localhost:7420")
            .header("Origin", "http://localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body.to_owned()))
            .unwrap()
    }

    async fn read_body(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn revision_of(body: &str) -> Option<u64> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value.get("revision").and_then(|r| r.as_u64()))
    }

    fn test_todo() -> Todo {
        Todo {
            id: "abc".to_owned(),
            title: "run me".to_owned(),
            body: String::new(),
            status: TodoStatus::Running,
            source: TodoSource::Manual,
            source_ref: None,
            external_key: None,
            link_url: None,
            external_status: None,
            external_assignee: None,
            run_kind: Some("agent".to_owned()),
            run_ref: Some("codex".to_owned()),
            last_error: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn spawn_reply(outcome: Result<Todo, RunTodoError>) -> mpsc::Sender<DashboardCommand> {
        let (tx, mut rx) = mpsc::channel(16);
        tokio::spawn(async move {
            while let Some(command) = rx.recv().await {
                if let DashboardCommand::RunTodo { reply, .. } = command {
                    let _ = reply.send(outcome.clone());
                }
            }
        });
        tx
    }

    fn spawn_recording_reply(
        outcome: Result<Todo, RunTodoError>,
        seen: Arc<std::sync::Mutex<Vec<RunMode>>>,
    ) -> mpsc::Sender<DashboardCommand> {
        let (tx, mut rx) = mpsc::channel(16);
        tokio::spawn(async move {
            while let Some(command) = rx.recv().await {
                if let DashboardCommand::RunTodo { mode, reply, .. } = command {
                    seen.lock().unwrap().push(mode);
                    let _ = reply.send(outcome.clone());
                }
            }
        });
        tx
    }

    fn spawn_prompt_reply(
        outcome: Result<PromptAccepted, RunTodoError>,
    ) -> mpsc::Sender<DashboardCommand> {
        let (tx, mut rx) = mpsc::channel(16);
        tokio::spawn(async move {
            while let Some(command) = rx.recv().await {
                if let DashboardCommand::SubmitPrompt { reply, .. } = command {
                    let _ = reply.send(outcome.clone());
                }
            }
        });
        tx
    }

    fn spawn_session_reply(
        outcome: Result<crate::web::SessionView, RunTodoError>,
    ) -> (
        mpsc::Sender<DashboardCommand>,
        Arc<std::sync::Mutex<Vec<SessionCommand>>>,
    ) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_task = Arc::clone(&seen);
        let (tx, mut rx) = mpsc::channel(16);
        tokio::spawn(async move {
            while let Some(command) = rx.recv().await {
                if let DashboardCommand::Session { command, reply } = command {
                    seen_task.lock().unwrap().push(command);
                    let _ = reply.send(outcome.clone());
                }
            }
        });
        (tx, seen)
    }

    // TC-783 (FR-781): `POST /api/command` parses every action, maps App
    // refusals to HTTP statuses, needs the token and answers 409 in `serve`.
    #[tokio::test]
    async fn command_route_contract() {
        let token = "command-token-123".to_string();
        let session = crate::web::SessionView {
            conversation_id: "conv-1".to_owned(),
            selected_agent: "codex".to_owned(),
            ..crate::web::SessionView::default()
        };
        let router_with = |commands: mpsc::Sender<DashboardCommand>| {
            build_router_with(
                test_hub(),
                token.clone(),
                allowed_localhost(),
                DashboardServices {
                    commands: Some(commands),
                    web_attached: true,
                    ..DashboardServices::default()
                },
            )
        };

        let (tx, seen) = spawn_session_reply(Ok(session));
        let router = router_with(tx);
        let bodies = [
            r#"{"command":"select_agent","agent":"agy"}"#,
            r#"{"command":"set_mode","mode":"plan"}"#,
            r#"{"command":"load_models","agent":"codex"}"#,
            r#"{"command":"select_model","agent":"codex","model":"m1"}"#,
            r#"{"command":"set_effort","effort":"high"}"#,
            r#"{"command":"cancel","agent":"codex"}"#,
            r#"{"command":"new_conversation","title":"T"}"#,
            r#"{"command":"load_conversation","id":"conv-1"}"#,
            r#"{"command":"share","target":"cursor"}"#,
            r#"{"command":"status"}"#,
            r#"{"command":"toggle_details"}"#,
            r#"{"command":"quit"}"#,
        ];
        for body in bodies {
            let resp = router
                .clone()
                .oneshot(json_request("POST", "/api/command", &token, body))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{body}");
            let json: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
            assert_eq!(json["ok"], true);
            assert_eq!(json["session"]["conversation_id"], "conv-1");
        }
        assert_eq!(seen.lock().unwrap().len(), bodies.len());
        assert_eq!(
            seen.lock().unwrap()[1],
            SessionCommand::SetMode {
                agent: None,
                mode: "plan".to_owned()
            }
        );

        for body in [
            r#"{"command":"warp"}"#,
            r#"{"command":"select_agent"}"#,
            r#"not json"#,
        ] {
            let resp = router
                .clone()
                .oneshot(json_request("POST", "/api/command", &token, body))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
        }

        for (error, status) in [
            (
                RunTodoError::Forbidden("no".to_owned()),
                StatusCode::FORBIDDEN,
            ),
            (
                RunTodoError::UnknownAgent("x".to_owned()),
                StatusCode::NOT_FOUND,
            ),
            (RunTodoError::Missing("x".to_owned()), StatusCode::NOT_FOUND),
            (RunTodoError::Busy("busy".to_owned()), StatusCode::CONFLICT),
            (
                RunTodoError::Invalid("bad".to_owned()),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let (tx, _seen) = spawn_session_reply(Err(error));
            let resp = router_with(tx)
                .oneshot(json_request(
                    "POST",
                    "/api/command",
                    &token,
                    r#"{"command":"status"}"#,
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), status);
        }

        let (tx, _seen) = spawn_session_reply(Err(RunTodoError::NotFound));
        let resp = router_with(tx)
            .oneshot(json_request(
                "POST",
                "/api/command",
                "wrong-token",
                r#"{"command":"status"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let serve_router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices::default(),
        );
        let resp = serve_router
            .oneshot(json_request(
                "POST",
                "/api/command",
                &token,
                r#"{"command":"status"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    // TC-541 / FR-541: CRUD routes and the snapshot reflect every mutation.
    #[tokio::test]
    async fn todo_routes_crud_and_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(TodoStore::open(&dir.path().join("conversations.sqlite3")).unwrap());
        let hub = test_hub();
        let token = "todo-token-123".to_string();
        let services = DashboardServices {
            todos: Some(store.clone()),
            commands: None,
            mcp: None,
            web_attached: false,
            events_path: None,
            memory: None,
            docs: None,
        };
        let router = build_router_with(hub.clone(), token.clone(), allowed_localhost(), services);

        // POST create -> 201 and the snapshot gains the to-do with a higher revision.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/todos",
                &token,
                r#"{"title":"buy milk","body":"2%"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created = read_body(resp).await;
        let created_json: serde_json::Value = serde_json::from_str(&created).unwrap();
        let id = created_json["id"].as_str().unwrap().to_owned();
        assert_eq!(created_json["status"], "open");
        assert_eq!(created_json["source"], "manual");

        let state = read_body(
            router
                .clone()
                .oneshot(json_request("GET", "/api/state", &token, ""))
                .await
                .unwrap(),
        )
        .await;
        let state_json: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!(state_json["todos"].as_array().unwrap().len(), 1);
        assert_eq!(state_json["todos"][0]["title"], "buy milk");
        let rev_after_create = revision_of(&state).unwrap();
        assert!(rev_after_create >= 1);

        // GET list -> 200.
        let resp = router
            .clone()
            .oneshot(json_request("GET", "/api/todos", &token, ""))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let list = read_body(resp).await;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&list)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );

        // PATCH title -> 200.
        let resp = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/todos/{id}"),
                &token,
                r#"{"title":"buy oat milk"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let patched = read_body(resp).await;
        assert!(patched.contains("buy oat milk"));

        // PATCH body -> 200.
        let resp = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/todos/{id}"),
                &token,
                r#"{"body":"shelf 3"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(read_body(resp).await.contains("shelf 3"));

        // PATCH status done -> 200 and snapshot revision increased.
        let resp = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/todos/{id}"),
                &token,
                r#"{"status":"done"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(read_body(resp).await.contains("\"done\""));

        let state = read_body(
            router
                .clone()
                .oneshot(json_request("GET", "/api/state", &token, ""))
                .await
                .unwrap(),
        )
        .await;
        assert!(revision_of(&state).unwrap() > rev_after_create);
        assert!(state.contains("buy oat milk"));

        // DELETE -> 204, then the list is empty.
        let resp = router
            .clone()
            .oneshot(json_request(
                "DELETE",
                &format!("/api/todos/{id}"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = router
            .oneshot(json_request("GET", "/api/todos", &token, ""))
            .await
            .unwrap();
        let list = read_body(resp).await;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&list)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert!(store.list().unwrap().is_empty());
    }

    // TC-544 / FR-541: invalid and unauthorized requests get the exact codes.
    #[tokio::test]
    async fn todo_routes_reject_invalid_requests() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(TodoStore::open(&dir.path().join("conversations.sqlite3")).unwrap());
        let token = "todo-token-123".to_string();
        let services = DashboardServices {
            todos: Some(store),
            commands: None,
            mcp: None,
            web_attached: false,
            events_path: None,
            memory: None,
            docs: None,
        };
        let router = build_router_with(test_hub(), token.clone(), allowed_localhost(), services);

        // empty title -> 400
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/todos",
                &token,
                r#"{"title":"   "}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // 201-character title -> 400
        let long_title = "x".repeat(201);
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/todos",
                &token,
                &format!(r#"{{"title":"{long_title}"}}"#),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // non-JSON content type -> 415
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/todos")
            .header("Host", "localhost:7420")
            .header("Origin", "http://localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "text/plain")
            .body(axum::body::Body::from(r#"{"title":"ok"}"#))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        // body over axum's default limit -> 413 request too large
        let oversized = "x".repeat(3 * 1024 * 1024);
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/todos")
            .header("Host", "localhost:7420")
            .header("Origin", "http://localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(format!(
                r#"{{"title":"{oversized}"}}"#
            )))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(read_body(resp).await.contains("request too large"));

        // unknown id -> 404
        let resp = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                "/api/todos/does-not-exist",
                &token,
                r#"{"title":"x"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // status "running" -> 400 (create one first for a valid id)
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/todos",
                &token,
                r#"{"title":"valid"}"#,
            ))
            .await
            .unwrap();
        let id = serde_json::from_str::<serde_json::Value>(&read_body(resp).await).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let resp = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/todos/{id}"),
                &token,
                r#"{"status":"running"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // missing token -> 401
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/todos")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // foreign Origin -> 403
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/todos")
            .header("Host", "localhost:7420")
            .header("Origin", "http://evil.example")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // TC-570 / FR-563: link fields validate on POST/PATCH (400), round-trip in
    // the API and snapshot, and PATCH null clears them.
    #[tokio::test]
    async fn todo_routes_validate_links() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(TodoStore::open(&dir.path().join("conversations.sqlite3")).unwrap());
        let token = "todo-token-123".to_string();
        let router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                events_path: None,
                todos: Some(store.clone()),
                commands: None,
                mcp: None,
                web_attached: false,
                memory: None,
                docs: None,
            },
        );

        // Valid key + link -> 201 with both fields echoed.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/todos",
                &token,
                r#"{"title":"linked","external_key":"PROJ-123","link_url":"https://example.invalid/browse/PROJ-123"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        let id = created["id"].as_str().unwrap().to_owned();
        assert_eq!(created["external_key"], "PROJ-123");
        assert_eq!(
            created["link_url"],
            "https://example.invalid/browse/PROJ-123"
        );

        // Key without a link or base URL -> key kept, link null.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/todos",
                &token,
                r#"{"title":"key only","external_key":"PROJ-9"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let key_only: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        assert_eq!(key_only["external_key"], "PROJ-9");
        assert!(key_only["link_url"].is_null());

        // Invalid links and keys -> 400.
        for body in [
            r#"{"title":"x","link_url":"javascript:alert(1)"}"#,
            r#"{"title":"x","link_url":"/relative/path"}"#,
            r#"{"title":"x","external_key":"123"}"#,
            r#"{"title":"x","external_key":"PROJ"}"#,
        ] {
            let resp = router
                .clone()
                .oneshot(json_request("POST", "/api/todos", &token, body))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "body: {body}");
        }

        // FR-563: import-only status/assignee cannot be user-set.
        for (body, expected) in [
            (
                r#"{"title":"x","external_status":"Done"}"#,
                "external_status is set by import",
            ),
            (
                r#"{"title":"x","external_assignee":"Demo User"}"#,
                "external_assignee is set by import",
            ),
        ] {
            let resp = router
                .clone()
                .oneshot(json_request("POST", "/api/todos", &token, body))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "body: {body}");
            assert_eq!(
                read_body(resp).await,
                format!("{{\"error\":\"{expected}\"}}")
            );
        }
        let overlong = format!(
            r#"{{"title":"x","link_url":"https://example.invalid/{}"}}"#,
            "a".repeat(2048)
        );
        let resp = router
            .clone()
            .oneshot(json_request("POST", "/api/todos", &token, &overlong))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // PATCH a browse URL onto a key-less to-do -> the key is extracted.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/todos",
                &token,
                r#"{"title":"unkeyed"}"#,
            ))
            .await
            .unwrap();
        let unkeyed_id = serde_json::from_str::<serde_json::Value>(&read_body(resp).await).unwrap()
            ["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let resp = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/todos/{unkeyed_id}"),
                &token,
                r#"{"link_url":"https://example.atlassian.net/browse/PROJ-456"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let patched: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        assert_eq!(patched["external_key"], "PROJ-456");
        assert_eq!(
            patched["link_url"],
            "https://example.atlassian.net/browse/PROJ-456"
        );

        // PATCH null clears a field.
        let resp = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/todos/{id}"),
                &token,
                r#"{"external_key":null,"link_url":null}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cleared: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        assert!(cleared["external_key"].is_null());
        assert!(cleared["link_url"].is_null());

        // The snapshot carries the fields too.
        let state = read_body(
            router
                .oneshot(json_request("GET", "/api/state", &token, ""))
                .await
                .unwrap(),
        )
        .await;
        assert!(state.contains("PROJ-9"));
    }

    // TC-545 / FR-542: running a to-do needs the --web command channel.
    #[tokio::test]
    async fn todo_run_requires_web_mode() {
        let token = "todo-token-123".to_string();

        // No command channel (`dmuxai serve`) -> 409 with the web-mode hint.
        let serve_router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices::default(),
        );
        let resp = serve_router
            .oneshot(json_request(
                "POST",
                "/api/todos/abc/run",
                &token,
                r#"{"mode":"graph"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert!(read_body(resp).await.contains("requires dmuxai --web"));

        // A channel whose receiver replies Busy -> 409.
        let busy_router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                todos: None,
                commands: Some(spawn_reply(Err(RunTodoError::Busy(
                    "agent busy".to_owned(),
                )))),
                mcp: None,
                web_attached: false,
                events_path: None,
                memory: None,
                docs: None,
            },
        );
        let resp = busy_router
            .oneshot(json_request(
                "POST",
                "/api/todos/abc/run",
                &token,
                r#"{"mode":"agent","agent":"codex"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);

        // A successful reply -> 202 with the running to-do.
        let ok_router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                todos: None,
                commands: Some(spawn_reply(Ok(test_todo()))),
                mcp: None,
                web_attached: false,
                events_path: None,
                memory: None,
                docs: None,
            },
        );
        let resp = ok_router
            .oneshot(json_request(
                "POST",
                "/api/todos/abc/run",
                &token,
                r#"{"mode":"graph"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert!(read_body(resp).await.contains("\"running\""));
    }

    // TC-545 / FR-542: a `--web` server whose store failed to open answers 503
    // "to-do board unavailable" instead of the `serve` 409 hint.
    #[tokio::test]
    async fn todo_run_web_without_store_is_503() {
        let token = "todo-token-123".to_string();
        let router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                todos: None,
                commands: None,
                mcp: None,
                web_attached: true,
                events_path: None,
                memory: None,
                docs: None,
            },
        );
        let resp = router
            .oneshot(json_request(
                "POST",
                "/api/todos/abc/run",
                &token,
                r#"{"mode":"graph"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(read_body(resp).await.contains("to-do board unavailable"));
    }

    // TC-545 / FR-542: the agent id is trimmed before it is forwarded.
    #[tokio::test]
    async fn todo_run_trims_agent_id() {
        let token = "todo-token-123".to_string();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                todos: None,
                commands: Some(spawn_recording_reply(Ok(test_todo()), seen.clone())),
                mcp: None,
                web_attached: false,
                events_path: None,
                memory: None,
                docs: None,
            },
        );
        let resp = router
            .oneshot(json_request(
                "POST",
                "/api/todos/abc/run",
                &token,
                r#"{"mode":"agent","agent":"  codex  "}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let modes = seen.lock().unwrap();
        assert_eq!(modes.len(), 1);
        match &modes[0] {
            RunMode::Agent { agent } => assert_eq!(agent, "codex"),
            other => panic!("expected agent mode, got {other:?}"),
        }
    }

    // TC-766 / FR-765: prompt route contract
    #[tokio::test]
    async fn prompt_route_contract() {
        let token = "test-token-123".to_string();
        let accepted = PromptAccepted {
            conversation_id: "conv-1".to_string(),
            agent: "codex".to_string(),
        };
        let router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                mcp: None,
                todos: None,
                commands: Some(spawn_prompt_reply(Ok(accepted))),
                web_attached: false,
                events_path: None,
                memory: None,
                docs: None,
            },
        );

        // 1. 202 with a spawned reply double
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/prompt",
                &token,
                r#"{"text":"hello world","agent":"codex","mode":"ask"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let body = read_body(resp).await;
        let val: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(val["accepted"], true);
        assert_eq!(val["conversation_id"], "conv-1");
        assert_eq!(val["agent"], "codex");

        // 2. 400 empty text
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/prompt",
                &token,
                r#"{"text":"   "}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(read_body(resp).await.contains("text is required"));

        // 3. 400 bad mode
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/prompt",
                &token,
                r#"{"text":"hello","mode":"badmode"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(read_body(resp).await.contains("invalid mode"));

        // 4. 409 without sender
        let serve_router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices::default(),
        );
        let resp = serve_router
            .oneshot(json_request(
                "POST",
                "/api/prompt",
                &token,
                r#"{"text":"hello"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert!(read_body(resp).await.contains("requires dmuxai --web"));

        // 5. 401 without token
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/prompt")
            .header("Host", "localhost:7420")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(r#"{"text":"hello"}"#))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 6. 403 foreign Origin
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/prompt")
            .header("Host", "localhost:7420")
            .header("Origin", "http://evil.example")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(r#"{"text":"hello"}"#))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // TC-767 / FR-766: REST catch-up and SSE `state-change` frames tail the log file.
    #[tokio::test]
    async fn events_log_and_sse_state_change() {
        use crate::events::{EventDraft, EventLog, events_path, read_since};

        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::open(dir.path(), dir.path().display().to_string());
        let events_file = events_path(dir.path());
        let _ = log.append(vec![
            EventDraft {
                kind: "agent".to_owned(),
                task: Some("task".to_owned()),
                todo: None,
                node: None,
                runner: Some("codex".to_owned()),
                model: None,
                from_state: Some("idle".to_owned()),
                to_state: "pending".to_owned(),
                message: None,
            },
            EventDraft {
                kind: "agent".to_owned(),
                task: Some("task".to_owned()),
                todo: None,
                node: None,
                runner: Some("codex".to_owned()),
                model: None,
                from_state: Some("pending".to_owned()),
                to_state: "started".to_owned(),
                message: None,
            },
            EventDraft {
                kind: "agent".to_owned(),
                task: Some("task".to_owned()),
                todo: None,
                node: None,
                runner: Some("codex".to_owned()),
                model: None,
                from_state: Some("started".to_owned()),
                to_state: "running".to_owned(),
                message: Some("responding".to_owned()),
            },
        ]);

        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let token = "events-log-token-12345".to_string();
        let services = DashboardServices {
            mcp: None,
            todos: None,
            commands: None,
            web_attached: false,
            events_path: Some(events_file.clone()),
            memory: None,
            docs: None,
        };
        let router = build_router_with(hub.clone(), token.clone(), allowed_localhost(), services);

        let resp = router
            .clone()
            .oneshot(json_request("GET", "/api/events/log?since=1", &token, ""))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        let seqs = json["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["seq"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(seqs, vec![2, 3]);
        assert_eq!(json["last_seq"].as_u64(), Some(3));

        let resp = router
            .clone()
            .oneshot(json_request(
                "GET",
                "/api/events/log?since=0&limit=1",
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["events"].as_array().unwrap().len(), 1);
        assert_eq!(json["last_seq"].as_u64(), Some(1));

        let resp = router
            .clone()
            .oneshot(json_request("GET", "/api/events/log?since=abc", &token, ""))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let req = axum::http::Request::builder()
            .uri("/api/events/log?since=0")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let config = ServerConfig {
            bind: "127.0.0.1".parse().unwrap(),
            port: 0,
            allow_remote: false,
            token: token.clone(),
            services: DashboardServices {
                mcp: None,
                todos: None,
                commands: None,
                web_attached: false,
                events_path: Some(events_file.clone()),
                memory: None,
                docs: None,
            },
        };
        let server = start_server(config, hub).await.unwrap();
        let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
        let req = format!(
            "GET /api/events?since=1&token={token} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\n\r\n",
            server.addr
        );
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut buf = Vec::new();
        let mut temp_buf = [0u8; 4096];
        let read_sse = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let n = stream.read(&mut temp_buf).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&temp_buf[..n]);
                let text = String::from_utf8_lossy(&buf);
                let seqs = parse_state_change_seqs(&text);
                if seqs.contains(&2) && seqs.contains(&3) {
                    break;
                }
            }
        })
        .await;
        assert!(
            read_sse.is_ok(),
            "timed out waiting for replayed state-change frames"
        );

        let _ = log.append(vec![EventDraft {
            kind: "agent".to_owned(),
            task: Some("task".to_owned()),
            todo: None,
            node: None,
            runner: Some("codex".to_owned()),
            model: None,
            from_state: Some("running".to_owned()),
            to_state: "completed".to_owned(),
            message: None,
        }]);

        let appended = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let n = stream.read(&mut temp_buf).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&temp_buf[..n]);
                if parse_state_change_seqs(&String::from_utf8_lossy(&buf)).contains(&4) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(appended.is_ok(), "timed out waiting for appended seq 4");
        assert_eq!(read_since(&events_file, 0, None).len(), 4);

        server.abort();
    }

    fn parse_state_change_seqs(buf: &str) -> Vec<u64> {
        let mut seqs = Vec::new();
        let lines: Vec<&str> = buf.lines().collect();
        let mut index = 0;
        while index < lines.len() {
            if lines[index] == "event: state-change" {
                index += 1;
                if index < lines.len()
                    && let Some(id) = lines[index].strip_prefix("id: ")
                    && let Ok(seq) = id.trim().parse::<u64>()
                {
                    seqs.push(seq);
                }
            }
            index += 1;
        }
        seqs
    }

    // TC-728 (FR-727): memory API routes and auth.
    #[tokio::test]
    async fn memory_routes_api() {
        use crate::memory::{MemoryStore, ProposalInput};

        let vault = tempfile::tempdir().unwrap();
        let vault_path = vault.path().to_path_buf();
        let memory = MemoryHandle {
            store: MemoryStore::new(vault_path.clone()),
            project: "demo".to_owned(),
            dir: "cursor-workflow/memory".to_owned(),
            inject: true,
        };
        let token = "memory-token-728".to_owned();
        let hub = test_hub();
        let services = DashboardServices {
            todos: None,
            commands: None,
            web_attached: false,
            mcp: None,
            memory: Some(memory.clone()),
            events_path: None,
            docs: None,
        };
        let router = build_router_with(hub, token.clone(), allowed_localhost(), services);

        let no_auth = axum::http::Request::builder()
            .method("GET")
            .uri("/api/memory")
            .header("Host", "localhost:7420")
            .header("Origin", "http://localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(no_auth).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let get_resp = router
            .clone()
            .oneshot(json_request("GET", "/api/memory", &token, ""))
            .await
            .unwrap();
        assert_eq!(get_resp.status(), StatusCode::OK);
        let get_body = read_body(get_resp).await;
        let get_json: serde_json::Value = serde_json::from_str(&get_body).unwrap();
        assert_eq!(get_json["enabled"], true);
        assert_eq!(get_json["project"], "demo");
        assert_eq!(get_json["dir"], "cursor-workflow/memory");
        assert!(get_json["context"].is_string());
        assert!(get_json["pending"].is_array());
        assert!(
            !get_body.contains(vault_path.to_str().unwrap()),
            "response must not leak vault path"
        );

        let save_resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/memory/save",
                &token,
                r#"{"text":"dashboard note line"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(save_resp.status(), StatusCode::OK);
        let save_json: serde_json::Value =
            serde_json::from_str(&read_body(save_resp).await).unwrap();
        assert_eq!(save_json["saved"], true);
        assert_eq!(save_json["target"], "projects/demo.md");

        let after_save = router
            .clone()
            .oneshot(json_request("GET", "/api/memory", &token, ""))
            .await
            .unwrap();
        let after_body = read_body(after_save).await;
        assert!(after_body.contains("dashboard note line"));
        assert!(!after_body.contains(vault_path.to_str().unwrap()));

        let secret_resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/memory/save",
                &token,
                r#"{"text":"my password is hunter2"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(secret_resp.status(), StatusCode::BAD_REQUEST);

        let proposal_id = memory
            .store
            .propose(
                "demo",
                "web-test",
                &[ProposalInput {
                    target: "projects/demo.md".to_owned(),
                    text: "proposal from test".to_owned(),
                }],
            )
            .unwrap()
            .added
            .first()
            .map(|p| p.id.clone())
            .expect("proposal added");

        let approve_resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/memory/proposals/{proposal_id}/approve"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(approve_resp.status(), StatusCode::OK);
        let approve_json: serde_json::Value =
            serde_json::from_str(&read_body(approve_resp).await).unwrap();
        assert_eq!(approve_json["ok"], true);

        let approve_again = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/memory/proposals/{proposal_id}/approve"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(approve_again.status(), StatusCode::CONFLICT);

        let reject_id = memory
            .store
            .propose(
                "demo",
                "reject-me",
                &[ProposalInput {
                    target: "human.md".to_owned(),
                    text: "reject this one".to_owned(),
                }],
            )
            .unwrap()
            .added[0]
            .id
            .clone();

        let reject_resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/memory/proposals/{reject_id}/reject"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(reject_resp.status(), StatusCode::OK);

        let unknown = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/memory/proposals/does-not-exist/approve",
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

        let disabled_router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices::default(),
        );
        let disabled_get = disabled_router
            .clone()
            .oneshot(json_request("GET", "/api/memory", &token, ""))
            .await
            .unwrap();
        let disabled_json: serde_json::Value =
            serde_json::from_str(&read_body(disabled_get).await).unwrap();
        assert_eq!(disabled_json["enabled"], false);

        let disabled_post = disabled_router
            .oneshot(json_request(
                "POST",
                "/api/memory/save",
                &token,
                r#"{"text":"nope"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(disabled_post.status(), StatusCode::CONFLICT);
        assert!(
            read_body(disabled_post)
                .await
                .contains("memory is not configured")
        );
    }

    // ---- slice C test helpers -------------------------------------------

    fn mcp_stdio(name: &str, kind: ServerKind, env: &[(&str, &str)]) -> ResolvedMcpServer {
        ResolvedMcpServer {
            name: name.to_owned(),
            kind,
            transport: Transport::Stdio {
                command: "npx".to_owned(),
                args: vec!["-y".to_owned(), "server".to_owned()],
                env: env
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect(),
            },
            runners: None,
            tools: ToolNames::default(),
        }
    }

    fn mcp_http(
        name: &str,
        kind: ServerKind,
        url: &str,
        headers: &[(&str, &str)],
    ) -> ResolvedMcpServer {
        ResolvedMcpServer {
            name: name.to_owned(),
            kind,
            transport: Transport::Http {
                url: url.to_owned(),
                headers: headers
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect(),
            },
            runners: None,
            tools: ToolNames::default(),
        }
    }

    // TC-567 / FR-562: the server listing redacts env values, header values and
    // URL query strings; a missing registry is 503.
    #[tokio::test]
    async fn mcp_servers_route_redacts_secrets() {
        let token = "mcp-token-123".to_string();
        let registry = McpRegistry::new(
            vec![
                mcp_stdio(
                    "everything",
                    ServerKind::Generic,
                    &[("API_TOKEN", "demo-env-secret")],
                ),
                mcp_http(
                    "jira",
                    ServerKind::Jira,
                    "http://127.0.0.1:9/mcp?key=demo-query-secret",
                    &[("Authorization", "demo-header-secret")],
                ),
            ],
            vec!["mcp server gone disabled: env var MISSING is not set".to_owned()],
        );
        let router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                events_path: None,
                todos: None,
                commands: None,
                mcp: Some(Arc::new(registry)),
                web_attached: false,
                memory: None,
                docs: None,
            },
        );

        let resp = router
            .clone()
            .oneshot(json_request("GET", "/api/mcp/servers", &token, ""))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert!(body.contains("\"name\":\"everything\""));
        assert!(body.contains("\"name\":\"jira\""));
        assert!(body.contains("\"transport\":\"stdio\""));
        assert!(body.contains("\"transport\":\"http\""));
        assert!(body.contains("\"kind\":\"jira\""));
        assert!(!body.contains("demo-env-secret"));
        assert!(!body.contains("demo-header-secret"));
        assert!(!body.contains("demo-query-secret"));
        assert!(!body.contains("key="));
        assert!(!body.contains("example.invalid"));

        // No registry at all -> 503.
        let bare = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices::default(),
        );
        let resp = bare
            .oneshot(json_request("GET", "/api/mcp/servers", &token, ""))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // TC-562 / FR-561, NFR-560: import from the in-repo mock over streamable
    // HTTP for a Jira search, a Confluence page id, and the failure paths.
    #[tokio::test]
    async fn import_creates_todos_from_mock_jira_and_confluence() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = tokio::spawn(crate::mcp::mock::serve_http(listener));

        let url = format!("http://{addr}/mcp");
        let jira = ResolvedMcpServer {
            name: "jira".to_owned(),
            kind: ServerKind::Jira,
            transport: Transport::Http {
                url: url.clone(),
                headers: std::collections::BTreeMap::new(),
            },
            runners: None,
            tools: ToolNames {
                search: "jira_search".to_owned(),
                get: "jira_get".to_owned(),
            },
        };
        let confluence = ResolvedMcpServer {
            name: "confluence".to_owned(),
            kind: ServerKind::Confluence,
            transport: Transport::Http {
                url,
                headers: std::collections::BTreeMap::new(),
            },
            runners: None,
            tools: ToolNames {
                search: "confluence_search".to_owned(),
                get: "confluence_get".to_owned(),
            },
        };
        let generic = mcp_http(
            "generic",
            ServerKind::Generic,
            "http://127.0.0.1:9/mcp",
            &[],
        );
        let registry = Arc::new(McpRegistry::new(
            vec![jira, confluence, generic],
            Vec::new(),
        ));

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(TodoStore::open(&dir.path().join("conversations.sqlite3")).unwrap());
        let hub = test_hub();
        let token = "mcp-token-123".to_string();
        let router = build_router_with(
            hub,
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                events_path: None,
                todos: Some(store.clone()),
                commands: None,
                mcp: Some(registry.clone()),
                web_attached: false,
                memory: None,
                docs: None,
            },
        );

        // Jira search -> two new to-dos with source_ref set.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/import",
                &token,
                r#"{"server":"jira","query":"DEMO"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let value: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        assert_eq!(value["imported"], 2);
        assert_eq!(value["updated"], 0);
        let todos = value["todos"].as_array().unwrap();
        assert_eq!(todos.len(), 2);
        assert!(todos.iter().all(|todo| todo["source"] == "jira"));
        assert!(todos.iter().any(|todo| todo["source_ref"] == "DEMO-1"));
        assert!(todos.iter().any(|todo| todo["source_ref"] == "DEMO-2"));
        // TC-571 / FR-563/FR-564: the item key/url land on the to-do.
        let demo1 = todos
            .iter()
            .find(|todo| todo["source_ref"] == "DEMO-1")
            .unwrap();
        assert_eq!(demo1["external_key"], "DEMO-1");
        assert_eq!(demo1["link_url"], "https://example.invalid/jira/DEMO-1");

        // Re-import -> updated, never duplicated.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/import",
                &token,
                r#"{"server":"jira","query":"DEMO"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let value: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        assert_eq!(value["imported"], 0);
        assert_eq!(value["updated"], 2);
        assert_eq!(store.list().unwrap().len(), 2);

        // A numeric Confluence page id selects tools.get.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/import",
                &token,
                r#"{"server":"confluence","query":"1001"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let value: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        assert_eq!(value["imported"], 1);
        assert_eq!(value["todos"][0]["source"], "confluence");
        assert_eq!(value["todos"][0]["source_ref"], "1001");

        // Imported to-dos reach the hub.
        let state = read_body(
            router
                .clone()
                .oneshot(json_request("GET", "/api/state", &token, ""))
                .await
                .unwrap(),
        )
        .await;
        assert!(state.contains("DEMO-1"));
        assert!(state.contains("confluence"));

        // Unknown server -> 404.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/import",
                &token,
                r#"{"server":"nope","query":"x"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Non-importable kind -> 400.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/import",
                &token,
                r#"{"server":"generic","query":"x"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Tool error -> 502 naming the server exactly once (finding 6).
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/import",
                &token,
                r#"{"server":"jira","query":"NOPE-9"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = read_body(resp).await;
        assert_eq!(
            body.matches("jira").count(),
            1,
            "server name must appear once: {body}"
        );
        assert!(body.contains("unknown id: NOPE-9"));

        // Non-JSON content type -> 415.
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/import")
            .header("Host", "localhost:7420")
            .header("Origin", "http://localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "text/plain")
            .body(axum::body::Body::from(
                r#"{"server":"jira","query":"DEMO"}"#,
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        // No store -> 503.
        let no_store = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                events_path: None,
                todos: None,
                commands: None,
                mcp: Some(registry),
                web_attached: false,
                memory: None,
                docs: None,
            },
        );
        let resp = no_store
            .oneshot(json_request(
                "POST",
                "/api/import",
                &token,
                r#"{"server":"jira","query":"DEMO"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        mock.abort();
    }

    // TC-573 / FR-563, UX-561: import stores the mock Jira status/assignee,
    // re-import refreshes them, user PATCH is rejected, and the snapshot carries
    // the configured base URL for link previews.
    #[tokio::test]
    async fn import_updates_external_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = tokio::spawn(crate::mcp::mock::serve_http(listener));

        let jira = ResolvedMcpServer {
            name: "jira".to_owned(),
            kind: ServerKind::Jira,
            transport: Transport::Http {
                url: format!("http://{addr}/mcp"),
                headers: std::collections::BTreeMap::new(),
            },
            runners: None,
            tools: ToolNames {
                search: "jira_search".to_owned(),
                get: "jira_get".to_owned(),
            },
        };
        let registry = Arc::new(McpRegistry::new(vec![jira], Vec::new()));

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(TodoStore::open(&dir.path().join("conversations.sqlite3")).unwrap());
        let base = "https://example.atlassian.net";
        store.set_jira_base_url(Some(base.to_owned()));
        let token = "mcp-token-123".to_string();
        let router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                events_path: None,
                todos: Some(store.clone()),
                commands: None,
                mcp: Some(registry),
                web_attached: false,
                memory: None,
                docs: None,
            },
        );

        // Import -> status/assignee stored from the fixtures.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/import",
                &token,
                r#"{"server":"jira","query":"DEMO"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let demo1_id = {
            let todos = store.list().unwrap();
            let demo1 = todos
                .iter()
                .find(|t| t.source_ref.as_deref() == Some("DEMO-1"))
                .unwrap();
            assert_eq!(demo1.external_status.as_deref(), Some("In Progress"));
            assert_eq!(demo1.external_assignee.as_deref(), Some("Demo User"));
            let demo2 = todos
                .iter()
                .find(|t| t.source_ref.as_deref() == Some("DEMO-2"))
                .unwrap();
            assert_eq!(demo2.external_status.as_deref(), Some("To Do"));
            assert!(demo2.external_assignee.is_none());
            demo1.id.clone()
        };

        // The snapshot previews links from the configured base URL.
        let state = read_body(
            router
                .clone()
                .oneshot(json_request("GET", "/api/state", &token, ""))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            state.contains(&format!("\"jira_base_url\":\"{base}\"")),
            "snapshot must carry the base URL: {state}"
        );
        assert!(state.contains("\"external_status\":\"In Progress\""));

        // Re-import with the changed fixture -> same rows, updated status.
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/import",
                &token,
                &format!(
                    r#"{{"server":"jira","query":"{}"}}"#,
                    crate::mcp::mock::JIRA_STATUS_KEYWORD
                ),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let value: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        assert_eq!(value["imported"], 0);
        assert_eq!(value["updated"], 2);
        let todos = store.list().unwrap();
        assert_eq!(todos.len(), 2, "re-import never duplicates");
        let demo1 = todos.iter().find(|t| t.id == demo1_id).unwrap();
        assert_eq!(demo1.external_status.as_deref(), Some("Done"));
        assert_eq!(demo1.external_assignee.as_deref(), Some("Demo Reviewer"));

        // A user PATCH of `external_status` is rejected with the fixed 400.
        let resp = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/todos/{demo1_id}"),
                &token,
                r#"{"external_status":"Open"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            read_body(resp).await,
            r#"{"error":"external_status is set by import"}"#
        );

        mock.abort();
    }

    // TC-873 (FR-872, FR-873, NFR-870)
    #[tokio::test]
    // The env lock must span the whole test (HOME/state dir are process-wide);
    // a current-thread test runtime never contends on it across awaits.
    #[allow(clippy::await_holding_lock)]
    async fn workspace_api_lists_dirs_and_validates() {
        let _env_guard = crate::state_path::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let state_dir = root.join("state");
        let outside = root.join("outside");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let prev_home = std::env::var_os("HOME");
        let prev_state = std::env::var_os("DMUXAI_STATE_DIR");
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("DMUXAI_STATE_DIR", &state_dir);
        }

        // Populate home with subdirs, a file, a dotdir, a denied dir (.ssh), and a symlink into .ssh
        let proj_a = home.join("alpha");
        let proj_b = home.join("beta");
        let dot_dir = home.join(".secret-dir");
        let ssh_dir = home.join(".ssh");
        std::fs::create_dir_all(&proj_a).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();
        std::fs::create_dir_all(&dot_dir).unwrap();
        std::fs::create_dir_all(&ssh_dir).unwrap();
        std::fs::write(home.join("README.txt"), "secret file contents").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&ssh_dir, home.join("ssh-link")).unwrap();
            std::os::unix::fs::symlink(&outside, home.join("outside-link")).unwrap();
        }

        let _ = crate::workspace::record_recent_workspace(&proj_a);

        let token = "secret-ws-token-873".to_owned();
        let hub = test_hub();
        let (cmd_tx, _cmd_rx) = mpsc::channel(8);
        let router = build_router_with(
            hub.clone(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                web_attached: true,
                commands: Some(cmd_tx),
                ..DashboardServices::default()
            },
        );

        // Auth required on all workspace routes
        for (method, path) in [
            ("GET", "/api/workspace"),
            ("GET", "/api/workspace/dirs"),
            ("POST", "/api/workspace/validate"),
            ("POST", "/api/workspace/switch"),
        ] {
            let resp = router
                .clone()
                .oneshot(json_request(method, path, "wrong-token", ""))
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} must require auth"
            );
        }

        // GET /api/workspace returns current, recent, browse_roots, can_switch
        let resp = router
            .clone()
            .oneshot(json_request("GET", "/api/workspace", &token, ""))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert!(!body.contains(&token), "token must never appear in body");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["can_switch"], cfg!(unix));
        assert!(
            v["recent"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x.as_str() == Some(&proj_a.to_string_lossy()))
        );

        // GET /api/workspace/dirs lists child directories only, hides dotdirs and symlinks to denied/outside
        let encoded_home = home.to_string_lossy().into_owned();
        let resp = router
            .clone()
            .oneshot(json_request(
                "GET",
                &format!("/api/workspace/dirs?path={encoded_home}"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert!(!body.contains(&token));
        assert!(!body.contains("README.txt"));
        assert!(!body.contains("secret file contents"));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let dirs: Vec<&str> = v["dirs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(dirs, vec!["alpha", "beta"]);

        // With hidden=1, .secret-dir appears, but .ssh (denied) and ssh-link are still excluded
        let resp = router
            .clone()
            .oneshot(json_request(
                "GET",
                &format!("/api/workspace/dirs?path={encoded_home}&hidden=1"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        let dirs: Vec<&str> = v["dirs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert!(dirs.contains(&".secret-dir"));
        assert!(!dirs.contains(&".ssh"));
        assert!(!dirs.contains(&"ssh-link"));

        // Cap at 500 directories
        let many_dir = home.join("many");
        std::fs::create_dir_all(&many_dir).unwrap();
        for i in 0..510 {
            std::fs::create_dir_all(many_dir.join(format!("d{i:04}"))).unwrap();
        }
        let encoded_many = many_dir.to_string_lossy().into_owned();
        let resp = router
            .clone()
            .oneshot(json_request(
                "GET",
                &format!("/api/workspace/dirs?path={encoded_many}"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        assert_eq!(v["dirs"].as_array().unwrap().len(), 500);

        // Denied path and outside browse roots are refused
        let encoded_ssh = ssh_dir.to_string_lossy().into_owned();
        let resp = router
            .clone()
            .oneshot(json_request(
                "GET",
                &format!("/api/workspace/dirs?path={encoded_ssh}"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let err_body = read_body(resp).await;
        assert!(err_body.contains("denied: credential directory"));

        let encoded_outside = outside.to_string_lossy().into_owned();
        let resp = router
            .clone()
            .oneshot(json_request(
                "GET",
                &format!("/api/workspace/dirs?path={encoded_outside}"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(read_body(resp).await.contains("outside browse roots"));

        // POST /api/workspace/validate works for valid, outside-roots valid, and invalid paths
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/workspace/validate",
                &token,
                &serde_json::json!({ "path": outside.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&read_body(resp).await).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["path"], outside.to_string_lossy().as_ref());

        for (bad_path, expected_reason) in [
            (
                home.join("README.txt").to_string_lossy().into_owned(),
                "not a directory",
            ),
            (
                home.join("no-such-dir").to_string_lossy().into_owned(),
                "does not exist",
            ),
            (
                ssh_dir.to_string_lossy().into_owned(),
                "denied: credential directory",
            ),
            (
                state_dir.to_string_lossy().into_owned(),
                "denied: DmuxAI state directory",
            ),
            ("/".to_owned(), "denied: filesystem root"),
        ] {
            let resp = router
                .clone()
                .oneshot(json_request(
                    "POST",
                    "/api/workspace/validate",
                    &token,
                    &serde_json::json!({ "path": bad_path }).to_string(),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = read_body(resp).await;
            assert!(!body.contains(&token));
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["ok"], false);
            assert_eq!(v["reason"], expected_reason);
        }

        // View-only router refuses dirs and switch, but allows validate
        let view_only_router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                web_attached: false,
                ..DashboardServices::default()
            },
        );
        let resp = view_only_router
            .clone()
            .oneshot(json_request(
                "GET",
                &format!("/api/workspace/dirs?path={encoded_home}"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let resp = view_only_router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/workspace/switch",
                &token,
                &serde_json::json!({ "path": proj_a.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let resp = view_only_router
            .oneshot(json_request(
                "POST",
                "/api/workspace/validate",
                &token,
                &serde_json::json!({ "path": proj_a.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        match prev_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match prev_state {
            Some(v) => unsafe { std::env::set_var("DMUXAI_STATE_DIR", v) },
            None => unsafe { std::env::remove_var("DMUXAI_STATE_DIR") },
        }
    }

    // TC-875 (FR-874): if the App never answers, the handler cancels the
    // switch so switch-pending cannot stay set until restart.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn workspace_switch_timeout_sends_cancel() {
        let _env_guard = crate::state_path::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let target = std::fs::canonicalize(tmp.path()).unwrap();
        let (cmd_tx, mut cmd_rx) = mpsc::channel(8);
        let hub = test_hub();
        hub.set_switch_config(WorkspaceSwitchConfig {
            unix_supported: Some(true),
            ..WorkspaceSwitchConfig::default()
        });
        let token = "switch-timeout-token".to_owned();
        let router = build_router_with(
            hub,
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                web_attached: true,
                commands: Some(cmd_tx),
                ..DashboardServices::default()
            },
        );
        let resp = router
            .oneshot(json_request(
                "POST",
                "/api/workspace/switch",
                &token,
                &serde_json::json!({ "path": target.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
        // First the switch request (its reply is held, never answered)...
        let first = cmd_rx.recv().await.expect("switch command");
        let _held_reply = match first {
            DashboardCommand::SwitchWorkspace { reply, .. } => reply,
            _ => panic!("expected SwitchWorkspace first"),
        };
        // ...then the cancel that clears switch-pending.
        assert!(matches!(
            cmd_rx.recv().await,
            Some(DashboardCommand::CancelSwitchWorkspace { .. })
        ));
    }

    // TC-875 (FR-874)
    #[tokio::test]
    // The env lock must span the whole test (HOME/state dir are process-wide);
    // a current-thread test runtime never contends on it across awaits.
    #[allow(clippy::await_holding_lock)]
    async fn workspace_switch_guards() {
        let _env_guard = crate::state_path::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let state_dir = root.join("state");
        let target_ws = home.join("target-ws");
        std::fs::create_dir_all(&target_ws).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();

        let prev_home = std::env::var_os("HOME");
        let prev_state = std::env::var_os("DMUXAI_STATE_DIR");
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("DMUXAI_STATE_DIR", &state_dir);
        }

        let token = "switch-token-875".to_owned();

        // 1. View-only returns 403
        let view_router = build_router_with(
            test_hub(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                web_attached: false,
                ..DashboardServices::default()
            },
        );
        let resp = view_router
            .oneshot(json_request(
                "POST",
                "/api/workspace/switch",
                &token,
                &serde_json::json!({ "path": target_ws.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // 2. Non-Unix (simulated via unix_supported: Some(false)) returns 501
        let (tx_non_unix, _rx_non_unix) = mpsc::channel(8);
        let non_unix_hub = test_hub();
        non_unix_hub.set_switch_config(WorkspaceSwitchConfig {
            unix_supported: Some(false),
            ..WorkspaceSwitchConfig::default()
        });
        let non_unix_router = build_router_with(
            non_unix_hub,
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                web_attached: true,
                commands: Some(tx_non_unix),
                ..DashboardServices::default()
            },
        );
        let resp = non_unix_router
            .oneshot(json_request(
                "POST",
                "/api/workspace/switch",
                &token,
                &serde_json::json!({ "path": target_ws.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);

        // 3. Real App command handler loop to test busy 409, atomic switch_pending, preflight failure, and exec double
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<DashboardCommand>(16);
        let hub = test_hub();
        let hub_for_loop = hub.clone();

        let busy_flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let switch_pending_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let busy_for_loop = busy_flag.clone();
        let pending_for_loop = switch_pending_flag.clone();

        let loop_handle = tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    DashboardCommand::SwitchWorkspace { reply, .. } => {
                        if pending_for_loop.load(std::sync::atomic::Ordering::SeqCst) {
                            let _ = reply.send(Err(RunTodoError::Busy(
                                "workspace switch in progress".to_owned(),
                            )));
                        } else if busy_for_loop.load(std::sync::atomic::Ordering::SeqCst) {
                            let _ = reply.send(Err(RunTodoError::Busy(
                                "cannot switch workspace while a turn or graph run is in flight"
                                    .to_owned(),
                            )));
                        } else {
                            pending_for_loop.store(true, std::sync::atomic::Ordering::SeqCst);
                            let _ = reply.send(Ok(()));
                        }
                    }
                    DashboardCommand::CancelSwitchWorkspace { notice } => {
                        pending_for_loop.store(false, std::sync::atomic::Ordering::SeqCst);
                        hub_for_loop.push_notice(notice);
                    }
                    DashboardCommand::SubmitPrompt { reply, .. } => {
                        if pending_for_loop.load(std::sync::atomic::Ordering::SeqCst) {
                            let _ = reply.send(Err(RunTodoError::Busy(
                                "workspace switch in progress".to_owned(),
                            )));
                        } else {
                            let _ = reply.send(Ok(crate::web::commands::PromptAccepted {
                                agent: "codex".to_owned(),
                                conversation_id: "c1".to_owned(),
                            }));
                        }
                    }
                    _ => {}
                }
            }
        });

        let captured_argv: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let argv_clone = captured_argv.clone();
        let exec_double: ExecFn = Arc::new(move |_exe, args, _tok| {
            *argv_clone.lock().unwrap() = args.to_vec();
            Ok(())
        });

        hub.set_switch_config(WorkspaceSwitchConfig {
            orig_args: vec![
                "--workspace-dir=/old/ws".to_owned(),
                "--web".to_owned(),
                "--port".to_owned(),
                "0".to_owned(),
            ],
            bound_port: Some(7432),
            current_exe: Some(std::env::current_exe().unwrap()),
            exec_hook: Some(exec_double.clone()),
            on_exec_error: None,
            unix_supported: Some(true),
        });
        let router = build_router_with(
            hub.clone(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                web_attached: true,
                commands: Some(cmd_tx.clone()),
                ..DashboardServices::default()
            },
        );

        // Busy -> 409
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/workspace/switch",
                &token,
                &serde_json::json!({ "path": target_ws.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert!(!switch_pending_flag.load(std::sync::atomic::Ordering::SeqCst));

        // Idle -> 202, sets switch_pending atomically, invokes exec double with single -C
        busy_flag.store(false, std::sync::atomic::Ordering::SeqCst);
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/workspace/switch",
                &token,
                &serde_json::json!({ "path": target_ws.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert!(switch_pending_flag.load(std::sync::atomic::Ordering::SeqCst));
        let argv = captured_argv.lock().unwrap().clone();
        assert_eq!(
            argv.iter().filter(|a| a.as_str() == "-C").count(),
            1,
            "re-exec argv must have exactly one -C: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.starts_with("--workspace-dir")),
            "old --workspace-dir= must be stripped: {argv:?}"
        );
        assert_eq!(
            argv,
            vec![
                "-C".to_owned(),
                target_ws.to_string_lossy().into_owned(),
                "--web".to_owned(),
                "--port".to_owned(),
                "7432".to_owned(),
            ]
        );

        // After 202, a new prompt gets 409 "workspace switch in progress"
        let resp = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/prompt",
                &token,
                r#"{"text":"hello"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert!(
            read_body(resp)
                .await
                .contains("workspace switch in progress")
        );

        // 4. Failing preflight clears switch_pending and publishes a dashboard notice
        switch_pending_flag.store(false, std::sync::atomic::Ordering::SeqCst);
        let bad_exe_hub = test_hub();
        bad_exe_hub.set_switch_config(WorkspaceSwitchConfig {
            orig_args: vec!["--web".to_owned()],
            bound_port: Some(7432),
            current_exe: Some(root.join("missing-binary")),
            exec_hook: Some(exec_double),
            on_exec_error: None,
            unix_supported: Some(true),
        });
        let bad_exe_router = build_router_with(
            bad_exe_hub.clone(),
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                web_attached: true,
                commands: Some(cmd_tx.clone()),
                ..DashboardServices::default()
            },
        );
        let resp = bad_exe_router
            .oneshot(json_request(
                "POST",
                "/api/workspace/switch",
                &token,
                &serde_json::json!({ "path": target_ws.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            !switch_pending_flag.load(std::sync::atomic::Ordering::SeqCst),
            "failing preflight must clear switch_pending"
        );
        assert!(
            bad_exe_hub.snapshot().session.as_ref().is_some_and(|s| s
                .transcript
                .iter()
                .any(|m| m.content.contains("Workspace switch failed"))),
            "failing preflight must raise a dashboard notice"
        );

        // 5. Failing exec double records exit code 1 and prints restart command
        let recorded_exit: Arc<std::sync::Mutex<Option<(i32, String)>>> =
            Arc::new(std::sync::Mutex::new(None));
        let exit_clone = recorded_exit.clone();
        let failing_exec: ExecFn = Arc::new(|_exe, _args, _tok| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "simulated exec failure",
            ))
        });
        let fail_exec_hub = test_hub();
        fail_exec_hub.set_switch_config(WorkspaceSwitchConfig {
            orig_args: vec!["--web".to_owned()],
            bound_port: Some(7432),
            current_exe: Some(std::env::current_exe().unwrap()),
            exec_hook: Some(failing_exec),
            on_exec_error: Some(Arc::new(move |code, msg| {
                *exit_clone.lock().unwrap() = Some((code, msg));
            })),
            unix_supported: Some(true),
        });
        let fail_exec_router = build_router_with(
            fail_exec_hub,
            token.clone(),
            allowed_localhost(),
            DashboardServices {
                web_attached: true,
                commands: Some(cmd_tx.clone()),
                ..DashboardServices::default()
            },
        );
        let resp = fail_exec_router
            .oneshot(json_request(
                "POST",
                "/api/workspace/switch",
                &token,
                &serde_json::json!({ "path": target_ws.to_string_lossy() }).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let (exit_code, stderr_msg) = recorded_exit.lock().unwrap().clone().unwrap();
        assert_eq!(exit_code, 1);
        assert!(stderr_msg.contains("simulated exec failure"));
        assert!(stderr_msg.contains(&format!(
            "dmuxai -C {} --web --port 7432",
            target_ws.display()
        )));

        loop_handle.abort();
        match prev_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match prev_state {
            Some(v) => unsafe { std::env::set_var("DMUXAI_STATE_DIR", v) },
            None => unsafe { std::env::remove_var("DMUXAI_STATE_DIR") },
        }
    }
}
