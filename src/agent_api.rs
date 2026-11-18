//! Shared helpers for the agent-facing surfaces: headless CLI, MCP server and
//! HTTP API (SPEC-agent-api FR-760..FR-767).

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::custom_agent::CustomAgentSpec;
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

/// The stable `error_kind` string of a headless exit code (FR-2140).
///
/// A busy database (or any other conflict) is reported as `"conflict"`, and an
/// internal error as `"internal"`, so a machine caller can tell the two apart
/// without parsing the message. Other exit codes carry no error kind.
pub fn error_kind_for_exit(code: i32) -> Option<&'static str> {
    match code {
        exit::CONFLICT => Some("conflict"),
        exit::INTERNAL => Some("internal"),
        _ => None,
    }
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
        json!({
            "ok": false,
            "exit_code": self.code,
            "error": self.message,
            "error_kind": error_kind_for_exit(self.code),
        })
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

/// Headless run admission: whether `runner` may run under `[graph] allowed_runners`
/// (FR-2838). With the list unset, every registered runner is admitted (true).
/// With the list set, membership decides admission (empty list = false).
pub fn runner_run_admitted(settings: &DmuxSettings, runner: &str) -> bool {
    match &settings.graph_allowed_runners {
        None => true,
        Some(allowed) => allowed
            .iter()
            .any(|r| r.trim().eq_ignore_ascii_case(runner.trim())),
    }
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
            let allowed = runner_run_admitted(&settings, &id);
            let graph_allowed = settings.graph_runner_allowed(&id);
            let run = last_agent_run
                .get(&id)
                .cloned()
                .unwrap_or_else(|| json!({ "state": "idle" }));
            json!({ "id": id, "label": label, "allowed": allowed, "graph_allowed": graph_allowed, "run": run })
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

fn runner_installed(id: &str, spec: Option<&CustomAgentSpec>) -> Option<bool> {
    if id.eq_ignore_ascii_case("cursor") {
        if let Some(bin) = std::env::var_os("DMUXAI_CURSOR_BIN") {
            let text = bin.to_string_lossy();
            if Path::new(&*text).is_absolute() || text.chars().any(std::path::is_separator) {
                Some(crate::settings::is_executable_file(Path::new(&*text)))
            } else {
                Some(crate::settings::find_on_path(&text).is_some())
            }
        } else {
            Some(crate::settings::find_on_path("cursor-agent").is_some())
        }
    } else if id.eq_ignore_ascii_case("agy") {
        if let Some(bin) = std::env::var_os("DMUXAI_AGY_BIN") {
            let text = bin.to_string_lossy();
            if Path::new(&*text).is_absolute() || text.chars().any(std::path::is_separator) {
                Some(crate::settings::is_executable_file(Path::new(&*text)))
            } else {
                Some(crate::settings::find_on_path(&text).is_some())
            }
        } else {
            Some(crate::settings::find_on_path("agy").is_some())
        }
    } else if id.eq_ignore_ascii_case("codex") {
        if let Some(bin) = std::env::var_os("DMUXAI_CODEX_BIN") {
            let text = bin.to_string_lossy();
            if Path::new(&*text).is_absolute() || text.chars().any(std::path::is_separator) {
                Some(crate::settings::is_executable_file(Path::new(&*text)))
            } else {
                Some(crate::settings::find_on_path(&text).is_some())
            }
        } else {
            Some(crate::settings::find_on_path("codex").is_some())
        }
    } else if let Some(spec) = spec {
        if spec.builtin && spec.name.eq_ignore_ascii_case("grok") {
            Some(crate::settings::find_grok_bin().is_some())
        } else if spec.builtin && spec.name.eq_ignore_ascii_case("opencode") {
            Some(crate::settings::find_opencode_bin().is_some())
        } else if let Some(cmd) = spec
            .command
            .first()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            if Path::new(cmd).is_absolute() || cmd.chars().any(std::path::is_separator) {
                Some(crate::settings::is_executable_file(Path::new(cmd)))
            } else {
                Some(crate::settings::find_on_path(cmd).is_some())
            }
        } else {
            None
        }
    } else {
        None
    }
}

