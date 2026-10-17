# PLAN: Agent API (SPEC-agent-api, FR-760..FR-773)

Status: implemented in v0.2.0

## Module contracts (shared names — do not rename)

### `src/events.rs` (lib, `pub mod events;`) — FR-764, FR-768, FR-769

```rust
pub const EVENT_SCHEMA_VERSION: u32 = 1;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    pub v: u32, pub seq: u64, pub ts: String /* RFC3339 UTC, ms */, pub ts_ms: u64,
    pub workspace: String, pub kind: String /* agent|node|graph|todo */,
    pub task: Option<String>, pub todo: Option<String>, pub node: Option<String>,
    pub runner: Option<String>, pub model: Option<String>,
    pub from_state: Option<String>, pub to_state: String, pub message: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventDraft { pub kind: String, pub task: Option<String>, pub todo: Option<String>,
    pub node: Option<String>, pub runner: Option<String>, pub model: Option<String>,
    pub from_state: Option<String>, pub to_state: String, pub message: Option<String> }
pub fn events_path(workspace_root: &Path) -> PathBuf;          // <root>/.dmuxai/events.jsonl
pub fn format_ts(ms: u64) -> String;                            // "2026-10-17T11:22:33.456Z"
pub fn read_since(path: &Path, since: u64, limit: Option<usize>) -> Vec<EventRecord>; // skips bad lines
pub struct EventLog;   // Clone
impl EventLog {
    pub fn open(workspace_root: &Path, workspace_label: String) -> Self; // never fails; dirs created lazily
    pub fn path(&self) -> &Path;
    pub fn last_seq(&self) -> u64;
    /// Cross-process safe: lock file `<root>/.dmuxai/events.lock` (create_new, stale after 5 s),
    /// re-reads the last seq under the lock, appends one line per draft, fsync not required.
    pub fn append(&self, drafts: Vec<EventDraft>) -> std::io::Result<Vec<EventRecord>>;
}
pub struct StateTracker; // first observe() is a silent baseline
impl StateTracker { pub fn new() -> Self;
    pub fn observe(&mut self, snapshot: &crate::web::DashboardSnapshot) -> Vec<EventDraft>; }
pub struct StallDetector;
impl StallDetector { pub fn new(after: Duration) -> Self; // ZERO disables
    pub fn busy(&mut self, runner: &str, model: Option<String>, now: Instant);
    pub fn activity(&mut self, runner: &str, now: Instant);
    pub fn idle(&mut self, runner: &str);
    pub fn tick(&mut self, now: Instant) -> Vec<EventDraft>; }  // one `stalled` per busy span
pub struct EventSink;
impl EventSink {
    pub fn new(log: EventLog, stall_after: Duration, notifier: Option<crate::notify::Notifier>) -> Arc<Self>;
    pub fn log(&self) -> &EventLog;
    /// Appends, broadcasts and notifies. kind=agent drafts drive the stall detector
    /// (pending/started/running => busy/activity; completed/error/cancelled => idle).
    pub fn emit(&self, drafts: Vec<EventDraft>);
    pub fn observe_snapshot(&self, snapshot: &crate::web::DashboardSnapshot);
    pub fn note_activity(&self, runner: &str);
    pub fn tick(&self);
    pub fn spawn_ticker(self: &Arc<Self>); // tokio interval 5 s, holds a Weak
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EventRecord>;
}
```

### `src/web/hub.rs` additions

```rust
impl DashboardHub {
    pub fn with_events(self, sink: Arc<crate::events::EventSink>) -> Self;
    pub fn events(&self) -> Option<Arc<crate::events::EventSink>>;
    pub fn emit_event(&self, draft: crate::events::EventDraft);
    pub fn note_runner_activity(&self, runner: &str);
}
```
`publish_at` and `set_todos` call `sink.observe_snapshot(&snapshot)` after a content change, outside the state lock.

### `src/notify.rs` (lib) — FR-770..FR-772

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct NotifySettings { pub webhook_url: Option<String>, pub webhook_secret_env: Option<String>,
    pub secret_header: Option<String>, pub timeout_secs: Option<u64>, pub command: Option<Vec<String>>,
    pub events: Option<Vec<String>>, pub stall_after_secs: Option<u64> }
