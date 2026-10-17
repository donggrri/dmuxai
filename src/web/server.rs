use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Path, Request, State};
use axum::http::header::HeaderName;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::Stream;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::WatchStream;

use crate::mcp::registry::{ImportError, McpRegistry};
use crate::memory::{MemoryError, MemoryHandle, Proposal, SaveOutcome};
use crate::todo::{NewTodo, TodoError, TodoPatch, TodoSource, TodoStatus};
use crate::web::DEFAULT_PORT;
use crate::web::auth::{BindCheck, check_bind, dashboard_url, token_matches};
use crate::web::commands::{DashboardCommand, RunMode, RunTodoError};
use crate::web::hub::DashboardHub;

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

async fn get_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = WatchStream::new(state.hub.subscribe()).filter_map(|mut snapshot| {
        snapshot.sent_at_ms = crate::web::snapshot::now_ms();
        serde_json::to_string(&snapshot)
            .ok()
            .map(|json| Ok(Event::default().event("snapshot").data(json)))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
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

fn run_error_response(error: RunTodoError) -> Response {
    match error {
        RunTodoError::Busy(message) => error_json(StatusCode::CONFLICT, &message),
        RunTodoError::AlreadyRunning => {
            error_json(StatusCode::CONFLICT, "to-do is already running")
        }
        RunTodoError::Forbidden(message) => error_json(StatusCode::FORBIDDEN, &message),
        RunTodoError::UnknownAgent(message) => error_json(StatusCode::NOT_FOUND, &message),
        RunTodoError::NotFound => error_json(StatusCode::NOT_FOUND, "to-do not found"),
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
        .route("/assets/app.css", get(get_app_css))
        .route("/api/state", get(get_state))
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
        .route("/api/todos", get(list_todos).post(create_todo))
        .route("/api/todos/{id}", patch(update_todo).delete(delete_todo))
        .route("/api/todos/{id}/run", post(run_todo))
        .route("/api/mcp/servers", get(list_mcp_servers))
        .route("/api/import", post(import_todos))
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
    let services = config.services.clone();
    let router = build_router_with(hub, config.token, allowed_hosts, services);

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
    use crate::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
    use crate::todo::{Todo, TodoStore};
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
                let DashboardCommand::RunTodo { reply, .. } = command;
                let _ = reply.send(outcome.clone());
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
                let DashboardCommand::RunTodo { mode, reply, .. } = command;
                seen.lock().unwrap().push(mode);
                let _ = reply.send(outcome.clone());
            }
        });
        tx
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
            memory: None,
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
            memory: None,
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
                todos: Some(store.clone()),
                commands: None,
                mcp: None,
                web_attached: false,
                memory: None,
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
                memory: None,
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
                memory: None,
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
                memory: None,
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
                memory: None,
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
                todos: None,
                commands: None,
                mcp: Some(Arc::new(registry)),
                web_attached: false,
                memory: None,
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
                todos: Some(store.clone()),
                commands: None,
                mcp: Some(registry.clone()),
                web_attached: false,
                memory: None,
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
                todos: None,
                commands: None,
                mcp: Some(registry),
                web_attached: false,
                memory: None,
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
                todos: Some(store.clone()),
                commands: None,
                mcp: Some(registry),
                web_attached: false,
                memory: None,
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
}
