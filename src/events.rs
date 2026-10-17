//! Append-only state-change event log (SPEC-agent-api, FR-764/FR-768/FR-769).
//!
//! Events are one JSON line each in `<workspace>/.dmuxai/events.jsonl`, written
//! with a cross-process lock so two DmuxAI processes sharing a workspace never
//! reuse a `seq`. `StateTracker` turns dashboard snapshots into the small set of
//! state transitions the spec defines, and `StallDetector` reports a busy runner
//! that has produced no output for `stall_after`.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::graph::{GraphNodeStatus, GraphRunPhase};
use crate::todo::TodoStatus;
use crate::web::{DashboardSnapshot, NodeView};

/// Schema version written into every record.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// One persisted event line (SPEC-agent-api §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    pub v: u32,
    pub seq: u64,
    pub ts: String,
    pub ts_ms: u64,
    pub workspace: String,
    pub kind: String,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub todo: Option<String>,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub runner: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub from_state: Option<String>,
    pub to_state: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// An event before it is assigned a `seq` and written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventDraft {
    pub kind: String,
    pub task: Option<String>,
    pub todo: Option<String>,
    pub node: Option<String>,
    pub runner: Option<String>,
    pub model: Option<String>,
    pub from_state: Option<String>,
    pub to_state: String,
    pub message: Option<String>,
}

/// `<workspace_root>/.dmuxai/events.jsonl`.
pub fn events_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".dmuxai").join("events.jsonl")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm), so
/// the crate does not need `chrono` just to stamp events.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Formats epoch milliseconds as `2026-10-17T11:22:33.456Z`.
pub fn format_ts(ms: u64) -> String {
    let secs = ms / 1000;
    let millis = ms % 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Highest valid `seq` in the event log file, or `0` when missing or empty.
pub fn last_event_seq(path: &Path) -> u64 {
    last_valid_seq(path)
}

/// Reads events with `seq > since`, oldest first, skipping malformed lines and
/// stopping after `limit` records.
pub fn read_since(path: &Path, since: u64, limit: Option<usize>) -> Vec<EventRecord> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return Vec::new(),
    };
    let mut events = Vec::new();
    for line in content.lines() {
        if limit.is_some_and(|limit| events.len() >= limit) {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<EventRecord>(line) else {
            continue;
        };
        if record.seq <= since {
            continue;
        }
        events.push(record);
    }
    events
}

/// Maximum tail window read when looking for the last `seq`, so a large log is
/// not scanned from the start on every append.
const TAIL_WINDOW: u64 = 64 * 1024;

/// Highest valid `seq` in `content`, or `None` when no line parses.
fn max_seq_in_str(content: &str) -> Option<u64> {
    let mut last: Option<u64> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<EventRecord>(line) {
            last = Some(match last {
                Some(current) => current.max(record.seq),
                None => record.seq,
            });
        }
    }
    last
}

/// Full scan of `path`, best effort (0 when unreadable). Only used when the tail
/// window holds no parseable line.
fn full_scan_last_seq(path: &Path) -> u64 {
    let Ok(bytes) = std::fs::read(path) else {
        return 0;
    };
    max_seq_in_str(&String::from_utf8_lossy(&bytes)).unwrap_or(0)
}

/// Last valid `seq` in `path`, best effort (0 when unreadable/empty). Reads only
/// the final 64 KiB unless that window contains no complete valid line, in which
/// case it falls back to a single full scan.
fn last_valid_seq(path: &Path) -> u64 {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return 0;
    };
    let Ok(len) = file.metadata().map(|metadata| metadata.len()) else {
        return 0;
    };
    if len <= TAIL_WINDOW {
        return full_scan_last_seq(path);
    }
    if file.seek(SeekFrom::Start(len - TAIL_WINDOW)).is_err() {
        return full_scan_last_seq(path);
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return full_scan_last_seq(path);
    }
    let content = String::from_utf8_lossy(&bytes);
    // The window starts mid-file, so everything up to and including the first
    // newline is a partial line and must be dropped before parsing.
    let Some(newline) = content.find('\n') else {
        return full_scan_last_seq(path);
    };
    match max_seq_in_str(&content[newline + 1..]) {
        Some(seq) => seq,
        None => full_scan_last_seq(path),
    }
}

