# PLAN: Local web dashboard

Status: slice A complete; slices B and C planned
Spec: [SPEC-web-dashboard.md](../specs/SPEC-web-dashboard.md)

## Architecture

- One binary. `dmuxai --web` runs the TUI and an axum HTTP server on the same
  tokio runtime; the App publishes snapshots into a shared `DashboardHub`
  (`tokio::sync::watch`). `dmuxai serve` runs only the server and feeds the hub
  from `.dmuxai/graphs/*.json` (`FileGraphSource`, 500 ms poll), so it works
  next to a TUI running in another tmux pane or terminal.
- Live updates are Server-Sent Events (`GET /api/events`): one-way, proxy- and
  WSL-friendly, auto-reconnect in `EventSource`, no WebSocket dependency.
- UI is plain HTML/JS/CSS in `src/web/assets/`, embedded with `include_str!`.
  No frontend build.
- Security: loopback bind by default, per-start token (Bearer or `?token=`),
  Host/Origin allow-list before auth, non-loopback only with `--allow-remote`
  plus a warning. Static assets are public but carry no data.
- Windows: run inside WSL; WSL forwards `localhost` to the Windows browser.
  The native Windows build compiles and its tests pass (CI).

## Module contract (slice A)

`src/web/snapshot.rs` (FR-525, NFR-521)

```rust
pub const SNAPSHOT_SCHEMA: &str = "dmuxai.dashboard/v1";
#[serde(rename_all = "snake_case")] pub enum SnapshotSource { Tui, Files }
pub struct DashboardSnapshot { pub schema: String, pub revision: u64, pub generated_at_ms: u64,
    pub source: SnapshotSource, pub workspace: String, pub agents: Vec<AgentView>, pub graph: Option<GraphView> }
pub struct AgentView { pub id: String, pub label: String, pub busy: bool, pub phase: Option<String>, pub model: Option<String> }
pub struct GraphView { pub id: String, pub phase: GraphRunPhase, pub orchestrator_model: String, pub user_prompt: String,
    pub current_nodes: Vec<String>, pub waiting_gate_node: Option<String>, pub counts: GraphCounts,
    pub nodes: Vec<NodeView>, pub edges: Vec<GraphEdge> }
pub struct GraphCounts { pub pending: usize, pub ready: usize, pub running: usize, pub waiting_gate: usize,
    pub completed: usize, pub failed: usize, pub skipped: usize }
pub struct NodeView { pub id: String, pub role: NodeRole, pub job: String, pub runner: String, pub model_id: String,
    pub status: GraphNodeStatus, pub attempt: u32, pub started_at_ms: Option<u64>, pub finished_at_ms: Option<u64>,
    pub elapsed_ms: Option<u64> }
pub fn snapshot_from_run(run: &GraphRun) -> GraphView; // timings None; job/user_prompt truncated to 200 chars on a char boundary with "…"
```

All derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`.

`src/web/hub.rs` (FR-526)

```rust
#[derive(Clone)] pub struct DashboardHub { /* Arc<Mutex<state>> + watch::Sender<DashboardSnapshot> */ }
impl DashboardHub {
    pub fn new(source: SnapshotSource, workspace: String) -> Self;
    pub fn publish(&self, agents: Vec<AgentView>, graph: Option<GraphView>); // system clock
    pub fn publish_at(&self, agents: Vec<AgentView>, graph: Option<GraphView>, now_ms: u64);
    pub fn snapshot(&self) -> DashboardSnapshot;
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<DashboardSnapshot>;
}
```

`publish_at` fills node timings from remembered per-node state (reset when the
graph id changes), sets `elapsed_ms` only for finished nodes with a start, and
bumps `revision` (and notifies) only when agents/graph differ from the previous
snapshot. It never blocks on subscribers.

`src/web/auth.rs` (FR-521, FR-522, FR-524)

```rust
pub fn generate_token() -> String;                         // 64 lowercase hex (two uuid v4 simple)
pub fn dashboard_url(addr: SocketAddr, token: &str) -> String; // http://127.0.0.1:7420/?token=..; IPv6 in []
pub enum BindCheck { Loopback, Remote { warning: String } }
pub fn check_bind(ip: IpAddr, allow_remote: bool) -> Result<BindCheck, String>; // 0.0.0.0 / :: are remote
pub(crate) fn token_matches(expected: &str, given: &str) -> bool; // constant time
```

`src/web/server.rs` (FR-522, FR-523, FR-527, FR-529)

```rust
pub struct ServerConfig { pub bind: IpAddr, pub port: u16, pub allow_remote: bool, pub token: String }
pub fn build_router(hub: DashboardHub, token: String, allowed_hosts: Option<Vec<String>>) -> axum::Router;
pub struct DashboardServer { pub addr: SocketAddr, pub url: String, pub warning: Option<String>, /* JoinHandle */ }
pub async fn start_server(config: ServerConfig, hub: DashboardHub) -> std::io::Result<DashboardServer>;
```

- `allowed_hosts = Some([...])` for loopback binds (`127.0.0.1:P`, `localhost:P`,
  `[::1]:P`); `None` for remote binds (any Host). In both cases a present
  `Origin` must equal `http://<Host>`; otherwise 403. Host check runs first.
