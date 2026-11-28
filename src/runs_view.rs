//! Read-only run listing and detail shared by `GET /api/runs*`
//! (SPEC-headless-runs-dashboard FR-2806, FR-2807), `dmuxai runs list|show`
//! and the MCP `runs_list`/`runs_show` tools
//! (SPEC-coordinator-observability FR-3855, FR-3856).

use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::agent_api::{ApiError, exit};
use crate::graph::GraphRun;
use crate::run_journal::{
    JournalError, JournalStore, RunManifest, RunStatus, mask_json_value, mask_sensitive_tokens,
    valid_run_id,
};
use crate::web::snapshot;

pub const RUNS_LIMIT_DEFAULT: usize = 50;
pub const RUNS_LIMIT_MAX: usize = 200;
pub const PROGRESS_DEFAULT: usize = 50;
pub const PROGRESS_MAX: usize = 200;

static GRAPH_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,128}$").unwrap());

static RUNNER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]{1,64}$").unwrap());

/// Upper bound on bytes read from a graph snapshot file.
const GRAPH_READ_CAP: u64 = 4 * 1024 * 1024;

/// Opens a regular file without following symlinks. Returns `None` when the
/// path is a symlink, not a regular file, missing, or unreadable.
pub fn open_regular_nofollow(path: &Path) -> Option<fs::File> {
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

/// Parses a list limit: unset or empty → 50, otherwise 1..=200.
pub fn parse_runs_limit(raw: Option<&str>) -> Option<usize> {
    match raw {
        None | Some("") => Some(RUNS_LIMIT_DEFAULT),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=RUNS_LIMIT_MAX).contains(n)),
    }
}

/// Parses a progress tail length: unset or empty → 50, otherwise 0..=200.
pub fn parse_progress_limit(raw: Option<&str>) -> Option<usize> {
    match raw {
        None | Some("") => Some(PROGRESS_DEFAULT),
        Some(value) => value.parse::<usize>().ok().filter(|n| *n <= PROGRESS_MAX),
    }
}

/// Validates a `--status` filter against the manifest status vocabulary.
pub fn parse_status_filter(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    value
        .parse::<RunStatus>()
        .map(|status| status.as_status_str().to_owned())
        .map_err(|_| {
            format!(
                "invalid run status '{value}' (expected active, running, completed, failed, cancelled, awaiting_approval or approved)"
            )
        })
}

pub fn is_run_active(status: &str) -> bool {
    status == "active" || status == "running"
}

#[cfg(target_os = "linux")]
fn proc_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
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