/// Removes the lock file even if the append fails.
struct LockGuard {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Close first so Windows can delete the file.
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

const LOCK_STALE: Duration = Duration::from_secs(5);
const LOCK_TIMEOUT: Duration = Duration::from_millis(500);
const LOCK_RETRY: Duration = Duration::from_millis(5);

fn acquire_lock(lock_path: &Path) -> std::io::Result<LockGuard> {
    let start = Instant::now();
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(file) => {
                return Ok(LockGuard {
                    path: lock_path.to_path_buf(),
                    file: Some(file),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Ok(metadata) = std::fs::metadata(lock_path)
                    && let Ok(modified) = metadata.modified()
                    && modified.elapsed().is_ok_and(|age| age > LOCK_STALE)
                {
                    let _ = std::fs::remove_file(lock_path);
                    continue;
                }
                if start.elapsed() >= LOCK_TIMEOUT {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "timed out acquiring the events lock",
                    ));
                }
                std::thread::sleep(LOCK_RETRY);
            }
            Err(error) => return Err(error),
        }
    }
}

/// Append-only event log over a workspace file. Cheap to clone; handles share
/// nothing but the path, and every append takes the cross-process lock.
#[derive(Clone)]
pub struct EventLog {
    path: Arc<PathBuf>,
    workspace: Arc<String>,
    last_seq: Arc<AtomicU64>,
}

impl EventLog {
    /// Never fails; the `.dmuxai` directory is created lazily on first append.
    pub fn open(workspace_root: &Path, workspace_label: String) -> Self {
        let path = events_path(workspace_root);
        let last = last_valid_seq(&path);
        Self {
            path: Arc::new(path),
            workspace: Arc::new(workspace_label),
            last_seq: Arc::new(AtomicU64::new(last)),
        }
    }

    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq.load(Ordering::SeqCst)
    }

    /// Appends one JSON line per draft, assigning consecutive `seq` values
    /// under the cross-process lock.
    pub fn append(&self, drafts: Vec<EventDraft>) -> std::io::Result<Vec<EventRecord>> {
        if drafts.is_empty() {
            return Ok(Vec::new());
        }
        let dir = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("events path has no parent"))?;
        std::fs::create_dir_all(dir)?;

        let guard = acquire_lock(&dir.join("events.lock"))?;

        // Re-read under the lock so two processes never pick the same seq.
        let mut seq = last_valid_seq(&self.path);
        let now = now_ms();
        let ts = format_ts(now);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.as_path())?;
        let mut records = Vec::with_capacity(drafts.len());
        for draft in drafts {
            seq += 1;
            let record = EventRecord {
                v: EVENT_SCHEMA_VERSION,
                seq,
                ts: ts.clone(),
                ts_ms: now,
                workspace: self.workspace.as_ref().clone(),
                kind: draft.kind,
                task: draft.task,
                todo: draft.todo,
                node: draft.node,
                runner: draft.runner,
                model: draft.model,
                from_state: draft.from_state,
                to_state: draft.to_state,
                message: draft.message,
            };
            let line = serde_json::to_string(&record).map_err(std::io::Error::other)?;
            writeln!(file, "{line}")?;
            records.push(record);
        }
        drop(file);
        drop(guard);
        self.last_seq.store(seq, Ordering::SeqCst);
        Ok(records)
    }
}

fn node_status_str(status: GraphNodeStatus) -> &'static str {
    match status {
        GraphNodeStatus::Pending => "pending",
        GraphNodeStatus::Ready => "ready",
        GraphNodeStatus::Running => "running",
        GraphNodeStatus::WaitingGate => "waiting_gate",
        GraphNodeStatus::Completed => "completed",
        GraphNodeStatus::Failed => "failed",
        GraphNodeStatus::Skipped => "skipped",
    }
}

