//! Document preview and edit API routes (SPEC-docs-preview).
//!
//! Provides authenticated endpoints under `/api/docs/*` for browsing, reading,
//! editing, rendering and tracking workspace document changes, as well as capability-ticket
//! endpoints under `/docs/view/*` and `/docs/download/*`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Query, State};
use axum::http::header::HeaderName;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::docs::confine::Workspace;
use crate::docs::{DocKind, DocsError, MAX_TEXT_BYTES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketPurpose {
    View,
    Download,
}

struct TicketEntry {
    path: PathBuf,
    purpose: TicketPurpose,
    expires_at: Instant,
}

#[derive(Default)]
pub struct TicketStore {
    tickets: HashMap<String, TicketEntry>,
    order: VecDeque<String>,
}

impl TicketStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(&mut self, path: PathBuf, purpose: TicketPurpose) -> String {
        self.cleanup();
        while self.order.len() >= 256 {
            if let Some(old_id) = self.order.pop_front() {
                self.tickets.remove(&old_id);
            }
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let expires_at = Instant::now() + Duration::from_secs(300);
        self.tickets.insert(
            id.clone(),
            TicketEntry {
                path,
                purpose,
                expires_at,
            },
        );
        self.order.push_back(id.clone());
        id
    }

    pub fn get(&mut self, id: &str, expected_purpose: TicketPurpose) -> Option<PathBuf> {
        self.cleanup();
        let entry = self.tickets.get(id)?;
        if entry.purpose == expected_purpose && Instant::now() <= entry.expires_at {
            Some(entry.path.clone())
        } else {
            None
        }
    }

    fn cleanup(&mut self) {
        let now = Instant::now();
        self.tickets.retain(|_, v| v.expires_at > now);
        self.order.retain(|id| self.tickets.contains_key(id));
    }
}

struct OpenTicketEntry {
    relative_path: String,
    expires_at: Instant,
}

/// Single-use TUI open tickets (60 s, max 256). Shared with [`DocsService`] and the TUI.
#[derive(Default)]
pub struct OpenTickets {
    tickets: HashMap<String, OpenTicketEntry>,
    order: VecDeque<String>,
}

impl OpenTickets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issues a ticket bound to one workspace-relative path.
    pub fn mint(&mut self, relative_path: String) -> String {
        self.cleanup();
        while self.order.len() >= 256 {
            if let Some(old_id) = self.order.pop_front() {
                self.tickets.remove(&old_id);
            }
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let expires_at = Instant::now() + Duration::from_secs(60);
        self.tickets.insert(
            id.clone(),
            OpenTicketEntry {
                relative_path,
                expires_at,
            },
        );
        self.order.push_back(id.clone());
        id
    }

    /// Consumes a ticket on first successful use.
    pub fn take(&mut self, id: &str) -> Option<String> {
        self.cleanup();
        let entry = self.tickets.remove(id)?;
        self.order.retain(|ticket| ticket != id);
        if Instant::now() > entry.expires_at {
            return None;
        }
        Some(entry.relative_path)
    }

    fn cleanup(&mut self) {
        let now = Instant::now();
        self.tickets.retain(|_, v| v.expires_at > now);
        self.order.retain(|id| self.tickets.contains_key(id));
    }
}

pub struct DocsService {
    pub workspace: Workspace,
    pub read_only: bool,
    pub changes: crate::docs::changes::ChangeLog,
    pub soffice: Option<PathBuf>,
    pub tickets: Arc<Mutex<TicketStore>>,
    pub open_tickets: Arc<Mutex<OpenTickets>>,
    pub(crate) dashboard_token: String,
}

impl DocsService {
    pub fn new(
        workspace: Workspace,
        read_only: bool,
        changes: crate::docs::changes::ChangeLog,
        soffice: Option<PathBuf>,
        open_tickets: Arc<Mutex<OpenTickets>>,
        dashboard_token: String,
    ) -> Self {
        Self {
            workspace,
            read_only,
            changes,
            soffice,
            tickets: Arc::new(Mutex::new(TicketStore::new())),
            open_tickets,
            dashboard_token,
        }
    }
}

#[derive(Clone)]
pub struct DocsApiState {
    pub docs: Option<Arc<DocsService>>,
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

fn docs_error_response(error: DocsError) -> Response {
    match error {
        DocsError::OutsideWorkspace => error_json(StatusCode::FORBIDDEN, "path outside workspace"),
        DocsError::NotFound => error_json(StatusCode::NOT_FOUND, "not found"),
        DocsError::NotAFile => error_json(StatusCode::BAD_REQUEST, "not a file"),
        DocsError::TooLarge => error_json(StatusCode::PAYLOAD_TOO_LARGE, "too large"),
        DocsError::HtmlPreviewOnly => error_json(StatusCode::FORBIDDEN, "html is preview-only"),
        DocsError::NotEditable => error_json(StatusCode::UNSUPPORTED_MEDIA_TYPE, "not editable"),
        DocsError::Conflict { sha256, mtime_ms } => {
            let body = serde_json::json!({
                "error": "conflict",
                "sha256": sha256,
                "mtime_ms": mtime_ms,
            })
            .to_string();
            (
                StatusCode::CONFLICT,
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (header::CACHE_CONTROL, "no-store"),
                    (HeaderName::from_static("x-content-type-options"), "nosniff"),
                ],
                body,
            )
                .into_response()
        }
        DocsError::NotUtf8 => error_json(StatusCode::UNSUPPORTED_MEDIA_TYPE, "not utf-8"),
        DocsError::Io(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
}

struct DocsJson<T>(T);

impl<S, T> axum::extract::FromRequest<S> for DocsJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(DocsJson(value)),
            Err(rejection) => {
                use axum::extract::rejection::JsonRejection;
                let (status, message) = match rejection {
                    JsonRejection::MissingJsonContentType(_) => {
                        (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported media type")
                    }
                    JsonRejection::JsonSyntaxError(_) | JsonRejection::JsonDataError(_) => {
                        (StatusCode::BAD_REQUEST, "invalid json")
                    }
                    JsonRejection::BytesRejection(_) => {
                        (StatusCode::PAYLOAD_TOO_LARGE, "too large")
                    }
                    _ => (StatusCode::BAD_REQUEST, "invalid request"),
                };
                Err(error_json(status, message))
            }
        }
    }
}

#[derive(Deserialize)]
struct FilePathQuery {
    #[serde(default)]
    path: String,
    /// `code`: the Files viewer's read; text files that can be highlighted
    /// get `code_html` instead of `content`.
    #[serde(default)]
    view: Option<String>,
}

#[derive(Deserialize)]
struct SaveFileBody {
    path: String,
    content: String,
    base_sha256: String,
}

#[derive(Deserialize)]
struct RenderPathQuery {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct RenderTextBody {
    text: String,
}

#[derive(Deserialize)]
struct ViewBody {
    path: String,
}

#[derive(Deserialize)]
struct ChangesQuery {
    #[serde(default)]
    node: Option<String>,
}

async fn get_docs_js() -> impl IntoResponse {
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
        crate::web::assets::DOCS_JS,
    )
}

async fn get_docs_css() -> impl IntoResponse {
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
        crate::web::assets::DOCS_CSS,
    )
}

