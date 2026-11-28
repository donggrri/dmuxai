//! Read-only runs API (`GET /api/runs*`, SPEC-headless-runs-dashboard FR-2806..FR-2809).

use std::convert::Infallible;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::run_journal::{JournalStore, mask_json_value, mask_sensitive_tokens, valid_run_id};
use crate::runs_view::{RunLookupError, run_item_from_manifest};
pub(crate) use crate::runs_view::{
    is_run_active, list_manifests, open_regular_nofollow, parse_runs_limit,
};
use crate::web::server::{AppState, Ws, error_json, json_ok};

/// Upper bound on bytes read from the progress log per stream tick.
const PROGRESS_READ_CAP: u64 = 1024 * 1024;

/// Progress lines returned by `GET /api/runs/{id}` (FR-2807).
const DETAIL_PROGRESS_LINES: usize = 200;

fn manifest_file_meta(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

fn redact_token_in_text(text: &str, token: &str) -> String {
    if token.is_empty() {
        text.to_owned()
    } else {
        text.replace(token, "[REDACTED]")
    }
}

pub(crate) fn redact_token_in_value(value: &mut serde_json::Value, token: &str) {
    if token.is_empty() {
        return;
    }
    match value {
        serde_json::Value::String(text) => {
            if text.contains(token) {
                *text = text.replace(token, "[REDACTED]");
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_token_in_value(item, token);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values_mut() {
                redact_token_in_value(item, token);
            }
        }
        _ => {}
    }
}

#[derive(serde::Deserialize, Default)]
pub(crate) struct RunsListQuery {
    pub(crate) limit: Option<String>,
}

fn workspace_root_from_events(path: &Path) -> Option<PathBuf> {
    let dmuxai = path.parent()?;
    if dmuxai.file_name().and_then(|n| n.to_str()) != Some(".dmuxai") {
        return None;
    }
    dmuxai.parent().map(Path::to_path_buf)
}

fn journal_for(ws: &crate::web::directory::WorkspaceEntry) -> Option<(PathBuf, JournalStore)> {
    let events_path = ws.services.events_path.as_ref()?;
    let root = workspace_root_from_events(events_path)?;
    let dir = crate::run_journal::repo_runs_dir(&root).ok()?;
    Some((root, JournalStore::new(dir)))
}

pub(crate) fn list_response(store: &JournalStore, limit: usize, token: &str) -> Response {
    let mut payload = crate::runs_view::list_json(store, &[], limit);
    redact_token_in_value(&mut payload, token);
    json_ok(StatusCode::OK, &payload)
}

pub(crate) async fn list(
    State(state): State<AppState>,
    Ws(ws): Ws,
    Query(params): Query<RunsListQuery>,
) -> Response {
    let Some(limit) = parse_runs_limit(params.limit.as_deref()) else {
        return error_json(StatusCode::BAD_REQUEST, "invalid limit");
    };
    let Some((_, store)) = journal_for(&ws) else {
        return json_ok(StatusCode::OK, &serde_json::json!({ "runs": [] }));
    };
    list_response(&store, limit, state.token())
}

pub(crate) fn detail_response(
    store: &JournalStore,
    workspace_root: Option<&Path>,
    id: &str,
    token: &str,
) -> Response {
    match crate::runs_view::run_detail(store, workspace_root, id, DETAIL_PROGRESS_LINES) {
        Ok(mut payload) => {
            redact_token_in_value(&mut payload, token);
            json_ok(StatusCode::OK, &payload)
        }
        Err(RunLookupError::InvalidId) => error_json(StatusCode::BAD_REQUEST, "invalid run id"),
        Err(RunLookupError::NotFound | RunLookupError::Journal(_)) => {
            error_json(StatusCode::NOT_FOUND, "run not found")
        }
    }
}

pub(crate) async fn detail(
    State(state): State<AppState>,
    Ws(ws): Ws,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !valid_run_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid run id");
    }
    let Some((workspace_root, store)) = journal_for(&ws) else {
        return error_json(StatusCode::NOT_FOUND, "run not found");
    };
    detail_response(&store, Some(&workspace_root), &id, state.token())
}

fn masked_event_data(record: &crate::events::EventRecord, token: &str) -> String {
    let mut value = serde_json::to_value(record).unwrap_or(serde_json::json!({}));
    mask_json_value(&mut value);
    redact_token_in_value(&mut value, token);
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned())
}