fn phase_str(phase: GraphRunPhase) -> &'static str {
    match phase {
        GraphRunPhase::Active => "active",
        GraphRunPhase::WaitingGate => "waiting_gate",
        GraphRunPhase::Completed => "completed",
        GraphRunPhase::Failed => "failed",
    }
}

fn todo_status_str(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Open => "open",
        TodoStatus::Running => "running",
        TodoStatus::Done => "done",
    }
}

fn node_draft(
    graph_id: &str,
    node: &NodeView,
    from_state: Option<String>,
    to_state: String,
) -> EventDraft {
    EventDraft {
        kind: "node".to_string(),
        task: Some(graph_id.to_string()),
        todo: None,
        node: Some(node.id.clone()),
        runner: Some(node.runner.clone()),
        model: Some(node.model_id.clone()),
        from_state,
        to_state,
        message: None,
    }
}

fn todo_draft(
    todo_id: &str,
    from_state: String,
    to_state: &str,
    message: Option<String>,
) -> EventDraft {
    EventDraft {
        kind: "todo".to_string(),
        todo: Some(todo_id.to_string()),
        from_state: Some(from_state),
        to_state: to_state.to_string(),
        message,
        ..EventDraft::default()
    }
}

/// Maps a to-do status change to the §3 vocabulary.
fn todo_transition(previous: &str, next: &str, last_error: &Option<String>) -> Option<String> {
    match (previous, next) {
        (_, "running") => Some("started".to_string()),
        ("running", "done") => Some("completed".to_string()),
        ("running", "open") => Some(if last_error.is_some() {
            "failed".to_string()
        } else {
            "open".to_string()
        }),
        (_, "done") => Some("completed".to_string()),
        (_, "open") => Some("open".to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct NodeState {
    status: String,
}

#[derive(Debug, Clone)]
struct TodoState {
    status: String,
}

/// Diffs successive dashboard snapshots into state-change drafts. The first
/// `observe` is a silent baseline.
#[derive(Debug, Default)]
pub struct StateTracker {
    initialized: bool,
    graph_id: Option<String>,
    graph_phase: Option<String>,
    nodes: HashMap<String, NodeState>,
    todos: HashMap<String, TodoState>,
}

impl StateTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn baseline(&mut self, snapshot: &DashboardSnapshot) {
        self.graph_id = snapshot.graph.as_ref().map(|graph| graph.id.clone());
        self.graph_phase = snapshot
            .graph
            .as_ref()
            .map(|graph| phase_str(graph.phase).to_string());
        self.nodes = snapshot
            .graph
            .as_ref()
            .map(|graph| {
                graph
                    .nodes
                    .iter()
                    .map(|node| {
                        (
                            node.id.clone(),
                            NodeState {
                                status: node_status_str(node.status).to_string(),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.todos = snapshot
            .todos
            .iter()
            .map(|todo| {
                (
                    todo.id.clone(),
                    TodoState {
                        status: todo_status_str(todo.status).to_string(),
                    },
                )
            })
            .collect();
    }

    pub fn observe(&mut self, snapshot: &DashboardSnapshot) -> Vec<EventDraft> {
        if !self.initialized {
            self.initialized = true;
            self.baseline(snapshot);
            return Vec::new();
        }

        let mut drafts = Vec::new();

        if let Some(graph) = &snapshot.graph {
            let phase = phase_str(graph.phase).to_string();
            if self.graph_id.as_deref() != Some(graph.id.as_str()) {
                // Only a *different* graph id resets node state. A graph that
                // merely disappears keeps its state, so the same graph
                // reappearing later emits nothing new.
                self.nodes.clear();
                drafts.push(EventDraft {
                    kind: "graph".to_string(),
                    task: Some(graph.id.clone()),
                    to_state: phase.clone(),
                    ..EventDraft::default()
                });
                for node in &graph.nodes {
                    let status = node_status_str(node.status);
                    // First sight in a new graph: only non-pending nodes are news.
                    if status != "pending" {
                        drafts.push(node_draft(&graph.id, node, None, status.to_string()));
                    }
                    self.nodes.insert(
                        node.id.clone(),
                        NodeState {
                            status: status.to_string(),
                        },
                    );
                }
                self.graph_id = Some(graph.id.clone());
                self.graph_phase = Some(phase);
            } else {
                if self.graph_phase.as_deref() != Some(phase.as_str()) {
                    drafts.push(EventDraft {
                        kind: "graph".to_string(),
                        task: Some(graph.id.clone()),
                        from_state: self.graph_phase.clone(),
                        to_state: phase.clone(),
                        ..EventDraft::default()
                    });
                    self.graph_phase = Some(phase);
                }
                for node in &graph.nodes {
                    let status = node_status_str(node.status).to_string();
                    match self.nodes.get(&node.id) {
                        Some(previous) if previous.status == status => {}
                        Some(previous) => {
                            drafts.push(node_draft(
                                &graph.id,
                                node,
                                Some(previous.status.clone()),
                                status.clone(),
                            ));
                            self.nodes.insert(node.id.clone(), NodeState { status });
                        }
                        None => {
                            if status != "pending" {
                                drafts.push(node_draft(&graph.id, node, None, status.clone()));
                            }
                            self.nodes.insert(node.id.clone(), NodeState { status });
                        }
                    }
                }
            }
        }
        // `snapshot.graph` is `None`: keep the previous graph's tracker state so
        // a transient disappearance does not re-emit everything on reappearance.

        for todo in &snapshot.todos {
            let status = todo_status_str(todo.status).to_string();
            match self.todos.get(&todo.id) {
                None => {
                    // New to-dos are only news if they start running.
                    if status == "running" {
                        drafts.push(EventDraft {
                            kind: "todo".to_string(),
                            todo: Some(todo.id.clone()),
                            to_state: "started".to_string(),
                            ..EventDraft::default()
                        });
                    }
                    self.todos.insert(todo.id.clone(), TodoState { status });
                }
                Some(previous) => {
                    if previous.status != status {
                        if let Some(to_state) =
                            todo_transition(&previous.status, &status, &todo.last_error)
                        {
                            let message = (to_state == "failed")
                                .then(|| todo.last_error.clone())
                                .flatten();
                            drafts.push(todo_draft(
                                &todo.id,
                                previous.status.clone(),
                                &to_state,
                                message,
                            ));
                        }
                        self.todos.insert(todo.id.clone(), TodoState { status });
                    }
                }
            }
        }

        drafts
    }
}

#[derive(Debug, Clone)]
struct RunnerStall {
    busy: bool,
    last_activity: Instant,
    model: Option<String>,
    emitted: bool,
}

/// Per-runner stall timer: one `stalled` draft per busy span. A zero `after`
/// disables detection entirely.
pub struct StallDetector {
    after: Duration,
    runners: HashMap<String, RunnerStall>,
}

impl StallDetector {
    pub fn new(after: Duration) -> Self {
        Self {
            after,
            runners: HashMap::new(),
        }
    }

    pub fn busy(&mut self, runner: &str, model: Option<String>, now: Instant) {
        if self.after.is_zero() {
            return;
        }
        let entry = self
            .runners
            .entry(runner.to_string())
            .or_insert(RunnerStall {
                busy: true,
                last_activity: now,
                model: model.clone(),
                emitted: false,
            });
        if !entry.busy {
            entry.busy = true;
            entry.last_activity = now;
            entry.emitted = false;
        }
        entry.model = model;
    }

    pub fn activity(&mut self, runner: &str, now: Instant) {
        if let Some(entry) = self.runners.get_mut(runner) {
            entry.last_activity = now;
            entry.emitted = false;
        }
    }

    pub fn idle(&mut self, runner: &str) {
        if let Some(entry) = self.runners.get_mut(runner) {
            entry.busy = false;
            entry.emitted = false;
        }
    }

    pub fn tick(&mut self, now: Instant) -> Vec<EventDraft> {
        let mut drafts = Vec::new();
        if self.after.is_zero() {
            return drafts;
        }
        for (runner, entry) in &mut self.runners {
            if entry.busy
                && !entry.emitted
                && now.saturating_duration_since(entry.last_activity) >= self.after
            {
                drafts.push(EventDraft {
                    kind: "agent".to_string(),
                    runner: Some(runner.clone()),
                    model: entry.model.clone(),
                    from_state: Some("running".to_string()),
                    to_state: "stalled".to_string(),
                    message: Some(format!("no runner output for {}s", self.after.as_secs())),
                    ..EventDraft::default()
                });
                entry.emitted = true;
            }
        }
        drafts
    }
}

/// Best-effort record of a failed append, written next to the event log. Never
/// touches stdout/stderr and swallows its own errors.
fn log_append_failure(events_path: &Path, error: &std::io::Error) {
    let Some(dir) = events_path.parent() else {
        return;
    };
    let _ = std::fs::create_dir_all(dir);
    let line = format!(
        "{} events append failed: {:?}",
        format_ts(now_ms()),
        error.kind()
    );
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("notify.log"))
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Fan-out for events: persistent log, live broadcast, optional notify hook and
/// the stall detector. A write failure is swallowed; runners are never blocked.
pub struct EventSink {
    log: EventLog,
    broadcaster: tokio::sync::broadcast::Sender<EventRecord>,
    notifier: Option<crate::notify::Notifier>,
    tracker: Mutex<StateTracker>,
    stall: Mutex<StallDetector>,
}

impl EventSink {
    pub fn new(
        log: EventLog,
        stall_after: Duration,
        notifier: Option<crate::notify::Notifier>,
    ) -> Arc<Self> {
        let (broadcaster, _) = tokio::sync::broadcast::channel(256);
        Arc::new(Self {
            log,
            broadcaster,
            notifier,
            tracker: Mutex::new(StateTracker::new()),
            stall: Mutex::new(StallDetector::new(stall_after)),
        })
    }

    pub fn log(&self) -> &EventLog {
        &self.log
    }

    pub fn emit(&self, drafts: Vec<EventDraft>) {
        if drafts.is_empty() {
            return;
        }
        let records = match self.log.append(drafts) {
            Ok(records) => records,
            Err(error) => {
                log_append_failure(self.log.path(), &error);
                return;
            }
        };
        for record in &records {
            let _ = self.broadcaster.send(record.clone());
            if let Some(notifier) = &self.notifier {
                notifier.notify(record);
            }
            self.feed_stall(record);
        }
    }

    fn feed_stall(&self, record: &EventRecord) {
        if record.kind != "agent" {
            return;
        }
        let Some(runner) = record.runner.as_deref() else {
            return;
        };
        let now = Instant::now();
        let mut stall = self.stall.lock().unwrap_or_else(|p| p.into_inner());
        match record.to_state.as_str() {
            "pending" | "started" => stall.busy(runner, record.model.clone(), now),
            "running" => stall.activity(runner, now),
            "completed" | "error" | "cancelled" => stall.idle(runner),
            _ => {}
        }
    }

    pub fn observe_snapshot(&self, snapshot: &DashboardSnapshot) {
        let drafts = self
            .tracker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .observe(snapshot);
        self.emit(drafts);
    }

    pub fn note_activity(&self, runner: &str) {
        self.stall
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .activity(runner, Instant::now());
    }

    pub fn tick(&self) {
        let drafts = self
            .stall
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .tick(Instant::now());
        self.emit(drafts);
    }

    /// Starts a 5 s background tick holding only a `Weak` reference. Each tick
    /// runs its lock-acquiring append on a blocking thread so a Tokio worker is
    /// never held up (FR-768). A no-op outside a Tokio runtime.
    pub fn spawn_ticker(self: &Arc<Self>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = Arc::downgrade(self);
        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                let Some(sink) = weak.upgrade() else {
                    break;
                };
                let _ = tokio::task::spawn_blocking(move || sink.tick()).await;
            }
        });
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EventRecord> {
        self.broadcaster.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::NodeRole;
    use crate::todo::{Todo, TodoSource};
    use crate::web::{GraphCounts, SNAPSHOT_SCHEMA, SnapshotSource};

    fn draft(kind: &str, to_state: &str) -> EventDraft {
        EventDraft {
            kind: kind.to_string(),
            to_state: to_state.to_string(),
            ..EventDraft::default()
        }
    }

    fn snapshot_with(graph: Option<crate::web::GraphView>, todos: Vec<Todo>) -> DashboardSnapshot {
        DashboardSnapshot {
            schema: SNAPSHOT_SCHEMA.to_string(),
            revision: 0,
            generated_at_ms: 0,
            sent_at_ms: 0,
            source: SnapshotSource::Tui,
            workspace: "/ws".to_string(),
            ui: None,
            agents: Vec::new(),
            graph,
            todos,
            jira_base_url: None,
            session: None,
        }
    }

    fn graph(id: &str, phase: GraphRunPhase, status: GraphNodeStatus) -> crate::web::GraphView {
        crate::web::GraphView {
            id: id.to_string(),
            phase,
            orchestrator_model: "orch".to_string(),
            user_prompt: "prompt".to_string(),
            current_nodes: Vec::new(),
            waiting_gate_node: None,
            counts: GraphCounts {
                pending: 0,
                ready: 0,
                running: 0,
                waiting_gate: 0,
                completed: 0,
                failed: 0,
                skipped: 0,
            },
            nodes: vec![NodeView {
                id: "n1".to_string(),
                role: NodeRole::Implement,
                job: "job".to_string(),
                runner: "cursor".to_string(),
                model_id: "model-1".to_string(),
                status,
                attempt: 0,
                started_at_ms: None,
                finished_at_ms: None,
                elapsed_ms: None,
                deps: Vec::new(),
                layout: None,
                detail: crate::web::NodeDetail::default(),
            }],
            edges: Vec::new(),
            layers: 0,
        }
    }

    fn todo(id: &str, status: TodoStatus, last_error: Option<&str>) -> Todo {
        Todo {
            id: id.to_string(),
            title: "task".to_string(),
            body: String::new(),
            status,
            source: TodoSource::Manual,
            source_ref: None,
            external_key: None,
            link_url: None,
            external_status: None,
            external_assignee: None,
            run_kind: None,
            run_ref: None,
            last_error: last_error.map(str::to_string),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    // TC-765 / FR-764 / NFR-760: read_since skips malformed lines, filters by
    // seq and honors the limit (first `limit` records, oldest first).
    #[test]
    fn read_since_skips_malformed_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::open(dir.path(), "ws".to_string());
        let written = log
            .append(vec![
                draft("agent", "started"),
                draft("node", "running"),
                draft("todo", "completed"),
            ])
            .unwrap();
        assert_eq!(written.len(), 3);

        let path = events_path(dir.path());
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{{ not json").unwrap();
        drop(file);

        let filtered = read_since(&path, 1, None);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].seq, 2);
        assert_eq!(filtered[1].seq, 3);

        let limited = read_since(&path, 0, Some(2));
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].seq, 1);
        assert_eq!(limited[1].seq, 2);

        assert!(read_since(&path, 0, Some(0)).is_empty());
    }

    #[test]
    fn format_ts_matches_the_schema_example() {
        assert_eq!(format_ts(1_792_236_153_456), "2026-10-17T11:22:33.456Z");
        assert_eq!(format_ts(0), "1970-01-01T00:00:00.000Z");
    }

    // TC-769 / FR-768: one event per transition, nothing for repeats; the first
    // observe is a silent baseline.
    #[test]
    fn tracker_emits_one_event_per_transition() {
        let mut tracker = StateTracker::new();
        let base = snapshot_with(
            Some(graph("g1", GraphRunPhase::Active, GraphNodeStatus::Ready)),
            vec![todo("t1", TodoStatus::Open, None)],
        );

        assert!(
            tracker.observe(&base).is_empty(),
            "first observe is baseline"
        );
        assert!(
            tracker.observe(&base).is_empty(),
            "identical snapshot emits nothing"
        );

        // Node ready -> running.
        let node_running = snapshot_with(
            Some(graph("g1", GraphRunPhase::Active, GraphNodeStatus::Running)),
            vec![todo("t1", TodoStatus::Open, None)],
        );
        let drafts = tracker.observe(&node_running);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].kind, "node");
        assert_eq!(drafts[0].task.as_deref(), Some("g1"));
        assert_eq!(drafts[0].node.as_deref(), Some("n1"));
        assert_eq!(drafts[0].runner.as_deref(), Some("cursor"));
        assert_eq!(drafts[0].model.as_deref(), Some("model-1"));
        assert_eq!(drafts[0].from_state.as_deref(), Some("ready"));
        assert_eq!(drafts[0].to_state, "running");

        // Graph phase active -> waiting_gate.
        let phase_waiting = snapshot_with(
            Some(graph(
                "g1",
                GraphRunPhase::WaitingGate,
                GraphNodeStatus::Running,
            )),
            vec![todo("t1", TodoStatus::Open, None)],
        );
        let drafts = tracker.observe(&phase_waiting);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].kind, "graph");
        assert_eq!(drafts[0].from_state.as_deref(), Some("active"));
        assert_eq!(drafts[0].to_state, "waiting_gate");

        // To-do open -> running => started.
        let todo_running = snapshot_with(
            Some(graph(
                "g1",
                GraphRunPhase::WaitingGate,
                GraphNodeStatus::Running,
            )),
            vec![todo("t1", TodoStatus::Running, None)],
        );
        let drafts = tracker.observe(&todo_running);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].kind, "todo");
        assert_eq!(drafts[0].todo.as_deref(), Some("t1"));
        assert_eq!(drafts[0].to_state, "started");

        // To-do running -> done => completed.
        let todo_done = snapshot_with(
            Some(graph(
                "g1",
                GraphRunPhase::WaitingGate,
                GraphNodeStatus::Running,
            )),
            vec![todo("t1", TodoStatus::Done, None)],
        );
        let drafts = tracker.observe(&todo_done);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].to_state, "completed");
        assert!(drafts[0].message.is_none());

        // New graph: one graph event plus one node event for the non-pending node.
        let new_graph = snapshot_with(
            Some(graph("g2", GraphRunPhase::Active, GraphNodeStatus::Running)),
            vec![todo("t1", TodoStatus::Done, None)],
        );
        let drafts = tracker.observe(&new_graph);
        assert!(
            drafts
                .iter()
                .any(|d| d.kind == "graph" && d.task.as_deref() == Some("g2"))
        );
        assert!(drafts.iter().any(|d| d.kind == "node"
            && d.task.as_deref() == Some("g2")
            && d.to_state == "running"
            && d.from_state.is_none()));

        // A new graph whose only node is pending emits no node event.
        let pending_graph = snapshot_with(
            Some(graph("g3", GraphRunPhase::Active, GraphNodeStatus::Pending)),
            vec![todo("t1", TodoStatus::Done, None)],
        );
        let drafts = tracker.observe(&pending_graph);
        assert!(drafts.iter().any(|d| d.kind == "graph"));
        assert!(drafts.iter().all(|d| d.kind != "node"));

        // New to-do that is already running emits `started`.
        let new_running_todo = snapshot_with(
            Some(graph("g3", GraphRunPhase::Active, GraphNodeStatus::Pending)),
            vec![
                todo("t1", TodoStatus::Done, None),
                todo("t2", TodoStatus::Running, None),
            ],
        );
        let drafts = tracker.observe(&new_running_todo);
        assert!(drafts.iter().any(|d| d.kind == "todo"
            && d.todo.as_deref() == Some("t2")
            && d.to_state == "started"));

        // Running -> open with an error => failed with the message.
        let failed_todo = snapshot_with(
            Some(graph("g3", GraphRunPhase::Active, GraphNodeStatus::Pending)),
            vec![
                todo("t1", TodoStatus::Done, None),
                todo("t2", TodoStatus::Open, Some("boom")),
            ],
        );
        let drafts = tracker.observe(&failed_todo);
        let failed = drafts
            .iter()
            .find(|d| d.kind == "todo" && d.todo.as_deref() == Some("t2"))
            .expect("todo t2 transition");
        assert_eq!(failed.to_state, "failed");
        assert_eq!(failed.message.as_deref(), Some("boom"));
    }

    // TC-770 / FR-768: concurrent appends from two handles on one workspace get
    // strictly increasing, unique seqs and the log resumes after reopen.
    #[test]
    fn event_log_seq_is_monotonic_across_handles() {
        let dir = tempfile::tempdir().unwrap();
        let first = EventLog::open(dir.path(), "ws".to_string());
        let second = EventLog::open(dir.path(), "ws".to_string());

        let mut handles = Vec::new();
        for worker in 0..4 {
            let log = if worker % 2 == 0 {
                first.clone()
            } else {
                second.clone()
            };
            handles.push(std::thread::spawn(move || {
                for _ in 0..25 {
                    log.append(vec![draft("agent", "running")])
                        .expect("events lock: append must succeed under contention");
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let mut seqs: Vec<u64> = read_since(&events_path(dir.path()), 0, None)
            .into_iter()
            .map(|record| record.seq)
            .collect();
        seqs.sort_unstable();
        assert_eq!(seqs, (1..=100).collect::<Vec<u64>>());

        let reopened = EventLog::open(dir.path(), "ws".to_string());
        assert_eq!(reopened.last_seq(), 100);
    }

    // TC-770 / FR-768: last_valid_seq reads only the final 64 KiB on large logs,
    // and a truncated last line still yields the previous seq.
    #[test]
    fn last_seq_reads_only_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = events_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        let total: u64 = 2_000;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        for seq in 1..=total {
            let record = EventRecord {
                v: EVENT_SCHEMA_VERSION,
                seq,
                ts: "2026-10-17T11:22:33.456Z".to_string(),
                ts_ms: 1_792_236_153_456,
                workspace: "ws".to_string(),
                kind: "agent".to_string(),
                task: None,
                todo: None,
                node: None,
                runner: Some("cursor".to_string()),
                model: None,
                from_state: None,
                to_state: "running".to_string(),
                message: Some("a".repeat(64)),
            };
            writeln!(file, "{}", serde_json::to_string(&record).unwrap()).unwrap();
        }
        drop(file);

        assert!(
            std::fs::metadata(&path).unwrap().len() > TAIL_WINDOW,
            "fixture must exceed the tail window"
        );
        assert_eq!(last_valid_seq(&path), total);

        // A truncated trailing line (no newline, invalid JSON) is skipped and
        // the previous seq is returned.
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "{{\"v\":1,\"seq\":").unwrap();
        drop(file);

        assert_eq!(last_valid_seq(&path), total);
        assert_eq!(
            EventLog::open(dir.path(), "ws".to_string()).last_seq(),
            total
        );
    }

    // TC-771 / FR-769: one stalled event per busy span, reset by activity; a
    // zero threshold disables detection.
    #[test]
    fn stall_detector_emits_once_and_resets() {
        let after = Duration::from_secs(10);
        let mut detector = StallDetector::new(after);
        let start = Instant::now();
        detector.busy("cursor", Some("model-1".to_string()), start);

        assert!(detector.tick(start + Duration::from_secs(5)).is_empty());

        let drafts = detector.tick(start + Duration::from_secs(11));
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].kind, "agent");
        assert_eq!(drafts[0].runner.as_deref(), Some("cursor"));
        assert_eq!(drafts[0].model.as_deref(), Some("model-1"));
        assert_eq!(drafts[0].from_state.as_deref(), Some("running"));
        assert_eq!(drafts[0].to_state, "stalled");
        assert_eq!(
            drafts[0].message.as_deref(),
            Some("no runner output for 10s")
        );

        // No duplicate for the same busy span.
        assert!(detector.tick(start + Duration::from_secs(12)).is_empty());

        // Activity re-arms the timer.
        detector.activity("cursor", start + Duration::from_secs(13));
        assert!(detector.tick(start + Duration::from_secs(20)).is_empty());
        let drafts = detector.tick(start + Duration::from_secs(24));
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].to_state, "stalled");

        // Zero disables the detector.
        let mut disabled = StallDetector::new(Duration::ZERO);
        disabled.busy("cursor", None, start);
        assert!(disabled.tick(start + Duration::from_secs(999)).is_empty());
    }
}