async fn get_docs(State(state): State<DocsApiState>) -> Response {
    let Some(service) = &state.docs else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "docs unavailable");
    };
    let list = crate::docs::list::list_documents(&service.workspace, crate::docs::MAX_LIST_ENTRIES);
    json_ok(StatusCode::OK, &list)
}

#[derive(Debug, Deserialize)]
struct TreeQuery {
    #[serde(default)]
    dir: String,
}

async fn get_tree(State(state): State<DocsApiState>, Query(query): Query<TreeQuery>) -> Response {
    let Some(service) = &state.docs else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "docs unavailable");
    };
    match crate::docs::list::list_children(
        &service.workspace,
        &query.dir,
        crate::docs::MAX_LIST_ENTRIES,
    ) {
        Ok(tree) => json_ok(StatusCode::OK, &tree),
        Err(e) => docs_error_response(e),
    }
}

async fn get_file(
    State(state): State<DocsApiState>,
    Query(query): Query<FilePathQuery>,
) -> Response {
    let Some(service) = &state.docs else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "docs unavailable");
    };
    // Reading, hashing and highlighting are blocking work: keep them off
    // the async runtime threads.
    let workspace = service.workspace.clone();
    let code_view = query.view.as_deref() == Some("code");
    let path = query.path;
    let read = tokio::task::spawn_blocking(move || {
        crate::docs::file::read_file_with_view(&workspace, &path, code_view)
    })
    .await;
    match read {
        Ok(Ok(doc_file)) => json_ok(StatusCode::OK, &doc_file),
        Ok(Err(e)) => docs_error_response(e),
        Err(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "read failed"),
    }
}

async fn put_file(
    State(state): State<DocsApiState>,
    DocsJson(body): DocsJson<SaveFileBody>,
) -> Response {
    let Some(service) = &state.docs else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "docs unavailable");
    };

    // 1. Resolve path (404 / 403 / 400)
    let resolved = match service.workspace.resolve(&body.path) {
        Ok(r) => r,
        Err(e) => return docs_error_response(e),
    };
    let metadata = match std::fs::metadata(&resolved.absolute) {
        Ok(m) => m,
        Err(e) => return docs_error_response(DocsError::from(e)),
    };
    if metadata.is_dir() {
        return docs_error_response(DocsError::NotAFile);
    }

    // 2. Kind check
    let kind = DocKind::for_path(&resolved.absolute);
    let requested_kind = DocKind::for_path(std::path::Path::new(&resolved.relative));
    if kind == DocKind::Html || requested_kind == DocKind::Html {
        return error_json(StatusCode::FORBIDDEN, "html is preview-only");
    }
    if !kind.editable() || !requested_kind.editable() {
        return error_json(StatusCode::UNSUPPORTED_MEDIA_TYPE, "not editable");
    }

    // 3. Read-only check
    if service.read_only {
        return error_json(StatusCode::FORBIDDEN, "read-only: start dmuxai --web");
    }

    // 4. Payload size check
    if body.content.len() as u64 > MAX_TEXT_BYTES {
        return error_json(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    }

    // 5. Save with concurrency check and atomic rename
    match crate::docs::file::save_file(
        &service.workspace,
        &body.path,
        &body.content,
        &body.base_sha256,
    ) {
        Ok(outcome) => json_ok(StatusCode::OK, &outcome),
        Err(e) => docs_error_response(e),
    }
}

async fn get_render(
    State(state): State<DocsApiState>,
    Query(query): Query<RenderPathQuery>,
) -> Response {
    let Some(service) = &state.docs else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "docs unavailable");
    };

    let resolved = match service.workspace.resolve(&query.path) {
        Ok(r) => r,
        Err(e) => return docs_error_response(e),
    };
    let metadata = match std::fs::metadata(&resolved.absolute) {
        Ok(m) => m,
        Err(e) => return docs_error_response(DocsError::from(e)),
    };
    if metadata.is_dir() {
        return docs_error_response(DocsError::NotAFile);
    }
    let kind = DocKind::for_path(&resolved.absolute);
    if kind != DocKind::Markdown {
        return error_json(StatusCode::UNSUPPORTED_MEDIA_TYPE, "not markdown");
    }
    if metadata.len() > MAX_TEXT_BYTES {
        return error_json(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    }
    let bytes = match std::fs::read(&resolved.absolute) {
        Ok(b) => b,
        Err(e) => return docs_error_response(DocsError::from(e)),
    };
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        return error_json(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    }
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return docs_error_response(DocsError::NotUtf8),
    };
    let html = crate::docs::markdown::render_markdown(&text);
    json_ok(StatusCode::OK, &serde_json::json!({ "html": html }))
}