impl NotifySettings { pub fn stall_after(&self) -> Duration; /* default 600 s */ pub fn validate(&self) -> Vec<String>; }
pub struct Notifier; // Clone
impl Notifier { pub fn spawn(settings: &NotifySettings, workspace_root: &Path) -> Option<Self>;
    pub fn notify(&self, record: &crate::events::EventRecord); } // try_send, never blocks
```
`DmuxSettings.notify: NotifySettings` from `[notify]`; `validate()` warnings join `config_warnings`.

### `src/web/discovery.rs` — FR-773

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery { pub base_url: String, pub port: u16, pub pid: u32, pub token: String }
pub fn discovery_path(workspace_root: &Path) -> PathBuf; // <root>/.dmuxai/web.json
pub fn write_discovery(workspace_root: &Path, d: &Discovery) -> io::Result<PathBuf>; // 0600 on unix
pub fn remove_discovery(workspace_root: &Path);
pub fn read_discovery(workspace_root: &Path) -> Option<Discovery>;
pub fn token_from_env(value: Option<&str>) -> Result<Option<String>, String>;
```

### `src/app.rs` (binary) — FR-761, FR-762, FR-765, FR-768

```rust
pub struct HeadlessRequest { pub prompt: String, pub agent: Option<String>, pub mode: Option<ExecutionMode>,
    pub graph: bool, pub todo_id: Option<String> }
pub struct HeadlessOutcome { pub ok: bool, pub exit_code: i32, pub conversation_id: String,
    pub agent: Option<String>, pub graph_id: Option<String>, pub text: String,
    pub messages: Vec<HeadlessMessage>, pub error: Option<String>, pub todo: Option<Todo> }
pub struct HeadlessMessage { pub role: String, pub agent: Option<String>, pub content: String } // Serialize
impl App {
    pub fn attach_todo_store(&mut self, store: Arc<TodoStore>); // no reconcile, no command channel
    pub async fn run_headless(&mut self, req: HeadlessRequest, cancel: Arc<AtomicBool>) -> HeadlessOutcome;
}
```
`DashboardCommand::SubmitPrompt { text, agent: Option<String>, mode: Option<String>, reply: oneshot::Sender<Result<PromptAccepted, RunTodoError>> }`
with `PromptAccepted { conversation_id: String, agent: String }` (Serialize) in `src/web/commands.rs`.

Exit codes live in `dmuxai::agent_api::exit`: OK 0, FAILED 1, USAGE 2, NOT_FOUND 3, CONFLICT 4,
FORBIDDEN 5, INTERNAL 6, CANCELLED 130.

## Ownership (parallel work, one owner per file)

| Owner | Files | Requirements / tests |
| --- | --- | --- |
| opencode-deepseek | `src/events.rs`, `src/web/hub.rs`, `src/lib.rs` (`pub mod events;`) | FR-764/768/769, TC-765 (unit), TC-769, TC-770, TC-771 |
| opencode-glm | `src/notify.rs`, `src/settings.rs`, `src/web/discovery.rs`, `src/web/mod.rs`, `Cargo.toml` | FR-770..773, TC-772, TC-773, TC-774 |
| AGY | `src/app.rs`, `src/web/commands.rs`, `src/web/server.rs` | FR-761/762/765/766/768 (agent events), TC-766, TC-767 |
| maintainer | `src/main.rs`, `src/agent_api.rs`, `src/mcp_server.rs`, `tests/agent_api_cli.rs`, docs | FR-760/763/764/767/773 wiring, TC-760..765, TC-768 |

Cross-review: deepseek reviews glm's diff and AGY's server routes; glm reviews deepseek's
events/hub diff; AGY reviews the maintainer CLI/MCP diff.

## Risks and rollback

- Two processes appending the same log: lock file with stale-lock recovery (TC-770).
- Webhook/command latency: bounded queue, background task, timeouts (TC-772/773).
- Rollback: remove the new subcommands and modules; no schema change.
