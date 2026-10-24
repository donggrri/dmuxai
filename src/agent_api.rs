//! Shared helpers for the agent-facing surfaces: headless CLI, MCP server and
//! HTTP API (SPEC-agent-api FR-760..FR-767).

use std::path::Path;

use serde_json::{Value, json};

use crate::settings::DmuxSettings;
use crate::todo::{NewTodo, Todo, TodoError, TodoSource, TodoStatus, TodoStore};

/// Stable process exit codes for the headless CLI (FR-761, FR-762).
pub mod exit {
    pub const OK: i32 = 0;
    pub const FAILED: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const NOT_FOUND: i32 = 3;
    pub const CONFLICT: i32 = 4;
    pub const FORBIDDEN: i32 = 5;
    pub const INTERNAL: i32 = 6;
    /// Plan approval or sensitive gate required in headless mode (FR-947, FR-944).
    pub const APPROVAL_REQUIRED: i32 = 7;
    pub const CANCELLED: i32 = 130;
}

/// An error that carries the exit code it maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub code: i32,
    pub message: String,
}

impl ApiError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "ok": false, "exit_code": self.code, "error": self.message })
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<TodoError> for ApiError {
    fn from(error: TodoError) -> Self {
        match error {
            TodoError::NotFound => Self::new(exit::NOT_FOUND, "to-do not found"),
            TodoError::Invalid(message) => Self::new(exit::USAGE, message),
            TodoError::Storage(message) => Self::new(exit::INTERNAL, message),
        }
    }
}

/// Opens the per-repo to-do store (same database as the TUI, FR-540).
pub fn open_todo_store(workspace_root: &Path) -> Result<TodoStore, ApiError> {
    let db_path = crate::state_path::repo_db_path(workspace_root)
        .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?;
    }
    TodoStore::open(&db_path).map_err(ApiError::from)
}

/// Parses a `--status` filter value.
pub fn parse_todo_status(value: &str) -> Result<TodoStatus, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "open" => Ok(TodoStatus::Open),
        "running" => Ok(TodoStatus::Running),
        "done" => Ok(TodoStatus::Done),
        other => Err(ApiError::new(
            exit::USAGE,
            format!("invalid status '{other}' (expected open, running or done)"),
        )),
    }
}

/// `todo add` (FR-762): creates a manual to-do.
pub fn todo_add(store: &TodoStore, title: &str, body: &str) -> Result<Todo, ApiError> {
    if title.trim().is_empty() {
        return Err(ApiError::new(exit::USAGE, "title is required"));
    }
    store
        .create(NewTodo {
            title: title.trim().to_owned(),
            body: body.to_owned(),
            source: TodoSource::Manual,
            source_ref: None,
        })
        .map_err(ApiError::from)
}

/// `todo list` (FR-762): all to-dos, optionally filtered by status.
pub fn todo_list(store: &TodoStore, status: Option<TodoStatus>) -> Result<Vec<Todo>, ApiError> {
    let todos = store.list().map_err(ApiError::from)?;
    Ok(todos
        .into_iter()
        .filter(|todo| status.is_none_or(|s| todo.status == s))
        .collect())
}

/// Runner ids known to this workspace: built-ins plus `[[agents.custom]]`.
pub fn runner_ids(settings: &DmuxSettings) -> Vec<(String, String)> {
    let mut ids = vec![
        ("cursor".to_owned(), "Cursor".to_owned()),
        ("agy".to_owned(), "AGY".to_owned()),
        ("codex".to_owned(), "Codex".to_owned()),
    ];
    for spec in &settings.custom_agents {
        ids.push((spec.name.clone(), spec.label().to_owned()));
    }
    ids
}

/// Whether `runner` may run under `[graph] allowed_runners` (NFR-762, FR-813).
pub fn runner_allowed(settings: &DmuxSettings, runner: &str) -> bool {
    settings.graph_runner_allowed(runner)
}