/// Reads at most [`PROGRESS_READ_CAP`] bytes of complete progress lines after
/// `offset`. The file must be a regular file reached without following a
/// symlink; on any open failure `offset` is left unchanged. A single line
/// longer than the cap is skipped rather than buffered without limit.
fn tail_text_lines(path: &Path, offset: &mut u64, max_lines: usize) -> Vec<String> {
    let Some(mut file) = open_regular_nofollow(path) else {
        return Vec::new();
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let start = if len < *offset { 0 } else { *offset };
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let capacity = (len - start).min(PROGRESS_READ_CAP);
    let mut bytes = Vec::with_capacity(capacity as usize);
    if (&mut file).take(capacity).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut consumed = 0u64;
    let mut line_start = 0usize;
    while let Some(relative_newline) = bytes[line_start..].iter().position(|&b| b == b'\n') {
        if lines.len() >= max_lines {
            break;
        }
        let line_end = line_start + relative_newline;
        let line = String::from_utf8_lossy(&bytes[line_start..line_end]);
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let normalized = crate::run_journal::normalize_progress_line(trimmed);
            lines.push(mask_sensitive_tokens(&normalized));
        }
        line_start = line_end + 1;
        consumed = line_start as u64;
    }
    let has_newline = bytes.contains(&b'\n');
    if max_lines > 0
        && consumed == 0
        && !has_newline
        && bytes.len() as u64 == capacity
        && capacity == PROGRESS_READ_CAP
        && capacity < len - start
    {
        // No complete line fits in the cap: skip past the oversized line so the
        // next tick can continue after it.
        let mut skip_pos = start + capacity;
        loop {
            let mut chunk = [0u8; 8192];
            match file.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(relative_newline) = chunk[..n].iter().position(|&b| b == b'\n') {
                        skip_pos += relative_newline as u64 + 1;
                        break;
                    }
                    skip_pos += n as u64;
                }
                Err(_) => break,
            }
        }
        *offset = skip_pos;
    } else {
        *offset = start + consumed;
    }
    lines
}

fn is_terminal_status(status: &str) -> bool {
    !is_run_active(status)
}

pub(crate) async fn stream(
    State(state): State<AppState>,
    Ws(ws): Ws,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !valid_run_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid run id");
    }
    let Some((_, store)) = journal_for(&ws) else {
        return error_json(StatusCode::NOT_FOUND, "run not found");
    };
    stream_response(
        &store,
        ws.services.events_path.clone(),
        id,
        state.token().to_owned(),
    )
}

pub(crate) fn stream_response(
    store: &JournalStore,
    events_path: Option<PathBuf>,
    run_id: String,
    token: String,
) -> Response {
    if store.load(&run_id).is_err() {
        return error_json(StatusCode::NOT_FOUND, "run not found");
    }

    let store_root = store.root().to_path_buf();

    let (tx, rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();

    tokio::spawn(async move {
        let store = JournalStore::new(store_root);
        let manifest_path = store.root().join(&run_id).join("manifest.json");
        let progress_path = store.root().join(&run_id).join("progress.jsonl");

        let send_run_snapshot = |tx: &mpsc::UnboundedSender<Result<Event, Infallible>>,
                                 store: &JournalStore|
         -> Option<String> {
            let manifest = store.load(&run_id).ok()?;
            let item = run_item_from_manifest(store, &manifest);
            let mut value = serde_json::to_value(&item).unwrap_or(serde_json::json!({}));
            redact_token_in_value(&mut value, &token);
            let json = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned());
            let _ = tx.send(Ok(Event::default().event("run").data(json)));
            Some(manifest.status)
        };

        let _ = send_run_snapshot(&tx, &store);
        let mut manifest_meta = manifest_file_meta(&manifest_path);
        let mut progress_offset = 0u64;
        let mut events_offset = 0u64;
        let mut events_seq = 0u64;
        let mut interval = interval(Duration::from_millis(500));
        interval.tick().await;

        loop {
            if tx.is_closed() {
                return;
            }

            let mut had_new_data = false;

            if let Ok(manifest) = store.load(&run_id) {
                let current_meta = manifest_file_meta(&manifest_path);
                if current_meta != manifest_meta {
                    manifest_meta = current_meta;
                    if send_run_snapshot(&tx, &store).is_some() {
                        had_new_data = true;
                    }
                }

                for line in tail_text_lines(&progress_path, &mut progress_offset, 500) {
                    had_new_data = true;
                    let line =
                        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&line) {
                            mask_json_value(&mut value);
                            redact_token_in_value(&mut value, &token);
                            serde_json::to_string(&value).unwrap_or_else(|_| {
                                redact_token_in_text(&mask_sensitive_tokens(&line), &token)
                            })
                        } else {
                            redact_token_in_text(&mask_sensitive_tokens(&line), &token)
                        };
                    if tx
                        .send(Ok(Event::default().event("progress").data(line)))
                        .is_err()
                    {
                        return;
                    }
                }

                if let Some(path) = events_path.as_ref() {
                    let (records, next) =
                        crate::events::read_since_from(path, events_offset, events_seq, None);
                    events_offset = next;
                    for record in records {
                        events_seq = events_seq.max(record.seq);
                        if record.run_id.as_deref() != Some(run_id.as_str()) {
                            continue;
                        }
                        had_new_data = true;
                        let data = masked_event_data(&record, &token);
                        if tx
                            .send(Ok(Event::default()
                                .event("state-change")
                                .id(record.seq.to_string())
                                .data(data)))
                            .is_err()
                        {
                            return;
                        }
                    }
                }

                if is_terminal_status(&manifest.status) && !had_new_data {
                    let _ = tx.send(Ok(Event::default().event("end").data("")));
                    return;
                }
            } else {
                return;
            }

            interval.tick().await;
        }
    });

    Sse::new(UnboundedReceiverStream::new(rx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}