/// `dmuxai runners --json` (FR-2831).
pub fn runners_json(settings: &DmuxSettings) -> Value {
    let runners: Vec<Value> = runner_ids(settings)
        .into_iter()
        .map(|(id, label)| {
            let spec = settings
                .custom_agents
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(&id));
            let kind = if id.eq_ignore_ascii_case("cursor")
                || id.eq_ignore_ascii_case("agy")
                || id.eq_ignore_ascii_case("codex")
            {
                "builtin"
            } else if spec.is_some_and(|s| s.builtin) {
                "preset"
            } else {
                "custom"
            };
            let allowed = runner_run_admitted(settings, &id);
            let graph_allowed = settings.graph_runner_allowed(&id);
            let installed = runner_installed(&id, spec);
            let default_model = settings
                .runner_models
                .get(&id)
                .or_else(|| settings.runner_models.get(&id.to_ascii_lowercase()))
                .cloned()
                .or_else(|| spec.and_then(|s| s.default_model.clone()))
                .or_else(|| crate::model_policy::default_model_for_runner(&id).map(str::to_owned));
            let model_policy = if id.eq_ignore_ascii_case("cursor") {
                "any non-fast model (default composer-2.5)".to_owned()
            } else if id.eq_ignore_ascii_case("agy") {
                "gemini-*-flash-*, gemini-4-hydrogen-* or composer-2.5; no fast".to_owned()
            } else if id.eq_ignore_ascii_case("codex") {
                "composer-2.5 only".to_owned()
            } else if id.eq_ignore_ascii_case("grok") {
                "grok-* without fast".to_owned()
            } else if let Some(spec) = spec {
                if let Some(pattern) = &spec.allowed_models {
                    format!("allowed_models = {pattern}")
                } else {
                    "any non-fast model".to_owned()
                }
            } else {
                "any non-fast model".to_owned()
            };

            let mut entry = serde_json::Map::new();
            entry.insert("id".to_owned(), json!(id));
            entry.insert("label".to_owned(), json!(label));
            entry.insert("kind".to_owned(), json!(kind));
            entry.insert("allowed".to_owned(), json!(allowed));
            entry.insert("graph_allowed".to_owned(), json!(graph_allowed));
            entry.insert("installed".to_owned(), json!(installed));
            entry.insert("default_model".to_owned(), json!(default_model));
            entry.insert("model_policy".to_owned(), json!(model_policy));
            Value::Object(entry)
        })
        .collect();

    json!({
        "ok": true,
        "runners": runners,
    })
}

/// Parses output from `agy models` (FR-2833).
pub fn parse_agy_models(stdout: &str) -> Vec<(String, Option<String>)> {
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in stdout.lines() {
        let trimmed_line = line.trim();
        let Some(first_char) = trimmed_line.chars().next() else {
            continue;
        };
        if !first_char.is_ascii_lowercase() && !first_char.is_ascii_digit() {
            continue;
        }

        let (raw_id, raw_label) = if let Some((id, rest)) = trimmed_line.split_once('\t') {
            (id, Some(rest))
        } else if let Some((id, rest)) = trimmed_line.split_once(|c: char| c.is_ascii_whitespace())
        {
            (id, Some(rest))
        } else {
            (trimmed_line, None)
        };

        let id = raw_id.trim();
        if id.is_empty() || id.contains(char::is_whitespace) || !id.contains('-') {
            continue;
        }

        let label = raw_label
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        if seen.insert(id.to_owned()) {
            results.push((id.to_owned(), label));
        }
    }

    results
}

/// A listing result for one runner's models (FR-2832).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelListing {
    pub source: String,
    pub models: Vec<(String, Option<String>)>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
}

/// Pure builder for `dmuxai models --json` (FR-2832).
pub fn models_json(settings: &DmuxSettings, entries: Vec<(String, ModelListing)>) -> Value {
    let runners: Vec<Value> = entries
        .into_iter()
        .map(|(runner_id, listing)| {
            let allowed = runner_run_admitted(settings, &runner_id);
            let mut runner_obj = serde_json::Map::new();
            runner_obj.insert("id".to_owned(), json!(runner_id));
            runner_obj.insert("allowed".to_owned(), json!(allowed));
            runner_obj.insert("source".to_owned(), json!(listing.source));

            let models_arr: Vec<Value> = listing
                .models
                .iter()
                .map(|(id, label)| {
                    let model_allowed =
                        crate::model_policy::validate_model_for_runner(&runner_id, id).is_ok();
                    let mut m = serde_json::Map::new();
                    m.insert("id".to_owned(), json!(id));
                    if let Some(lbl) = label {
                        m.insert("label".to_owned(), json!(lbl));
                    }
                    m.insert("allowed".to_owned(), json!(model_allowed));
                    Value::Object(m)
                })
                .collect();

            runner_obj.insert("models".to_owned(), Value::Array(models_arr));

            if let Some(err) = &listing.error {
                runner_obj.insert("error".to_owned(), json!(err));
            }
            if let Some(kind) = &listing.error_kind {
                runner_obj.insert("error_kind".to_owned(), json!(kind));
            }

            Value::Object(runner_obj)
        })
        .collect();

    json!({
        "ok": true,
        "runners": runners,
    })
}