async fn post_render(
    State(_state): State<DocsApiState>,
    DocsJson(body): DocsJson<RenderTextBody>,
) -> Response {
    if body.text.len() as u64 > MAX_TEXT_BYTES {
        return error_json(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    }
    let html = crate::docs::markdown::render_markdown(&body.text);
    json_ok(StatusCode::OK, &serde_json::json!({ "html": html }))
}

async fn post_view(
    State(state): State<DocsApiState>,
    DocsJson(body): DocsJson<ViewBody>,
) -> Response {
    let Some(service) = &state.docs else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "docs unavailable");
    };

    let resolved = match service.workspace.resolve(&body.path) {
        Ok(r) => r,
        Err(e) => return docs_error_response(e),
    };
    let metadata = match std::fs::metadata(&resolved.absolute) {
        Ok(m) => m,
        Err(e) => return docs_error_response(DocsError::from(e)),
    };
    if metadata.is_dir() {
        return docs_error_response(DocsError::NotAFile);
    }

    let kind = DocKind::for_path(&resolved.absolute);
    match kind {
        DocKind::Html => {
            let ticket = service
                .tickets
                .lock()
                .unwrap()
                .create(resolved.absolute.clone(), TicketPurpose::View);
            json_ok(
                StatusCode::OK,
                &serde_json::json!({
                    "preview": "frame",
                    "kind": "html",
                    "url": format!("/docs/view/{ticket}"),
                }),
            )
        }
        DocKind::Pdf => {
            let ticket = service
                .tickets
                .lock()
                .unwrap()
                .create(resolved.absolute.clone(), TicketPurpose::View);
            json_ok(
                StatusCode::OK,
                &serde_json::json!({
                    "preview": "frame",
                    "kind": "pdf",
                    "url": format!("/docs/view/{ticket}"),
                }),
            )
        }
        DocKind::Office => {
            let dl_ticket = service
                .tickets
                .lock()
                .unwrap()
                .create(resolved.absolute.clone(), TicketPurpose::Download);
            let download_url = format!("/docs/download/{dl_ticket}");

            if metadata.len() > MAX_RAW_BYTES {
                return json_ok(
                    StatusCode::OK,
                    &serde_json::json!({
                        "preview": "unavailable",
                        "reason": "file too large to convert",
                        "download": download_url,
                    }),
                );
            }
            let bytes = match std::fs::read(&resolved.absolute) {
                Ok(b) => b,
                Err(e) => return docs_error_response(DocsError::from(e)),
            };
            let sha256 = crate::docs::sha256_hex(&bytes);
            drop(bytes);
            let soffice = service.soffice.clone();
            let source_path = resolved.absolute.clone();

            let preview = tokio::task::spawn_blocking(move || {
                crate::docs::office::convert_to_pdf(
                    soffice.as_deref(),
                    &source_path,
                    &sha256,
                    Duration::from_secs(60),
                )
            })
            .await;

            match preview {
                Ok(crate::docs::office::OfficePreview::Pdf(pdf_path)) => {
                    let view_ticket = service
                        .tickets
                        .lock()
                        .unwrap()
                        .create(pdf_path, TicketPurpose::View);
                    json_ok(
                        StatusCode::OK,
                        &serde_json::json!({
                            "preview": "pdf",
                            "url": format!("/docs/view/{view_ticket}"),
                            "download": download_url,
                        }),
                    )
                }
                Ok(crate::docs::office::OfficePreview::Unavailable { reason }) => json_ok(
                    StatusCode::OK,
                    &serde_json::json!({
                        "preview": "unavailable",
                        "reason": reason,
                        "download": download_url,
                    }),
                ),
                Err(_) => json_ok(
                    StatusCode::OK,
                    &serde_json::json!({
                        "preview": "unavailable",
                        "reason": "conversion failed",
                        "download": download_url,
                    }),
                ),
            }
        }
        _ => {
            let dl_ticket = service
                .tickets
                .lock()
                .unwrap()
                .create(resolved.absolute.clone(), TicketPurpose::Download);
            json_ok(
                StatusCode::OK,
                &serde_json::json!({
                    "preview": "download",
                    "download": format!("/docs/download/{dl_ticket}"),
                }),
            )
        }
    }
}

async fn get_view_ticket(
    State(state): State<DocsApiState>,
    AxumPath(ticket): AxumPath<String>,
) -> Response {
    let Some(service) = &state.docs else {
        return error_json(StatusCode::NOT_FOUND, "not found");
    };
    let path = match service
        .tickets
        .lock()
        .unwrap()
        .get(&ticket, TicketPurpose::View)
    {
        Some(p) => p,
        None => return error_json(StatusCode::NOT_FOUND, "ticket not found or expired"),
    };
    // Re-check confinement at use time: the file may have been swapped for a
    // symlink that points outside the workspace after the ticket was issued.
    let Some(path) = confined_ticket_path(service, &path) else {
        return error_json(StatusCode::NOT_FOUND, "ticket not found or expired");
    };
    let bytes = match read_raw(&path) {
        Ok(b) => b,
        Err(response) => return response,
    };
    let kind = DocKind::for_path(&path);
    if kind == DocKind::Html {
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (
                    HeaderName::from_static("content-security-policy"),
                    "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:",
                ),
                (HeaderName::from_static("x-content-type-options"), "nosniff"),
                (header::CACHE_CONTROL, "no-store"),
                (HeaderName::from_static("referrer-policy"), "no-referrer"),
            ],
            bytes,
        )
            .into_response()
    } else if kind == DocKind::Pdf || path.extension().and_then(|e| e.to_str()) == Some("pdf") {
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/pdf"),
                (header::CONTENT_DISPOSITION, "inline"),
                (header::CACHE_CONTROL, "no-store"),
                (HeaderName::from_static("x-content-type-options"), "nosniff"),
            ],
            bytes,
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (HeaderName::from_static("x-content-type-options"), "nosniff"),
            ],
            bytes,
        )
            .into_response()
    }
}

async fn get_download_ticket(
    State(state): State<DocsApiState>,
    AxumPath(ticket): AxumPath<String>,
) -> Response {
    let Some(service) = &state.docs else {
        return error_json(StatusCode::NOT_FOUND, "not found");
    };
    let path = match service
        .tickets
        .lock()
        .unwrap()
        .get(&ticket, TicketPurpose::Download)
    {
        Some(p) => p,
        None => return error_json(StatusCode::NOT_FOUND, "ticket not found or expired"),
    };
    // Re-check confinement at use time: the file may have been swapped for a
    // symlink that points outside the workspace after the ticket was issued.
    let Some(path) = confined_ticket_path(service, &path) else {
        return error_json(StatusCode::NOT_FOUND, "ticket not found or expired");
    };
    let bytes = match read_raw(&path) {
        Ok(b) => b,
        Err(response) => return response,
    };
    let filename = safe_filename(&path);
    let disposition = format!("attachment; filename=\"{filename}\"");
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, disposition.as_str()),
            (header::CACHE_CONTROL, "no-store"),
            (HeaderName::from_static("x-content-type-options"), "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

fn plain_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "not found",
    )
        .into_response()
}