/// `dmuxai status --json` (FR-763).
pub fn status_json(workspace_root: &Path) -> Value {
    let settings = DmuxSettings::load(workspace_root);
    let mut warnings: Vec<String> = settings.config_warnings.clone();
    let workspace = crate::workspace::normalize_workspace_path(workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());

    let mut last_agent_run: std::collections::HashMap<String, Value> =
        std::collections::HashMap::new();
    let events_path = crate::events::events_path(workspace_root);
    if let Ok(records) = std::fs::read_to_string(&events_path) {
        for line in records.lines() {
            if let Ok(rec) = serde_json::from_str::<crate::events::EventRecord>(line)
                && let Some(runner) = rec.runner
            {
                if rec.kind == "activity" {
                    if let Some(prev) = last_agent_run.get_mut(&runner)
                        && prev.get("state").and_then(|s| s.as_str()) == Some("running")
                    {
                        prev["last_output_at_ms"] = json!(rec.ts_ms);
                    }
                    continue;
                }
                if rec.kind != "agent" {
                    continue;
                }
                let state = match rec.to_state.as_str() {
                    "started" | "running" | "pending" | "stalled" => "running",
                    "completed" => "done",
                    "error" => "failed",
                    "cancelled" => "cancelled",
                    _ => "idle",
                };
                let mut run_val = json!({ "state": state });
                let prev_run = last_agent_run.get(&runner);
                let prev_running = prev_run
                    .filter(|prev| prev.get("state").and_then(|s| s.as_str()) == Some("running"));
                if state == "running" {
                    let prev_start =
                        prev_running.and_then(|prev| prev.get("started_at_ms").cloned());
                    let started_at = prev_start.unwrap_or_else(|| json!(rec.ts_ms));
                    run_val["started_at_ms"] = started_at.clone();

                    let prev_output =
                        prev_running.and_then(|prev| prev.get("last_output_at_ms").cloned());
                    let last_output = match rec.to_state.as_str() {
                        "stalled" | "pending" => prev_output.unwrap_or(started_at),
                        _ => json!(rec.ts_ms),
                    };
                    run_val["last_output_at_ms"] = last_output;
                } else if state != "idle" {
                    // done / failed / cancelled carry ended_at and, when the
                    // run's start is known, its duration (FR-820).
                    run_val["ended_at_ms"] = json!(rec.ts_ms);
                    if let Some(start) = prev_running
                        .and_then(|prev| prev.get("started_at_ms"))
                        .and_then(|v| v.as_u64())
                    {
                        run_val["started_at_ms"] = json!(start);
                        run_val["duration_ms"] = json!(rec.ts_ms.saturating_sub(start));
                    }
                    if let Some(last_output) =
                        prev_running.and_then(|prev| prev.get("last_output_at_ms").cloned())
                    {
                        run_val["last_output_at_ms"] = last_output;
                    }
                }
                if let Some(node) = rec.node {
                    run_val["node"] = json!(node);
                } else if let Some(prev_node) =
                    prev_running.and_then(|prev| prev.get("node").cloned())
                {
                    run_val["node"] = prev_node;
                }
                last_agent_run.insert(runner, run_val);
            }
        }
    }

    let agents: Vec<Value> = runner_ids(&settings)
        .into_iter()
        .map(|(id, label)| {
            let allowed = runner_allowed(&settings, &id);
            let run = last_agent_run
                .get(&id)
                .cloned()
                .unwrap_or_else(|| json!({ "state": "idle" }));
            json!({ "id": id, "label": label, "allowed": allowed, "run": run })
        })
        .collect();

    let todos = match open_todo_store(workspace_root).and_then(|store| todo_list(&store, None)) {
        Ok(list) => {
            let count = |s: TodoStatus| list.iter().filter(|t| t.status == s).count();
            json!({
                "open": count(TodoStatus::Open),
                "running": count(TodoStatus::Running),
                "done": count(TodoStatus::Done),
            })
        }
        Err(error) => {
            warnings.push(format!("to-dos unavailable: {error}"));
            Value::Null
        }
    };

    let graphs_dir = workspace_root.join(".dmuxai").join("graphs");
    let last_graph = match crate::web::FileGraphSource::new(workspace_root).poll() {
        Some(graph) => json!({ "id": graph.id, "phase": graph.phase, "counts": graph.counts }),
        None => {
            let has_files = std::fs::read_dir(&graphs_dir)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false);
            if has_files {
                warnings.push("last graph run could not be read".to_owned());
            }
            Value::Null
        }
    };

    let events_path = crate::events::events_path(workspace_root);
    let last_seq = crate::events::EventLog::open(workspace_root, workspace.clone()).last_seq();

    let dashboard = crate::web::discovery::read_discovery(workspace_root)
        .map(|d| {
            json!({
                "base_url": d.base_url,
                "port": d.port,
                "pid": d.pid,
                "token_file": crate::web::discovery::discovery_path(workspace_root).display().to_string(),
            })
        })
        .unwrap_or(Value::Null);

    json!({
        "workspace": workspace,
        "version": env!("CARGO_PKG_VERSION"),
        "agents": agents,
        "allowed_runners": settings.graph_allowed_runners,
        // FR-930: graph engineering is always on; the deprecated setting no longer exists.
        "graph_enabled": true,
        "todos": todos,
        "last_graph": last_graph,
        "events": { "path": events_path.display().to_string(), "last_seq": last_seq },
        "dashboard": dashboard,
        "warnings": warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_todo_status_accepts_known_values() {
        assert_eq!(parse_todo_status("Open").unwrap(), TodoStatus::Open);
        assert_eq!(parse_todo_status("done").unwrap(), TodoStatus::Done);
        assert_eq!(parse_todo_status("x").unwrap_err().code, exit::USAGE);
    }

    #[test]
    fn todo_add_rejects_blank_title_and_lists_by_status() {
        let dir = tempfile::tempdir().unwrap();
        let store = TodoStore::open(&dir.path().join("db.sqlite3")).unwrap();
        assert_eq!(todo_add(&store, "  ", "").unwrap_err().code, exit::USAGE);
        let todo = todo_add(&store, "write docs", "body").unwrap();
        assert_eq!(todo_list(&store, Some(TodoStatus::Open)).unwrap().len(), 1);
        assert!(
            todo_list(&store, Some(TodoStatus::Done))
                .unwrap()
                .is_empty()
        );
        assert_eq!(todo.title, "write docs");
    }

    #[test]
    fn status_json_maps_pending_and_stalled_to_running_preserving_start_time() {
        let dir = tempfile::tempdir().unwrap();
        let dmux_dir = dir.path().join(".dmuxai");
        std::fs::create_dir_all(&dmux_dir).unwrap();
        let events_file = dmux_dir.join("events.jsonl");

        let rec1 = crate::events::EventRecord {
            v: 1,
            seq: 1,
            ts: "2026-04-18T00:00:00Z".to_string(),
            ts_ms: 1000,
            workspace: dir.path().display().to_string(),
            kind: "agent".to_string(),
            task: None,
            todo: None,
            node: Some("A".to_string()),
            runner: Some("cursor".to_string()),
            model: None,
            from_state: Some("idle".to_string()),
            to_state: "started".to_string(),
            message: None,
            error_kind: None,
        };
        let rec2 = crate::events::EventRecord {
            v: 1,
            seq: 2,
            ts: "2026-04-18T00:00:05Z".to_string(),
            ts_ms: 6000,
            workspace: dir.path().display().to_string(),
            kind: "agent".to_string(),
            task: None,
            todo: None,
            node: None,
            runner: Some("cursor".to_string()),
            model: None,
            from_state: Some("running".to_string()),
            to_state: "pending".to_string(),
            message: None,
            error_kind: None,
        };
        let rec3 = crate::events::EventRecord {
            v: 1,
            seq: 3,
            ts: "2026-04-18T00:00:10Z".to_string(),
            ts_ms: 11000,
            workspace: dir.path().display().to_string(),
            kind: "agent".to_string(),
            task: None,
            todo: None,
            node: None,
            runner: Some("cursor".to_string()),
            model: None,
            from_state: Some("running".to_string()),
            to_state: "stalled".to_string(),
            message: None,
            error_kind: None,
        };

        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&rec1).unwrap(),
            serde_json::to_string(&rec2).unwrap(),
            serde_json::to_string(&rec3).unwrap(),
        );
        std::fs::write(&events_file, content).unwrap();

        let val = status_json(dir.path());
        let agents = val.get("agents").and_then(|a| a.as_array()).unwrap();
        let cursor = agents
            .iter()
            .find(|a| a.get("id").and_then(|id| id.as_str()) == Some("cursor"))
            .expect("cursor agent found");
        let run = cursor.get("run").expect("run object found");
        assert_eq!(run.get("state").and_then(|s| s.as_str()), Some("running"));
        assert_eq!(
            run.get("started_at_ms").and_then(|t| t.as_u64()),
            Some(1000)
        );
        assert_eq!(run.get("node").and_then(|n| n.as_str()), Some("A"));

        // The run completes, then a new run starts without a node: it must
        // not inherit node "A" from the previous run.
        let mut rec4 = rec3.clone();
        rec4.seq = 4;
        rec4.ts_ms = 12000;
        rec4.to_state = "completed".to_string();
        let mut rec5 = rec3.clone();
        rec5.seq = 5;
        rec5.ts_ms = 20000;
        rec5.from_state = Some("idle".to_string());
        rec5.to_state = "started".to_string();
        let cursor_run = |val: &Value| {
            val["agents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["id"] == "cursor")
                .map(|a| a["run"].clone())
                .unwrap()
        };
        let mut content = std::fs::read_to_string(&events_file).unwrap();
        content.push_str(&format!("{}\n", serde_json::to_string(&rec4).unwrap()));
        std::fs::write(&events_file, &content).unwrap();
        let done = cursor_run(&status_json(dir.path()));
        assert_eq!(done["state"], "done");
        assert_eq!(done["started_at_ms"].as_u64(), Some(1000));
        assert_eq!(done["ended_at_ms"].as_u64(), Some(12000));
        assert_eq!(done["duration_ms"].as_u64(), Some(11000));

        content.push_str(&format!("{}\n", serde_json::to_string(&rec5).unwrap()));
        std::fs::write(&events_file, content).unwrap();
        let val = status_json(dir.path());
        let run = val["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == "cursor")
            .map(|a| a["run"].clone())
            .unwrap();
        assert_eq!(run["state"], "running");
        assert_eq!(run["started_at_ms"].as_u64(), Some(20000));
        assert!(run.get("node").is_none(), "new run inherited node: {run}");
    }

    // TC-1776 / FR-1776: events.jsonl with started + activity -> running run has
    // last_output_at_ms equal to the activity ts.
    #[test]
    fn tc_1776_status_json_last_output_at() {
        let dir = tempfile::tempdir().unwrap();
        let dmux_dir = dir.path().join(".dmuxai");
        std::fs::create_dir_all(&dmux_dir).unwrap();
        let events_file = dmux_dir.join("events.jsonl");

        let rec_start = crate::events::EventRecord {
            v: 1,
            seq: 1,
            ts: "2026-10-24T00:00:00Z".to_string(),
            ts_ms: 1000,
            workspace: dir.path().display().to_string(),
            kind: "agent".to_string(),
            task: None,
            todo: None,
            node: None,
            runner: Some("cursor".to_string()),
            model: None,
            from_state: Some("idle".to_string()),
            to_state: "started".to_string(),
            message: None,
            error_kind: None,
        };
        let rec_act = crate::events::EventRecord {
            v: 1,
            seq: 2,
            ts: "2026-10-24T00:00:05Z".to_string(),
            ts_ms: 6000,
            workspace: dir.path().display().to_string(),
            kind: "activity".to_string(),
            task: None,
            todo: None,
            node: None,
            runner: Some("cursor".to_string()),
            model: None,
            from_state: None,
            to_state: "tool".to_string(),
            message: Some("running cargo test".to_string()),
            error_kind: None,
        };

        // When only started has arrived, last_output_at_ms equals started_at_ms
        std::fs::write(
            &events_file,
            format!("{}\n", serde_json::to_string(&rec_start).unwrap()),
        )
        .unwrap();
        let val1 = status_json(dir.path());
        let agents1 = val1["agents"].as_array().unwrap();
        let cursor1 = agents1
            .iter()
            .find(|a| a["id"] == "cursor")
            .expect("cursor agent found");
        let run1 = &cursor1["run"];
        assert_eq!(run1["state"], "running");
        assert_eq!(run1["started_at_ms"], 1000);
        assert_eq!(run1["last_output_at_ms"], 1000);

        // When activity arrives, last_output_at_ms updates to the activity timestamp
        let mut content = std::fs::read_to_string(&events_file).unwrap();
        content.push_str(&format!("{}\n", serde_json::to_string(&rec_act).unwrap()));
        std::fs::write(&events_file, &content).unwrap();

        let val2 = status_json(dir.path());
        let agents2 = val2["agents"].as_array().unwrap();
        let cursor2 = agents2
            .iter()
            .find(|a| a["id"] == "cursor")
            .expect("cursor agent found");
        let run2 = &cursor2["run"];
        assert_eq!(run2["state"], "running");
        assert_eq!(run2["started_at_ms"], 1000);
        assert_eq!(run2["last_output_at_ms"], 6000);
    }
}