- Routes: `GET /` (index.html), `GET /assets/app.js`, `GET /assets/app.css`
  (public, `Cache-Control: no-store`, CSP `default-src 'self'`), `GET /api/state`
  (JSON), `GET /api/events` (SSE `event: snapshot`, keep-alive 15 s). `/api/*`
  needs the token; 401 body is `{"error":"unauthorized"}`.
- `start_server`: `check_bind` (error -> `io::ErrorKind::InvalidInput`), bind;
  if `port == DEFAULT_PORT` and the port is in use, retry with port 0.

`src/web/file_source.rs` (FR-528)

```rust
pub struct FileGraphSource { /* dir: <workspace>/.dmuxai/graphs, last: Option<GraphView> */ }
impl FileGraphSource { pub fn new(workspace: &Path) -> Self; pub fn poll(&mut self) -> Option<GraphView>; }
```

`src/web/assets.rs` + `src/web/assets/{index.html,app.js,app.css}` (FR-529)

`src/main.rs` (FR-520, FR-521, FR-524)

- `CliAction::Serve(WebOptions)` when the first arg is `serve`;
  `CliAction::RunWeb(WebOptions)` when `--web` is present; `WebOptions { port: u16 = DEFAULT_PORT,
  bind: IpAddr = 127.0.0.1, allow_remote: bool }`; `--port`, `--bind`, `--allow-remote` valid only with
  `serve`/`--web`. Malformed -> `CliAction::Unknown` (exit 2).
- `serve`: no TTY check; resolve workspace; hub (`Files`); start server; print
  the URL as the first stdout line (flush); warning to stderr; poll the file
  source every 500 ms; exit on Ctrl+C.
- `--web`: as today plus hub (`Tui`), `app.attach_dashboard(hub)`, URL and
  warning pushed as startup notices; a start failure is a notice, not a crash.

`src/app.rs` (FR-530)

- `pub fn attach_dashboard(&mut self, hub: DashboardHub)` and
  `fn publish_dashboard(&self)` building `AgentView`s from the adapters
  (`id` = runner name, `label`, `busy`, activity phase label, selected model) and
  the graph from `graph_session` via `snapshot_from_run`. Called from the run
  loop whenever events were drained or state is dirty (hub dedups).

## Order and owners (dogfood through dmuxai)

1. AGY (`gemini-3.8-flash-high`): `src/web/{snapshot,hub,auth,server,file_source,assets}.rs`, `lib.rs`, TC-521..TC-529.
2. opencode glm (`glm-5.3-flash-free`), in parallel: `src/web/assets/*`, docs (TASKS/REVIEW/TEST-PLAN rows).
3. opencode deepseek (`deepseek-v4.1-flash-free`) after 1: `src/main.rs`, `src/app.rs`, `tests/web_serve.rs`, TC-520, TC-530, TC-531; cross-review of 1.
4. glm cross-reviews 3; the planner reviews everything and runs the gates.

## Risks and mitigations

- New HTTP dependency (axum/hyper): minimal features (`http1`, `json`, `query`, `tokio`).
- Token in URL can leak via browser history: loopback-only by default, token rotates per start, `Referrer-Policy: no-referrer`.
- TUI loop cost: hub dedups by content; publish is a cheap clone under a mutex.
- Windows CRLF in embedded assets: tests use `contains`, never byte-exact compares.

## Rollback

Remove `src/web`, the CLI branches and `attach_dashboard`; no persistence change in slice A.