fn percent_encode_doc_fragment(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b'/' => encoded.push_str("%2F"),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

async fn get_open_ticket(
    State(state): State<DocsApiState>,
    AxumPath(ticket): AxumPath<String>,
) -> Response {
    let Some(service) = &state.docs else {
        return plain_not_found();
    };
    let relative = service
        .open_tickets
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take(&ticket);
    let Some(relative) = relative else {
        return plain_not_found();
    };
    if service.workspace.resolve(&relative).is_err() {
        return plain_not_found();
    }
    let encoded = percent_encode_doc_fragment(&relative);
    // The token never travels in a URL: a tiny no-store page stashes it in
    // sessionStorage (the dashboard's TOKEN_KEY) and replaces itself with
    // `/#doc=...`, so history, Referer and proxy logs only see the ticket.
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let token_js = serde_json::to_string(&service.dashboard_token).unwrap_or_default();
    let target_js = serde_json::to_string(&format!("/#doc={encoded}")).unwrap_or_default();
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>dmuxai</title>\
         <script nonce=\"{nonce}\">try{{sessionStorage.setItem('dmuxai.token',{token_js});}}catch(e){{}}\
         location.replace({target_js});</script>"
    );
    let csp = format!("default-src 'none'; script-src 'nonce-{nonce}'; base-uri 'none'");
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (
                HeaderName::from_static("referrer-policy"),
                "no-referrer".to_string(),
            ),
            (header::CONTENT_SECURITY_POLICY, csp),
        ],
        body,
    )
        .into_response()
}

async fn get_changes(
    State(state): State<DocsApiState>,
    Query(query): Query<ChangesQuery>,
) -> Response {
    let Some(service) = &state.docs else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "docs unavailable");
    };
    if service.read_only {
        return json_ok(StatusCode::OK, &serde_json::json!({ "turns": [] }));
    }
    let turns = service.changes.list(query.node.as_deref());
    json_ok(StatusCode::OK, &serde_json::json!({ "turns": turns }))
}

/// Builds the docs router. Compatible with any parent router state.
/// Largest file served through a view/download ticket (read into memory).
const MAX_RAW_BYTES: u64 = 100 * 1024 * 1024;

/// Canonical ticket target if it is still inside the workspace or the
/// per-user Office PDF cache (NFR-751).
fn confined_ticket_path(service: &DocsService, path: &std::path::Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).ok()?;
    if service.workspace.relative_of(&canonical).is_some() {
        return Some(canonical);
    }
    let cache = std::fs::canonicalize(crate::docs::office::cache_dir()).ok()?;
    canonical.starts_with(&cache).then_some(canonical)
}

#[allow(clippy::result_large_err)]
fn read_raw(path: &std::path::Path) -> Result<Vec<u8>, Response> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_RAW_BYTES => {
            return Err(error_json(StatusCode::PAYLOAD_TOO_LARGE, "too large"));
        }
        Ok(_) => {}
        Err(_) => return Err(error_json(StatusCode::NOT_FOUND, "file not found")),
    }
    std::fs::read(path).map_err(|_| error_json(StatusCode::NOT_FOUND, "file not found"))
}

/// A `Content-Disposition` filename limited to safe characters.
fn safe_filename(path: &std::path::Path) -> String {
    let name: String = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.trim().is_empty() {
        "document".to_owned()
    } else {
        name
    }
}