/// One run in a list (FR-2806); every string is masked.
#[derive(Debug, Clone, Serialize)]
pub struct RunItem {
    pub run_id: String,
    pub origin: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_runner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_runner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
    pub created_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dmuxai_exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub stale: bool,
    pub stalled: bool,
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

pub fn run_item_from_manifest(store: &JournalStore, manifest: &RunManifest) -> RunItem {
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

/// Every loadable manifest under the store root, newest first. A missing
/// root, symlinks, invalid ids and malformed manifests are skipped.
pub fn list_manifests(store: &JournalStore) -> Vec<RunManifest> {
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

/// Runs newest first, keeping only `statuses` when non-empty, at most `limit`.
pub fn list_runs(store: &JournalStore, statuses: &[String], limit: usize) -> Vec<RunItem> {
    list_manifests(store)
        .into_iter()
        .filter(|manifest| statuses.is_empty() || statuses.contains(&manifest.status))
        .take(limit)
        .map(|manifest| run_item_from_manifest(store, &manifest))
        .collect()
}

/// `{"runs":[...]}` as returned by `GET /api/runs` (FR-2806).
pub fn list_json(store: &JournalStore, statuses: &[String], limit: usize) -> Value {
    json!({ "runs": list_runs(store, statuses, limit) })
}

fn load_graph_view(workspace_root: &Path, graph_id: &str) -> Option<Value> {
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

fn manifest_to_json(manifest: &RunManifest) -> Value {
    let mut value = serde_json::to_value(manifest).unwrap_or(json!({}));
    mask_json_value(&mut value);
    value
}

fn progress_as_json_values(store: &JournalStore, id: &str, max_lines: usize) -> Vec<Value> {
    if max_lines == 0 {
        return Vec::new();
    }
    let Ok(lines) = store.load_progress_tail(id, max_lines) else {
        return Vec::new();
    };
    lines
        .into_iter()
        .filter_map(|line| {
            let masked = mask_sensitive_tokens(&line);
            let mut value = serde_json::from_str::<Value>(&masked).ok()?;
            mask_json_value(&mut value);
            Some(value)
        })
        .collect()
}

/// Why a run detail could not be read.
#[derive(Debug)]
pub enum RunLookupError {
    InvalidId,
    NotFound,
    Journal(JournalError),
}

impl RunLookupError {
    pub fn to_api_error(&self, id: &str) -> ApiError {
        match self {
            RunLookupError::InvalidId => {
                ApiError::new(exit::USAGE, format!("invalid run id '{id}'"))
            }
            RunLookupError::NotFound => {
                ApiError::new(exit::NOT_FOUND, format!("run '{id}' not found"))
            }
            RunLookupError::Journal(error) => ApiError::new(exit::INTERNAL, error.to_string()),
        }
    }
}

/// `{run, manifest, graph, approval, progress}` as returned by
/// `GET /api/runs/{id}` (FR-2807), with the last `progress_lines` progress
/// lines. Read-only; every string is masked.
pub fn run_detail(
    store: &JournalStore,
    workspace_root: Option<&Path>,
    id: &str,
    progress_lines: usize,
) -> Result<Value, RunLookupError> {
    if !valid_run_id(id) {
        return Err(RunLookupError::InvalidId);
    }
    let run_dir = store.root().join(id);
    match fs::symlink_metadata(&run_dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(RunLookupError::Journal(JournalError::SymlinkDetected(
                run_dir,
            )));
        }
        Ok(_) => {}
        Err(_) => return Err(RunLookupError::NotFound),
    }
    let manifest = store.load(id).map_err(RunLookupError::Journal)?;
    let graph = workspace_root.and_then(|root| {
        manifest
            .links
            .graph_id
            .as_deref()
            .and_then(|gid| load_graph_view(root, gid))
    });
    Ok(json!({
        "run": run_item_from_manifest(store, &manifest),
        "manifest": manifest_to_json(&manifest),
        "graph": graph,
        "approval": approval_for(&manifest),
        "progress": progress_as_json_values(store, id, progress_lines),
    }))
}

fn usage(message: impl Into<String>) -> ApiError {
    ApiError::new(exit::USAGE, message.into())
}

fn limit_arg(
    args: &Value,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, ApiError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .map(|n| n as usize)
            .filter(|n| (min..=max).contains(n))
            .ok_or_else(|| usage(format!("{key} must be an integer from {min} to {max}"))),
    }
}

fn status_args(args: &Value) -> Result<Vec<String>, ApiError> {
    let raw: Vec<&str> = match args.get("status") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .ok_or_else(|| usage("status must be a string or an array of strings"))
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(usage("status must be a string or an array of strings")),
    };
    raw.into_iter()
        .map(|s| parse_status_filter(s).map_err(usage))
        .collect()
}

/// MCP `runs_list` / `runs_show` against an explicit journal store
/// (FR-3855, FR-3856). Errors carry `exit_code` like the CLI.
pub fn mcp_invoke_in(
    store: &JournalStore,
    workspace_root: Option<&Path>,
    name: &str,
    args: &Value,
) -> Value {
    let result = match name {
        "runs_list" => status_args(args).and_then(|statuses| {
            let limit = limit_arg(args, "limit", RUNS_LIMIT_DEFAULT, 1, RUNS_LIMIT_MAX)?;
            Ok(list_json(store, &statuses, limit))
        }),
        "runs_show" => {
            let id = args
                .get("run_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            limit_arg(args, "progress", PROGRESS_DEFAULT, 0, PROGRESS_MAX).and_then(|progress| {
                if id.is_empty() {
                    return Err(usage("run_id is required"));
                }
                run_detail(store, workspace_root, &id, progress).map_err(|e| e.to_api_error(&id))
            })
        }
        other => Err(usage(format!("unknown tool: {other}"))),
    };
    match result {
        Ok(mut value) => {
            value["ok"] = Value::Bool(true);
            value
        }
        Err(error) => error.to_json(),
    }
}

/// MCP `runs_list` / `runs_show` for the workspace's run journal.
pub fn mcp_invoke(workspace_root: &Path, name: &str, args: &Value) -> Value {
    match crate::run_journal::repo_runs_dir(workspace_root) {
        Ok(dir) => mcp_invoke_in(&JournalStore::new(dir), Some(workspace_root), name, args),
        Err(error) => ApiError::new(exit::INTERNAL, error.to_string()).to_json(),
    }
}
