//! Read-only runs API (`GET /api/runs*`, SPEC-headless-runs-dashboard FR-2806..FR-2809).

use std::convert::Infallible;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use regex::Regex;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::graph::GraphRun;
use crate::run_journal::{
    JournalStore, RunManifest, mask_json_value, mask_sensitive_tokens, valid_run_id,
};
use crate::web::server::{AppState, Ws, error_json, json_ok};
use crate::web::snapshot;

static GRAPH_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,128}$").unwrap());

static RUNNER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]{1,64}$").unwrap());

/// Upper bound on bytes read from the progress log per stream tick.
const PROGRESS_READ_CAP: u64 = 1024 * 1024;

/// Upper bound on bytes read from a graph snapshot file.
const GRAPH_READ_CAP: u64 = 4 * 1024 * 1024;

/// Opens a regular file without following symlinks. Returns `None` when the
/// path is a symlink, not a regular file, missing, or unreadable.
pub(crate) fn open_regular_nofollow(path: &Path) -> Option<fs::File> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_file() => return None,
        Ok(_) => {}
        Err(_) => return None,
    }
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .ok()?
    };
    #[cfg(not(unix))]
    let file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    // On non-unix the pre-open symlink check plus post-open metadata is the
    // strongest available guard; on unix O_NOFOLLOW already closed the race.
    #[cfg(not(unix))]
    {
        if fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
        {
            return None;
        }
    }
    Some(file)
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

pub(crate) fn parse_runs_limit(raw: Option<&str>) -> Result<usize, ()> {
    match raw {
        None | Some("") => Ok(50),
        Some(value) => {
            let parsed = value.parse::<usize>().map_err(|_| ())?;
            if parsed == 0 || parsed > 200 {
                Err(())
            } else {
                Ok(parsed)
            }
        }
    }
}

pub(crate) fn is_run_active(status: &str) -> bool {
    status == "active" || status == "running"
}

#[cfg(target_os = "linux")]
fn proc_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(target_os = "linux"))]
fn proc_alive(_pid: u32) -> bool {
    true
}