/// Real model listing collection for a single runner.
pub async fn collect_model_listing(
    workspace_root: &Path,
    runner: &str,
    settings: &DmuxSettings,
) -> ModelListing {
    if runner.eq_ignore_ascii_case("agy") {
        let static_agy = || ModelListing {
            source: "static".to_owned(),
            models: vec![
                ("gemini-3.8-flash-low".to_owned(), None),
                ("gemini-3.8-flash-medium".to_owned(), None),
                ("gemini-3.8-flash-high".to_owned(), None),
                ("gemini-4-hydrogen-low".to_owned(), None),
                ("gemini-4-hydrogen-medium".to_owned(), None),
                ("gemini-4-hydrogen-high".to_owned(), None),
                ("gemini-4-hydrogen-xhigh".to_owned(), None),
            ],
            error: None,
            error_kind: None,
        };

        let program = crate::process::resolve_program("DMUXAI_AGY_BIN", "agy");
        let mut cmd = crate::process::headless_command(program);
        cmd.arg("models");
        crate::process::apply_child_cwd_tokio(&mut cmd, workspace_root);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        match tokio::time::timeout(Duration::from_secs(10), cmd.output()).await {
            Ok(Ok(output)) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let parsed = parse_agy_models(&stdout);
                if parsed.is_empty() {
                    static_agy()
                } else {
                    ModelListing {
                        source: "command".to_owned(),
                        models: parsed,
                        error: None,
                        error_kind: None,
                    }
                }
            }
            _ => static_agy(),
        }
    } else if runner.eq_ignore_ascii_case("cursor") || runner.eq_ignore_ascii_case("codex") {
        ModelListing {
            source: "static".to_owned(),
            models: vec![("composer-2.5".to_owned(), None)],
            error: None,
            error_kind: None,
        }
    } else if let Some(spec) = settings
        .custom_agents
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(runner))
    {
        if !spec.models_command.is_empty() {
            match crate::custom_agent::list_model_ids(
                spec,
                workspace_root,
                Some(Duration::from_secs(10)),
            )
            .await
            {
                Ok(ids) => ModelListing {
                    source: "command".to_owned(),
                    models: ids.into_iter().map(|id| (id, None)).collect(),
                    error: None,
                    error_kind: None,
                },
                Err(err) => {
                    let error_kind = if spec.builtin && spec.name.eq_ignore_ascii_case("grok") {
                        Some("auth".to_owned())
                    } else if spec.builtin {
                        Some(crate::custom_agent::builtin_probe_error_kind(spec, &err).to_owned())
                    } else {
                        None
                    };
                    ModelListing {
                        source: "none".to_owned(),
                        models: vec![],
                        error: Some(err),
                        error_kind,
                    }
                }
            }
        } else if let Some(default_model) = &spec.default_model {
            ModelListing {
                source: "static".to_owned(),
                models: vec![(default_model.clone(), None)],
                error: None,
                error_kind: None,
            }
        } else {
            ModelListing {
                source: "none".to_owned(),
                models: vec![],
                error: None,
                error_kind: None,
            }
        }
    } else {
        ModelListing {
            source: "none".to_owned(),
            models: vec![],
            error: Some(format!("unknown runner: {runner}")),
            error_kind: None,
        }
    }
}