pub fn router<S>(docs: Option<Arc<DocsService>>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let state = DocsApiState { docs };
    Router::new()
        .route("/assets/docs.js", get(get_docs_js))
        .route("/assets/docs.css", get(get_docs_css))
        .route("/api/docs", get(get_docs))
        .route("/api/docs/tree", get(get_tree))
        .route("/api/docs/file", get(get_file).put(put_file))
        .route("/api/docs/render", get(get_render).post(post_render))
        .route("/api/docs/view", post(post_view))
        .route("/docs/view/{ticket}", get(get_view_ticket))
        .route("/docs/download/{ticket}", get(get_download_ticket))
        .route("/docs/open/{ticket}", get(get_open_ticket))
        .route("/api/docs/changes", get(get_changes))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::web::hub::DashboardHub;
    use crate::web::server::{DashboardServices, build_router_with};
    use crate::web::snapshot::SnapshotSource;

    fn test_docs_service(
        ws: Workspace,
        read_only: bool,
        changes: crate::docs::changes::ChangeLog,
        soffice: Option<PathBuf>,
    ) -> Arc<DocsService> {
        Arc::new(DocsService::new(
            ws,
            read_only,
            changes,
            soffice,
            Arc::new(Mutex::new(OpenTickets::new())),
            "test-token".to_string(),
        ))
    }

    fn test_router(service: Option<Arc<DocsService>>, token: &str) -> axum::Router {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let services = DashboardServices {
            docs: service,
            web_attached: true,
            ..Default::default()
        };
        build_router_with(
            hub,
            token.to_string(),
            Some(vec![
                "localhost:7420".to_string(),
                "127.0.0.1:7420".to_string(),
            ]),
            services,
        )
    }

    // TC-743: read_file_contract (FR-742, NFR-752)
    #[tokio::test]
    async fn read_file_contract() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("ok.md"), "# Hello Markdown").unwrap();
        let png_bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        std::fs::write(dir.path().join("img.png"), png_bytes).unwrap();
        let large_bytes = vec![b'x'; (MAX_TEXT_BYTES + 4096) as usize];
        std::fs::write(dir.path().join("large.txt"), &large_bytes).unwrap();

        let token = "testtoken";
        let service =
            test_docs_service(ws, false, crate::docs::changes::ChangeLog::default(), None);
        let router = test_router(Some(service), token);

        // 1. Markdown file -> 200 with text content, sha256, editable true
        let req = axum::http::Request::builder()
            .uri("/api/docs/file?path=ok.md")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["kind"], "markdown");
        assert_eq!(val["editable"], true);
        assert_eq!(val["content"], "# Hello Markdown");
        assert_eq!(val["sha256"], crate::docs::sha256_hex(b"# Hello Markdown"));

        // 2. Binary file -> 200, content null, editable false, sha256 present
        let req = axum::http::Request::builder()
            .uri("/api/docs/file?path=img.png")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["kind"], "binary");
        assert_eq!(val["editable"], false);
        assert!(val["content"].is_null());
        assert_eq!(val["sha256"], crate::docs::sha256_hex(&png_bytes));

        // 3. File > 2 MiB -> 200, content null, editable false, sha256 present
        let req = axum::http::Request::builder()
            .uri("/api/docs/file?path=large.txt")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["kind"], "text");
        assert_eq!(val["editable"], false);
        assert!(val["content"].is_null());
        assert_eq!(val["sha256"], crate::docs::sha256_hex(&large_bytes));

        // 4. Directory -> 400
        let req = axum::http::Request::builder()
            .uri("/api/docs/file?path=sub")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // 5. Missing file -> 404
        let req = axum::http::Request::builder()
            .uri("/api/docs/file?path=missing.md")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 6. Traversal outside workspace -> 403
        let req = axum::http::Request::builder()
            .uri("/api/docs/file?path=../outside.md")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    async fn get_json(router: &axum::Router, uri: &str, token: &str) -> serde_json::Value {
        let req = axum::http::Request::builder()
            .uri(uri)
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    // Files code view: `view=code` reads of highlightable text files carry
    // `code_html` (not `content`); a plain read still returns the raw text
    // for editing; unknown syntax and over-budget files get `content` only;
    // file markup is escaped; CRLF is one line; saving works on raw text.
    #[tokio::test]
    async fn read_text_file_has_code_view() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let cpp = "#include <cstdio>\nint main() {\n  return 0;\n}\n";
        let py = "def add(a, b):\n    return a + b\n";
        let sh = "#!/bin/sh\necho \"hi\"\n";
        let evil = "x = 1\n</code></pre><script>alert(1)</script>\n";
        let crlf = "a = 1\r\nb = 2\r\n";
        std::fs::write(dir.path().join("main.cpp"), cpp).unwrap();
        std::fs::write(dir.path().join("tool.py"), py).unwrap();
        std::fs::write(dir.path().join("run.sh"), sh).unwrap();
        std::fs::write(dir.path().join("evil.py"), evil).unwrap();
        std::fs::write(dir.path().join("crlf.py"), crlf).unwrap();
        std::fs::write(dir.path().join("notes.log"), "plain\n").unwrap();
        let big = "x = 1\n".repeat(crate::docs::highlight::MAX_HIGHLIGHT_BYTES / 6 + 10);
        std::fs::write(dir.path().join("big.py"), &big).unwrap();
        let nested = "[\n".repeat(128 * 1024);
        std::fs::write(dir.path().join("nested.json"), &nested).unwrap();
        std::fs::write(dir.path().join("ok.md"), "# Title\n").unwrap();

        let token = "testtoken";
        let service =
            test_docs_service(ws, false, crate::docs::changes::ChangeLog::default(), None);
        let router = test_router(Some(service), token);

        for (path, raw, language, lines) in [
            ("main.cpp", cpp, "C++", 4),
            ("tool.py", py, "Python", 2),
            ("run.sh", sh, "Bourne Again Shell (bash)", 2),
        ] {
            let val = get_json(
                &router,
                &format!("/api/docs/file?path={path}&view=code"),
                token,
            )
            .await;
            assert_eq!(val["kind"], "text", "{path}");
            assert_eq!(val["editable"], true, "{path}");
            assert_eq!(val["language"], language, "{path}");
            assert!(val["content"].is_null(), "{path}: no duplicate raw text");
            assert_eq!(val["content_omitted"], true, "{path}");
            assert_eq!(val["sha256"], crate::docs::sha256_hex(raw.as_bytes()));
            let html = val["code_html"].as_str().expect("code_html");
            assert!(
                html.starts_with("<pre class=\"code-view\"><code class=\"language-"),
                "{path}: {html}"
            );
            assert!(html.contains("class=\"hl-"), "{path}: {html}");
            assert_eq!(html.matches("class=\"code-line\"").count(), lines, "{path}");
            // The plain read (used to start editing) returns the raw text.
            let plain = get_json(&router, &format!("/api/docs/file?path={path}"), token).await;
            assert_eq!(plain["content"], raw, "{path}");
            assert!(plain.get("code_html").is_none(), "{path}");
            assert!(plain.get("content_omitted").is_none(), "{path}");
        }

        let val = get_json(&router, "/api/docs/file?path=evil.py&view=code", token).await;
        let html = val["code_html"].as_str().unwrap();
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("</code></pre><"), "{html}");

        let val = get_json(&router, "/api/docs/file?path=crlf.py&view=code", token).await;
        let html = val["code_html"].as_str().unwrap();
        assert_eq!(html.matches("class=\"code-line\"").count(), 2, "{html}");
        assert!(!html.contains('\r'));
        let plain = get_json(&router, "/api/docs/file?path=crlf.py", token).await;
        assert_eq!(plain["content"], crlf, "raw CRLF kept for editing");

        for path in ["notes.log", "big.py", "nested.json"] {
            let val = get_json(
                &router,
                &format!("/api/docs/file?path={path}&view=code"),
                token,
            )
            .await;
            assert!(val.get("code_html").is_none(), "{path}: plain view");
            assert!(
                val["content"].is_string(),
                "{path}: raw text for the plain view"
            );
        }
        let val = get_json(&router, "/api/docs/file?path=nested.json&view=code", token).await;
        let bytes = serde_json::to_vec(&val).unwrap().len();
        assert!(
            bytes < nested.len() * 2,
            "nested file response stays small: {bytes}"
        );

        let val = get_json(&router, "/api/docs/file?path=ok.md&view=code", token).await;
        assert!(
            val.get("code_html").is_none(),
            "markdown keeps its own renderer"
        );
        assert_eq!(val["content"], "# Title\n");

        // Save the raw text, then read back a fresh code view.
        let base = crate::docs::sha256_hex(py.as_bytes());
        let updated = "def add(a, b):\n    return a + b\n\nprint(add(1, 2))\n";
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({ "path": "tool.py", "content": updated, "base_sha256": base })
                    .to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tool.py")).unwrap(),
            updated
        );
        let val = get_json(&router, "/api/docs/file?path=tool.py&view=code", token).await;
        assert_eq!(val["sha256"], crate::docs::sha256_hex(updated.as_bytes()));
        let html = val["code_html"].as_str().unwrap();
        assert_eq!(html.matches("class=\"code-line\"").count(), 4);
        assert!(html.contains("print"));

        // A stale base hash is still a conflict.
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({ "path": "tool.py", "content": "x", "base_sha256": base })
                    .to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    // TC-746: save_with_concurrency_check (FR-744, NFR-752)
    #[tokio::test]
    async fn save_with_concurrency_check() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let file_path = dir.path().join("doc.md");
        std::fs::write(&file_path, "initial content").unwrap();
        let base_hash = crate::docs::sha256_hex(b"initial content");

        let token = "testtoken";
        let service =
            test_docs_service(ws, false, crate::docs::changes::ChangeLog::default(), None);
        let router = test_router(Some(service), token);

        // 1. PUT with correct base_sha256 -> 200, file updated
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "path": "doc.md",
                    "content": "updated content",
                    "base_sha256": base_hash,
                })
                .to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let new_hash = crate::docs::sha256_hex(b"updated content");
        assert_eq!(val["sha256"], new_hash);
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "updated content"
        );

        // 2. PUT with stale hash -> 409 conflict, file unchanged
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "path": "doc.md",
                    "content": "conflicting edit",
                    "base_sha256": base_hash, // Stale!
                })
                .to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["error"], "conflict");
        assert_eq!(val["sha256"], new_hash);
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "updated content"
        );

        // 3. PUT with payload > 2 MiB (e.g. 3 MiB) -> 413 Payload Too Large, file unchanged
        let large_payload = "a".repeat(3 * 1024 * 1024);
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "path": "doc.md",
                    "content": large_payload,
                    "base_sha256": new_hash,
                })
                .to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "updated content"
        );
    }

    // TC-747: save_rejections (FR-745)
    #[tokio::test]
    async fn save_rejections() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let html_file = dir.path().join("page.html");
        std::fs::write(&html_file, "<h1>preview only</h1>").unwrap();
        let png_file = dir.path().join("img.png");
        std::fs::write(&png_file, [1, 2, 3]).unwrap();
        let md_file = dir.path().join("doc.md");
        std::fs::write(&md_file, "# markdown").unwrap();

        let token = "testtoken";
        let web_service = test_docs_service(
            ws.clone(),
            false,
            crate::docs::changes::ChangeLog::default(),
            None,
        );
        let web_router = test_router(Some(web_service), token);

        let serve_service = test_docs_service(
            ws,
            true, // read_only
            crate::docs::changes::ChangeLog::default(),
            None,
        );
        let serve_router = test_router(Some(serve_service), token);

        // Against --web: HTML file -> 403 html is preview-only
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "path": "page.html",
                    "content": "<h1>modified</h1>",
                    "base_sha256": "anything",
                })
                .to_string(),
            ))
            .unwrap();
        let resp = web_router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["error"], "html is preview-only");
        assert_eq!(
            std::fs::read_to_string(&html_file).unwrap(),
            "<h1>preview only</h1>"
        );

        // Against --web: PNG file -> 415 not editable
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "path": "img.png",
                    "content": "modified",
                    "base_sha256": "anything",
                })
                .to_string(),
            ))
            .unwrap();
        let resp = web_router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(std::fs::read(&png_file).unwrap(), vec![1, 2, 3]);

        // Against serve (read_only = true): HTML file -> 403 html is preview-only
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "path": "page.html",
                    "content": "<h1>modified</h1>",
                    "base_sha256": "anything",
                })
                .to_string(),
            ))
            .unwrap();
        let resp = serve_router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["error"], "html is preview-only");

        // Against serve: PNG file -> 415 not editable
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "path": "img.png",
                    "content": "modified",
                    "base_sha256": "anything",
                })
                .to_string(),
            ))
            .unwrap();
        let resp = serve_router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        // Against serve: Markdown file -> 403 read-only: start dmuxai --web
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/docs/file")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "path": "doc.md",
                    "content": "# modified",
                    "base_sha256": crate::docs::sha256_hex(b"# markdown"),
                })
                .to_string(),
            ))
            .unwrap();
        let resp = serve_router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["error"], "read-only: start dmuxai --web");
        assert_eq!(std::fs::read_to_string(&md_file).unwrap(), "# markdown");
    }

    // TC-748: html_view_ticket_is_sandboxed (FR-746)
    #[tokio::test]
    async fn html_view_ticket_is_sandboxed() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        std::fs::write(dir.path().join("page.html"), "<h1>Sandbox Test</h1>").unwrap();

        let token = "testtoken";
        let service =
            test_docs_service(ws, false, crate::docs::changes::ChangeLog::default(), None);
        let router = test_router(Some(service), token);

        // 1. POST /api/docs/view
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/docs/view")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({ "path": "page.html" }).to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["preview"], "frame");
        assert_eq!(val["kind"], "html");
        let view_url = val["url"].as_str().unwrap().to_string();
        let ticket_id = view_url.strip_prefix("/docs/view/").unwrap().to_string();

        // 2. GET /docs/view/{ticket} carries sandbox headers
        let req = axum::http::Request::builder()
            .uri(&view_url)
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let headers = resp.headers();
        assert_eq!(
            headers.get("content-security-policy").unwrap(),
            "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:"
        );
        assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(headers.get("cache-control").unwrap(), "no-store");
        assert_eq!(headers.get("referrer-policy").unwrap(), "no-referrer");
        assert!(headers.get("x-frame-options").is_none());
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), b"<h1>Sandbox Test</h1>");

        // 3. GET /docs/view/bogus -> 404
        let req = axum::http::Request::builder()
            .uri("/docs/view/bogusticket1234")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 4. Ticket cannot authorize /api/* -> 401
        let req = axum::http::Request::builder()
            .uri(format!("/api/state?token={ticket_id}"))
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // TC-749: office_falls_back_without_soffice (FR-747)
    #[tokio::test]
    async fn office_falls_back_without_soffice() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let docx_bytes = b"PKmock office document bytes";
        std::fs::write(dir.path().join("x.docx"), docx_bytes).unwrap();

        let token = "testtoken";
        let service = test_docs_service(
            ws,
            false,
            crate::docs::changes::ChangeLog::default(),
            Some(PathBuf::from("/nonexistent/bin/soffice")),
        );
        let router = test_router(Some(service), token);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/docs/view")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({ "path": "x.docx" }).to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["preview"], "unavailable");
        assert!(val["reason"].is_string());
        let download_url = val["download"].as_str().unwrap().to_string();

        let req = axum::http::Request::builder()
            .uri(&download_url)
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let headers = resp.headers();
        assert_eq!(
            headers.get("content-disposition").unwrap(),
            "attachment; filename=\"x.docx\""
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), docx_bytes);
    }

    // TC-750: office_converts_with_soffice (FR-747)
    #[ignore]
    #[tokio::test]
    async fn office_converts_with_soffice() {
        let soffice = crate::docs::office::find_soffice();
        assert!(soffice.is_some(), "soffice must be available for this test");

        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let rtf_content =
            r"{\rtf1\ansi\deff0 {\fonttbl {\f0 Courier;}}\f0\fs24 LibreOffice PDF Test Document}";
        std::fs::write(dir.path().join("doc.rtf"), rtf_content).unwrap();

        let token = "testtoken";
        let service = test_docs_service(
            ws,
            false,
            crate::docs::changes::ChangeLog::default(),
            soffice,
        );
        let router = test_router(Some(service), token);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/docs/view")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({ "path": "doc.rtf" }).to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["preview"], "pdf");
        let view_url = val["url"].as_str().unwrap().to_string();

        let req = axum::http::Request::builder()
            .uri(&view_url)
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/pdf"
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(body.starts_with(b"%PDF-"));
    }

    // TC-752: changes_api_filters_and_bounds (FR-748, NFR-752)
    #[tokio::test]
    async fn changes_api_filters_and_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let changes = crate::docs::changes::ChangeLog::default();

        for i in 0..60 {
            let node = if i % 2 == 0 { "a" } else { "b" };
            changes.push(crate::docs::changes::TurnChanges {
                id: format!("turn-{i}"),
                agent: "agent".to_string(),
                graph_id: Some("graph".to_string()),
                node_id: Some(node.to_string()),
                started_ms: 1000 + i,
                finished_ms: 1050 + i,
                files: vec![],
                truncated: false,
                output_tail: vec![],
                links: vec![],
                message_id: None,
            });
        }

        let token = "testtoken";
        let web_service = test_docs_service(ws.clone(), false, changes.clone(), None);
        let web_router = test_router(Some(web_service), token);

        let serve_service = test_docs_service(
            ws, true, // read_only
            changes, None,
        );
        let serve_router = test_router(Some(serve_service), token);

        // 1. GET /api/docs/changes -> 50 kept, newest first
        let req = axum::http::Request::builder()
            .uri("/api/docs/changes")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = web_router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let turns = val["turns"].as_array().unwrap();
        assert_eq!(turns.len(), 50);
        assert_eq!(turns[0]["id"], "turn-59");
        assert_eq!(turns[49]["id"], "turn-10");

        // 2. GET /api/docs/changes?node=a -> only node "a"
        let req = axum::http::Request::builder()
            .uri("/api/docs/changes?node=a")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = web_router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let turns = val["turns"].as_array().unwrap();
        assert_eq!(turns.len(), 25);
        for turn in turns {
            assert_eq!(turn["node_id"], "a");
        }

        // 3. Against serve router -> empty list
        let req = axum::http::Request::builder()
            .uri("/api/docs/changes")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = serve_router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["turns"].as_array().unwrap().len(), 0);
    }

    // TC-756: open_ticket_is_single_use
    #[tokio::test]
    async fn open_ticket_is_single_use() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        std::fs::write(dir.path().join("docs/a.md"), "hello").unwrap();
        let open = Arc::new(Mutex::new(OpenTickets::new()));
        let service = Arc::new(DocsService::new(
            ws,
            false,
            crate::docs::changes::ChangeLog::default(),
            None,
            open.clone(),
            "sectok".to_string(),
        ));
        let ticket = open.lock().unwrap().mint("docs/a.md".to_string());
        let router = test_router(Some(service), "sectok");

        let req = axum::http::Request::builder()
            .uri(format!("/docs/open/{ticket}"))
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // The token is never put in a URL (no redirect Location).
        assert!(resp.headers().get(header::LOCATION).is_none());
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let csp = resp
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        assert!(csp.contains("script-src 'nonce-"), "CSP: {csp}");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"/#doc=docs%2Fa.md\"") || body.contains("#doc="),
            "{body}"
        );
        assert!(body.contains("\"sectok\""), "{body}");
        assert!(!body.contains("?token="), "{body}");

        let req = axum::http::Request::builder()
            .uri(format!("/docs/open/{ticket}"))
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let req = axum::http::Request::builder()
            .uri("/docs/open/not-a-real-ticket-id")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // TC-756: docs_routes_require_auth (NFR-751)
    #[tokio::test]
    async fn docs_routes_require_auth() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let token = "valid-token";
        let service =
            test_docs_service(ws, false, crate::docs::changes::ChangeLog::default(), None);
        let router = test_router(Some(service), token);

        // 1. Missing token -> 401
        let req = axum::http::Request::builder()
            .uri("/api/docs")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 2. Foreign Origin -> 403
        let req = axum::http::Request::builder()
            .uri("/api/docs")
            .header("Host", "localhost:7420")
            .header("Origin", "http://evil.com")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // 3. Correct token and origin -> 200
        let req = axum::http::Request::builder()
            .uri("/api/docs")
            .header("Host", "localhost:7420")
            .header("Origin", "http://localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 4. Missing token for /api/docs/tree -> 401 (TC-832)
        let req_tree = axum::http::Request::builder()
            .uri("/api/docs/tree")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp_tree = router.clone().oneshot(req_tree).await.unwrap();
        assert_eq!(resp_tree.status(), StatusCode::UNAUTHORIZED);

        // 5. Asset routes require no token -> 200
        let req_js = axum::http::Request::builder()
            .uri("/assets/docs.js")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp_js = router.clone().oneshot(req_js).await.unwrap();
        assert_eq!(resp_js.status(), StatusCode::OK);

        let req_css = axum::http::Request::builder()
            .uri("/assets/docs.css")
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp_css = router.clone().oneshot(req_css).await.unwrap();
        assert_eq!(resp_css.status(), StatusCode::OK);
    }

    // TC-748 (NFR-751): a ticket stops working when its file is swapped for a
    // symlink that escapes the workspace after issue.
    #[cfg(unix)]
    #[tokio::test]
    async fn view_ticket_rejects_symlink_swap() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.html"), "<p>secret</p>").unwrap();
        std::fs::write(dir.path().join("page.html"), "<p>ok</p>").unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        let token = "testtoken";
        let service =
            test_docs_service(ws, false, crate::docs::changes::ChangeLog::default(), None);
        let router = test_router(Some(service), token);
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/docs/view")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({ "path": "page.html" }).to_string(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let view_url = val["url"].as_str().unwrap().to_string();

        std::fs::remove_file(dir.path().join("page.html")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.html"),
            dir.path().join("page.html"),
        )
        .unwrap();
        let req = axum::http::Request::builder()
            .uri(&view_url)
            .header("Host", "localhost:7420")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // TC-831: tree_api_confined_and_bounded (SPEC-docs-tree FR-830, FR-831)
    #[tokio::test]
    async fn tree_api_confined_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        for ignored in crate::docs::IGNORED_DIRS {
            std::fs::create_dir_all(dir.path().join(ignored).join("nested")).unwrap();
            std::fs::write(dir.path().join(ignored).join("nested/x.md"), "# x").unwrap();
        }
        std::fs::create_dir_all(dir.path().join("docs/sub")).unwrap();
        std::fs::write(dir.path().join("docs/a.md"), "# a").unwrap();
        std::fs::write(dir.path().join("docs/sub/b.rs"), "fn b() {}").unwrap();
        std::fs::write(dir.path().join("top.txt"), "top").unwrap();

        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret.md"), "secret").unwrap();
            std::fs::write(outside.path().join("outside.md"), "outside").unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("linkdir")).unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("outside.md"),
                dir.path().join("link_out.md"),
            )
            .unwrap();
            std::os::unix::fs::symlink(dir.path().join("docs/a.md"), dir.path().join("link_in.md"))
                .unwrap();
        }

        let ws = Workspace::new(dir.path()).unwrap();
        let token = "testtoken";
        let service = test_docs_service(
            ws.clone(),
            false,
            crate::docs::changes::ChangeLog::default(),
            None,
        );
        let router = test_router(Some(service), token);

        let make_req = |uri: &str| {
            axum::http::Request::builder()
                .uri(uri)
                .header("Host", "localhost:7420")
                .header("Origin", "http://localhost:7420")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };

        // 1. 200 for root
        let resp = router
            .clone()
            .oneshot(make_req("/api/docs/tree"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            resp.into_body().collect(),
        )
        .await
        .unwrap()
        .unwrap()
        .to_bytes();
        let tree: crate::docs::list::TreeList = serde_json::from_slice(&body).unwrap();
        assert_eq!(tree.dir, "");
        assert!(!tree.truncated);
        let names: Vec<&str> = tree.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"docs"));
        assert!(names.contains(&"top.txt"));
        #[cfg(unix)]
        {
            assert!(!names.contains(&"linkdir"));
            assert!(!names.contains(&"link_out.md"));
            assert!(names.contains(&"link_in.md"));
        }

        // 200 for root with ?dir=
        let resp = router
            .clone()
            .oneshot(make_req("/api/docs/tree?dir="))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 2. 200 for subdir
        let resp = router
            .clone()
            .oneshot(make_req("/api/docs/tree?dir=docs"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            resp.into_body().collect(),
        )
        .await
        .unwrap()
        .unwrap()
        .to_bytes();
        let tree: crate::docs::list::TreeList = serde_json::from_slice(&body).unwrap();
        assert_eq!(tree.dir, "docs");
        assert_eq!(tree.entries.len(), 2);
        assert_eq!(tree.entries[0].name, "sub");
        assert_eq!(tree.entries[1].name, "a.md");

        // 3. 403 for ../, absolute and escaping symlink
        let resp = router
            .clone()
            .oneshot(make_req("/api/docs/tree?dir=../outside"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let resp = router
            .clone()
            .oneshot(make_req("/api/docs/tree?dir=/etc"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        #[cfg(unix)]
        {
            let resp = router
                .clone()
                .oneshot(make_req("/api/docs/tree?dir=link_out.md"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN);

            let resp = router
                .clone()
                .oneshot(make_req("/api/docs/tree?dir=linkdir"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        }

        // 4. 404 for missing and ignored
        let resp = router
            .clone()
            .oneshot(make_req("/api/docs/tree?dir=missing"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = router
            .clone()
            .oneshot(make_req("/api/docs/tree?dir=target"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 5. 400 for a file
        let resp = router
            .clone()
            .oneshot(make_req("/api/docs/tree?dir=top.txt"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp = router
            .clone()
            .oneshot(make_req("/api/docs/tree?dir=docs/a.md"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // 6. list_children with a cap of 3 sets truncated
        std::fs::write(dir.path().join("c.txt"), "c").unwrap();
        std::fs::write(dir.path().join("d.txt"), "d").unwrap();
        let bounded = crate::docs::list::list_children(&ws, "", 3).unwrap();
        assert!(bounded.truncated);
        assert_eq!(bounded.entries.len(), 3);
    }
}
