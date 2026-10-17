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
                && rec.kind == "agent"
                && let Some(runner) = rec.runner
            {
                let state = match rec.to_state.as_str() {
                    "started" | "running" => "running",
                    "completed" => "done",
                    "error" => "failed",
                    "cancelled" => "cancelled",
                    _ => "idle",
                };
                let mut run_val = json!({ "state": state });
                if state == "running" {
                    run_val["started_at_ms"] = json!(rec.ts_ms);
                }
                if let Some(node) = rec.node {
                    run_val["node"] = json!(node);
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
        "graph_enabled": settings.graph_enabled,
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
}