/// Collects models for either all runners or a single target agent (FR-2832).
pub async fn collect_models_json(
    workspace_root: &Path,
    settings: &DmuxSettings,
    target_agent: Option<&str>,
) -> Result<Value, ApiError> {
    let known_runners = runner_ids(settings);
    if let Some(target) = target_agent {
        let matching = known_runners
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(target.trim()));
        let Some((canonical_id, _)) = matching else {
            return Err(ApiError::new(
                exit::NOT_FOUND,
                format!("unknown agent '{target}'"),
            ));
        };
        let listing = collect_model_listing(workspace_root, canonical_id, settings).await;
        Ok(models_json(settings, vec![(canonical_id.clone(), listing)]))
    } else {
        let mut entries = Vec::new();
        for (id, _) in known_runners {
            let listing = collect_model_listing(workspace_root, &id, settings).await;
            entries.push((id, listing));
        }
        Ok(models_json(settings, entries))
    }
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
            run_id: None,
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
            run_id: None,
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
            run_id: None,
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
            run_id: None,
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
            run_id: None,
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

    // TC-2839 / FR-2838: status allowed matches headless run admission, graph_allowed reflects graph policy
    #[test]
    fn tc_2839_status_allowed_matches_headless_run() {
        let mut settings = DmuxSettings::default();
        settings
            .custom_agents
            .push(crate::custom_agent::grok_preset("grok"));
        settings
            .custom_agents
            .push(crate::custom_agent::opencode_preset("opencode"));

        // 1. Unset allowed_runners: allowed = true for all, graph_allowed = false for builtin presets
        assert_eq!(settings.graph_allowed_runners, None);
        for runner in ["cursor", "agy", "codex", "grok", "opencode"] {
            assert!(
                runner_run_admitted(&settings, runner),
                "{runner} should be admitted when allowed_runners is unset"
            );
        }
        assert!(!settings.graph_runner_allowed("grok"));
        assert!(!settings.graph_runner_allowed("opencode"));
        assert!(settings.graph_runner_allowed("cursor"));
        assert!(settings.graph_runner_allowed("agy"));

        // 2. Set allowed_runners: both equal list membership
        settings.graph_allowed_runners = Some(vec!["grok".to_string(), "cursor".to_string()]);
        assert!(runner_run_admitted(&settings, "grok"));
        assert!(runner_run_admitted(&settings, "cursor"));
        assert!(!runner_run_admitted(&settings, "agy"));
        assert!(!runner_run_admitted(&settings, "opencode"));

        assert!(settings.graph_runner_allowed("grok"));
        assert!(settings.graph_runner_allowed("cursor"));
        assert!(!settings.graph_runner_allowed("agy"));
        assert!(!settings.graph_runner_allowed("opencode"));

        // Case insensitivity
        assert!(runner_run_admitted(&settings, "GROK"));
        assert!(runner_run_admitted(&settings, "CURSOR"));

        // 3. Empty allowed_runners: all false
        settings.graph_allowed_runners = Some(vec![]);
        for runner in ["cursor", "agy", "codex", "grok", "opencode"] {
            assert!(
                !runner_run_admitted(&settings, runner),
                "{runner} should be rejected when allowed_runners is empty"
            );
            assert!(
                !settings.graph_runner_allowed(runner),
                "{runner} should be graph-rejected when allowed_runners is empty"
            );
        }

        // 4. status_json on temp workspace with no config, set config, and empty config
        let dir_none = tempfile::tempdir().unwrap();
        let val_none = status_json(dir_none.path());
        let agents_none = val_none["agents"].as_array().expect("agents array");
        for a in agents_none {
            assert_eq!(
                a["allowed"], true,
                "all runners allowed with no config: {a}"
            );
        }
        let cursor_none = agents_none.iter().find(|a| a["id"] == "cursor").unwrap();
        assert_eq!(cursor_none["allowed"], true);
        assert_eq!(cursor_none["graph_allowed"], true);
        let agy_none = agents_none.iter().find(|a| a["id"] == "agy").unwrap();
        assert_eq!(agy_none["allowed"], true);
        assert_eq!(agy_none["graph_allowed"], true);

        let dir_set = tempfile::tempdir().unwrap();
        std::fs::write(
            dir_set.path().join("dmuxai.toml"),
            "[graph]\nallowed_runners = [\"agy\"]\n",
        )
        .unwrap();
        let val_set = status_json(dir_set.path());
        let agents_set = val_set["agents"].as_array().expect("agents array");
        let cursor_set = agents_set.iter().find(|a| a["id"] == "cursor").unwrap();
        assert_eq!(cursor_set["allowed"], false);
        assert_eq!(cursor_set["graph_allowed"], false);
        let agy_set = agents_set.iter().find(|a| a["id"] == "agy").unwrap();
        assert_eq!(agy_set["allowed"], true);
        assert_eq!(agy_set["graph_allowed"], true);

        let dir_empty = tempfile::tempdir().unwrap();
        std::fs::write(
            dir_empty.path().join("dmuxai.toml"),
            "[graph]\nallowed_runners = []\n",
        )
        .unwrap();
        let val_empty = status_json(dir_empty.path());
        let agents_empty = val_empty["agents"].as_array().expect("agents array");
        for a in agents_empty {
            assert_eq!(a["allowed"], false);
            assert_eq!(a["graph_allowed"], false);
        }
        let cursor_empty = agents_empty.iter().find(|a| a["id"] == "cursor").unwrap();
        assert_eq!(cursor_empty["allowed"], false);
        assert_eq!(cursor_empty["graph_allowed"], false);
        let agy_empty = agents_empty.iter().find(|a| a["id"] == "agy").unwrap();
        assert_eq!(agy_empty["allowed"], false);
        assert_eq!(agy_empty["graph_allowed"], false);
    }

    // TC-2831 / FR-2831: runners_json fields and admission with unset/set allowed_runners
    #[test]
    fn tc_2831_runners_json_fields_and_admission() {
        let mut settings = DmuxSettings::default();
        let custom = crate::custom_agent::CustomAgentSpec {
            name: "my-custom".to_owned(),
            label: Some("My Custom".to_owned()),
            command: vec!["echo".to_owned()],
            builtin: false,
            allowed_models: Some("^custom-.*$".to_owned()),
            ..Default::default()
        };
        settings.custom_agents.push(custom);
        settings
            .custom_agents
            .push(crate::custom_agent::grok_preset("grok"));

        // 1. Unset allowed_runners
        assert_eq!(settings.graph_allowed_runners, None);
        let val = runners_json(&settings);
        assert_eq!(val["ok"], true);
        let runners = val["runners"].as_array().expect("runners array");
        assert_eq!(runners.len(), 5);

        let find = |name: &str| runners.iter().find(|r| r["id"] == name).unwrap();

        let cursor = find("cursor");
        assert_eq!(cursor["label"], "Cursor");
        assert_eq!(cursor["kind"], "builtin");
        assert_eq!(cursor["allowed"], true);
        assert_eq!(cursor["graph_allowed"], true);
        assert_eq!(cursor["default_model"], "composer-2.5");
        assert_eq!(
            cursor["model_policy"],
            "any non-fast model (default composer-2.5)"
        );

        let agy = find("agy");
        assert_eq!(agy["label"], "AGY");
        assert_eq!(agy["kind"], "builtin");
        assert_eq!(agy["allowed"], true);
        assert_eq!(agy["graph_allowed"], true);
        assert_eq!(agy["default_model"], Value::Null);
        assert_eq!(
            agy["model_policy"],
            "gemini-*-flash-*, gemini-4-hydrogen-* or composer-2.5; no fast"
        );

        let codex = find("codex");
        assert_eq!(codex["label"], "Codex");
        assert_eq!(codex["kind"], "builtin");
        assert_eq!(codex["allowed"], true);
        assert_eq!(codex["graph_allowed"], true);
        assert_eq!(codex["default_model"], Value::Null);
        assert_eq!(codex["model_policy"], "composer-2.5 only");

        let grok = find("grok");
        assert_eq!(grok["label"], "grok");
        assert_eq!(grok["kind"], "preset");
        assert_eq!(grok["allowed"], true);
        assert_eq!(grok["graph_allowed"], false);
        assert_eq!(grok["model_policy"], "grok-* without fast");

        let my_custom = find("my-custom");
        assert_eq!(my_custom["label"], "My Custom");
        assert_eq!(my_custom["kind"], "custom");
        assert_eq!(my_custom["allowed"], true);
        assert_eq!(my_custom["graph_allowed"], true);
        assert_eq!(my_custom["model_policy"], "allowed_models = ^custom-.*$");

        // 2. Set allowed_runners
        settings.graph_allowed_runners = Some(vec!["grok".to_owned(), "cursor".to_owned()]);
        let val2 = runners_json(&settings);
        let runners2 = val2["runners"].as_array().expect("runners array");
        let find2 = |name: &str| runners2.iter().find(|r| r["id"] == name).unwrap();
        assert_eq!(find2("cursor")["allowed"], true);
        assert_eq!(find2("cursor")["graph_allowed"], true);
        assert_eq!(find2("agy")["allowed"], false);
        assert_eq!(find2("agy")["graph_allowed"], false);
        assert_eq!(find2("grok")["allowed"], true);
        assert_eq!(find2("grok")["graph_allowed"], true);
        assert_eq!(find2("my-custom")["allowed"], false);
        assert_eq!(find2("my-custom")["graph_allowed"], false);
    }

    // TC-2833 / FR-2832: models_json policy and errors
    #[test]
    fn tc_2833_models_json_policy_and_errors() {
        let settings = DmuxSettings::default();
        let entries = vec![
            (
                "agy".to_owned(),
                ModelListing {
                    source: "command".to_owned(),
                    models: vec![
                        (
                            "gemini-4-hydrogen-high".to_owned(),
                            Some("Gemini 4 Hydrogen (High)".to_owned()),
                        ),
                        (
                            "claude-sonnet-5-5-high".to_owned(),
                            Some("Claude Sonnet 5.5 (High)".to_owned()),
                        ),
                    ],
                    error: None,
                    error_kind: None,
                },
            ),
            (
                "cursor".to_owned(),
                ModelListing {
                    source: "none".to_owned(),
                    models: vec![],
                    error: Some("command timed out".to_owned()),
                    error_kind: None,
                },
            ),
            (
                "grok".to_owned(),
                ModelListing {
                    source: "none".to_owned(),
                    models: vec![],
                    error: Some(crate::custom_agent::grok_login_message()),
                    error_kind: Some("auth".to_owned()),
                },
            ),
        ];

        let val = models_json(&settings, entries);
        assert_eq!(val["ok"], true);
        let runners = val["runners"].as_array().expect("runners array");
        assert_eq!(runners.len(), 3);

        // agy
        let agy = &runners[0];
        assert_eq!(agy["id"], "agy");
        assert_eq!(agy["allowed"], true);
        assert_eq!(agy["source"], "command");
        assert!(agy.get("error").is_none());
        assert!(agy.get("error_kind").is_none());
        let agy_models = agy["models"].as_array().expect("models array");
        assert_eq!(agy_models.len(), 2);
        assert_eq!(agy_models[0]["id"], "gemini-4-hydrogen-high");
        assert_eq!(agy_models[0]["label"], "Gemini 4 Hydrogen (High)");
        assert_eq!(agy_models[0]["allowed"], true);
        assert_eq!(agy_models[1]["id"], "claude-sonnet-5-5-high");
        assert_eq!(agy_models[1]["label"], "Claude Sonnet 5.5 (High)");
        assert_eq!(agy_models[1]["allowed"], false);

        // cursor (failed listing)
        let cursor = &runners[1];
        assert_eq!(cursor["id"], "cursor");
        assert_eq!(cursor["source"], "none");
        assert_eq!(cursor["error"], "command timed out");
        assert!(cursor.get("error_kind").is_none());
        assert_eq!(cursor["models"].as_array().unwrap().len(), 0);

        // grok (unauthenticated)
        let grok = &runners[2];
        assert_eq!(grok["id"], "grok");
        assert_eq!(grok["source"], "none");
        assert_eq!(grok["error"], crate::custom_agent::grok_login_message());
        assert_eq!(grok["error_kind"], "auth");
        assert_eq!(grok["models"].as_array().unwrap().len(), 0);
    }

    // TC-2834 / FR-2833: parse agy models output drops banner, keeps labels and order
    #[test]
    fn tc_2834_parse_agy_models_output() {
        let fixture = "Fetching available models...\ngemini-4-argon-medium\tGemini 4 Argon (Medium)\ngemini-4-hydrogen-high\tGemini 4 Hydrogen (High)\ngemini-4-hydrogen-xhigh\tGemini 4 Hydrogen (XHigh)\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\nclaude-sonnet-5-5-high\tClaude Sonnet 5.5 (High)\n";
        let parsed = parse_agy_models(fixture);
        assert_eq!(
            parsed,
            vec![
                (
                    "gemini-4-argon-medium".to_owned(),
                    Some("Gemini 4 Argon (Medium)".to_owned())
                ),
                (
                    "gemini-4-hydrogen-high".to_owned(),
                    Some("Gemini 4 Hydrogen (High)".to_owned())
                ),
                (
                    "gemini-4-hydrogen-xhigh".to_owned(),
                    Some("Gemini 4 Hydrogen (XHigh)".to_owned())
                ),
                (
                    "gemini-3.8-flash-high".to_owned(),
                    Some("Gemini 3.8 Flash (High)".to_owned())
                ),
                (
                    "claude-sonnet-5-5-high".to_owned(),
                    Some("Claude Sonnet 5.5 (High)".to_owned())
                ),
            ]
        );
    }
}