fn run_stale(manifest: &RunManifest) -> bool {
    if !is_run_active(&manifest.status) {
        return false;
    }
    let Some(pid) = manifest.pid else {
        return false;
    };
    #[cfg(target_os = "linux")]
    {
        !proc_alive(pid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

fn progress_mtime_ms(store: &JournalStore, id: &str) -> Option<u64> {
    let path = store.root().join(id).join("progress.jsonl");
    let meta = fs::metadata(&path).ok()?;
    meta.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn run_stalled(store: &JournalStore, id: &str, status: &str) -> bool {
    if !is_run_active(status) {
        return false;
    }
    let Some(mtime_ms) = progress_mtime_ms(store, id) else {
        return false;
    };
    let now = snapshot::now_ms();
    now.saturating_sub(mtime_ms) > 90_000
}

fn elapsed_ms(manifest: &RunManifest) -> u64 {
    let now = snapshot::now_ms();
    if is_run_active(&manifest.status) {
        return now.saturating_sub(manifest.created_at_ms);
    }
    manifest.duration_ms.unwrap_or_else(|| {
        manifest
            .completed_at_ms
            .unwrap_or(now)
            .saturating_sub(manifest.created_at_ms)
    })
}

#[derive(Serialize)]
struct RunItem {
    run_id: String,
    origin: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_runner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_runner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_model: Option<String>,
    created_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at_ms: Option<u64>,
    elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    dmuxai_exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    graph_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approved_by_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    stale: bool,
    stalled: bool,
}

fn mask_opt_string(value: &Option<String>) -> Option<String> {
    value.as_ref().map(|s| mask_sensitive_tokens(s))
}

/// FR-2816: the run id that superseded an `awaiting_approval` run by
/// executing its approved plan; recorded in `end_summary.approved_by_run_id`.
fn approved_by_run_id(manifest: &RunManifest) -> Option<String> {
    manifest
        .end_summary
        .as_ref()
        .and_then(|summary| summary.get("approved_by_run_id"))
        .and_then(|value| value.as_str())
        .map(mask_sensitive_tokens)
}

fn run_item_from_manifest(store: &JournalStore, manifest: &RunManifest) -> RunItem {
    RunItem {
        run_id: manifest.run_id.clone(),
        origin: mask_sensitive_tokens(&manifest.origin),
        status: manifest.status.clone(),
        requested_runner: mask_opt_string(&manifest.requested_runner),
        resolved_runner: mask_opt_string(&manifest.resolved_runner),
        requested_model: mask_opt_string(&manifest.requested_model),
        resolved_model: mask_opt_string(&manifest.resolved_model),
        created_at_ms: manifest.created_at_ms,
        completed_at_ms: manifest.completed_at_ms,
        elapsed_ms: elapsed_ms(manifest),
        dmuxai_exit_code: manifest.dmuxai_exit_code,
        error_kind: mask_opt_string(&manifest.error_kind),
        error_summary: mask_opt_string(&manifest.error_summary),
        graph_id: manifest
            .links
            .graph_id
            .as_ref()
            .map(|s| mask_sensitive_tokens(s)),
        approved_by_run_id: approved_by_run_id(manifest),
        pid: manifest.pid,
        stale: run_stale(manifest),
        stalled: run_stalled(store, &manifest.run_id, &manifest.status),
    }
}

pub(crate) fn list_manifests(store: &JournalStore) -> Vec<RunManifest> {
    let root = store.root();
    if !root.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut manifests = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if crate::run_journal::is_symlink(&path).unwrap_or(true) {
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !valid_run_id(name) {
            continue;
        }
        if let Ok(manifest) = store.load(name) {
            manifests.push(manifest);
        }
    }
    manifests.sort_by_key(|a| std::cmp::Reverse(a.created_at_ms));
    manifests
}

fn manifest_file_meta(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

fn load_graph_view(workspace_root: &Path, graph_id: &str) -> Option<serde_json::Value> {
    if !GRAPH_ID_RE.is_match(graph_id) {
        return None;
    }
    let graphs_dir = workspace_root.join(".dmuxai").join("graphs");
    match fs::symlink_metadata(&graphs_dir) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_dir() => return None,
        Ok(_) => {}
        Err(_) => return None,
    }
    let path = graphs_dir.join(format!("{graph_id}.json"));
    let mut file = open_regular_nofollow(&path)?;
    let mut bytes = Vec::new();
    if (&mut file)
        .take(GRAPH_READ_CAP)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return None;
    }
    let mut run: GraphRun = serde_json::from_slice(&bytes).ok()?;
    run.normalize_legacy_runners();
    let mut value = serde_json::to_value(snapshot::snapshot_from_run(&run)).ok()?;
    mask_json_value(&mut value);
    Some(value)
}

#[derive(Serialize)]
struct ApprovalPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_hash: Option<String>,
    hint: String,
}

fn approval_for(manifest: &RunManifest) -> Option<ApprovalPayload> {
    if manifest.status != "awaiting_approval" {
        return None;
    }
    let plan_hash = manifest
        .end_summary
        .as_ref()
        .and_then(|v| v.get("plan_hash"))
        .and_then(|v| v.as_str())
        .map(mask_sensitive_tokens);
    let has_graph = manifest
        .links
        .graph_id
        .as_deref()
        .is_some_and(|graph_id| !graph_id.is_empty());
    let valid_graph_hash = has_graph
        && plan_hash
            .as_deref()
            .is_some_and(crate::state_path::valid_pending_plan_hash);
    let hint = if valid_graph_hash {
        format!(
            "dmuxai run --graph --approve-plan-id {} -",
            plan_hash.as_deref().unwrap_or_default()
        )
    } else {
        let runner = manifest
            .requested_runner
            .as_deref()
            .or(manifest.resolved_runner.as_deref())
            .unwrap_or("<agent>");
        let runner = mask_sensitive_tokens(runner);
        let runner = if RUNNER_RE.is_match(&runner) {
            runner
        } else {
            "<agent>".to_owned()
        };
        format!("dmuxai run --agent {runner} --approve-plan -")
    };
    Some(ApprovalPayload { plan_hash, hint })
}

fn manifest_to_json(manifest: &RunManifest) -> serde_json::Value {
    let mut value = serde_json::to_value(manifest).unwrap_or(serde_json::json!({}));
    mask_json_value(&mut value);
    value
}

fn progress_as_json_values(store: &JournalStore, id: &str) -> Vec<serde_json::Value> {
    let Ok(lines) = store.load_progress_tail(id, 200) else {
        return Vec::new();
    };
    lines
        .into_iter()
        .filter_map(|line| {
            let masked = mask_sensitive_tokens(&line);
            let mut value = serde_json::from_str::<serde_json::Value>(&masked).ok()?;
            mask_json_value(&mut value);
            Some(value)
        })
        .collect()
}

pub(crate) fn list_response(store: &JournalStore, limit: usize, token: &str) -> Response {
    let runs: Vec<RunItem> = list_manifests(store)
        .into_iter()
        .take(limit)
        .map(|manifest| run_item_from_manifest(store, &manifest))
        .collect();
    let mut payload = serde_json::json!({ "runs": runs });
    redact_token_in_value(&mut payload, token);
    json_ok(StatusCode::OK, &payload)
}

pub(crate) async fn list(
    State(state): State<AppState>,
    Ws(ws): Ws,
    Query(params): Query<RunsListQuery>,
) -> Response {
    let limit = match parse_runs_limit(params.limit.as_deref()) {
        Ok(limit) => limit,
        Err(()) => return error_json(StatusCode::BAD_REQUEST, "invalid limit"),
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
    let manifest = match store.load(id) {
        Ok(m) => m,
        Err(_) => return error_json(StatusCode::NOT_FOUND, "run not found"),
    };
    let run = run_item_from_manifest(store, &manifest);
    let graph = workspace_root.and_then(|root| {
        manifest
            .links
            .graph_id
            .as_deref()
            .and_then(|gid| load_graph_view(root, gid))
    });
    let approval = approval_for(&manifest);
    let progress = progress_as_json_values(store, id);
    let mut payload = serde_json::json!({
        "run": run,
        "manifest": manifest_to_json(&manifest),
        "graph": graph,
        "approval": approval,
        "progress": progress,
    });
    redact_token_in_value(&mut payload, token);
    json_ok(StatusCode::OK, &payload)
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
