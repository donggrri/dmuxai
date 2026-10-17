# PLAN: Local web dashboard

Status: implemented in v0.2.0 (slices A, B and C)
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

## Slice B contract (to-do board)

`src/todo.rs` (FR-540): `TodoStore` (own `rusqlite::Connection` behind a `Mutex`, WAL,
`busy_timeout` 5 s) on the per-repo DB file (`state_path::repo_db_path`); additive
`CREATE TABLE IF NOT EXISTS todos (...)` + unique index on `(source, source_ref)` where
`source_ref IS NOT NULL`; no `user_version` change. Types `Todo`, `TodoStatus`,
`TodoSource`, `NewTodo`, `TodoPatch`, `TodoError` and the method list are fixed in the stub.

`src/web/commands.rs` (FR-542): `DashboardCommand::RunTodo { todo_id, mode: RunMode, reply }`
with `RunTodoError` -> HTTP mapping documented in the file.

`src/web` (FR-541, FR-542):

```rust
pub struct DashboardServices { pub todos: Option<Arc<TodoStore>>, pub commands: Option<mpsc::Sender<DashboardCommand>> }
impl Default for DashboardServices // both None
pub fn build_router_with(hub, token, allowed_hosts, services: DashboardServices) -> Router; // build_router = with Default
pub struct ServerConfig { ..., pub services: DashboardServices }
impl DashboardHub { pub fn set_todos(&self, todos: Vec<Todo>) } // revision bump only on change
DashboardSnapshot { ..., #[serde(default)] pub todos: Vec<Todo> }
```

Routes (token + Host/Origin checks as slice A): `GET /api/todos` (200 list),
`POST /api/todos` (`{"title","body"?}` -> 201 todo), `PATCH /api/todos/{id}`
(`{"title"?,"body"?,"status"?: "open"|"done"}` -> 200), `DELETE /api/todos/{id}` (204),
`POST /api/todos/{id}/run` (`{"mode":"agent","agent":"..."}` | `{"mode":"graph"}`;
no sender -> 409 `{"error":"requires dmuxai --web"}`; reply timeout 10 s -> 504; ok -> 202 todo).
Non-JSON content type -> 415; validation -> 400; store errors -> 500 `{"error":"internal"}`.
No todo store -> 503. After every successful mutation the router calls `hub.set_todos(store.list())`.

`src/main.rs`: both modes open `TodoStore` on `repo_db_path(workspace)`; `serve` passes
`commands: None`; `--web` creates `mpsc::channel(16)`, passes the sender to the server and
`app.attach_dashboard_services(store, receiver)`.

`src/app.rs` (FR-542): `pub fn attach_dashboard_services(&mut self, store: Arc<TodoStore>, commands: mpsc::Receiver<DashboardCommand>)`;
the run loop drains commands with `try_recv`; `RunTodo` resolves the to-do (`NotFound`,
`AlreadyRunning`), the agent by runner id (`UnknownAgent`), `[graph] allowed_runners`
(`Forbidden`), busy/graph-active (`Busy`), then submits `title\n\nbody` through the same
paths as a typed prompt (agent: `submit_to_agent`; graph: `plan_request` -> `start_graph_run`,
or the single-plan agent path), marks the to-do running and remembers the link
(agent -> todo id; graph id -> todo id). `finish_turn_outcome` and graph `Completed`/`Failed`
call `finish_run`; every change calls `hub.set_todos`.

Owners: AGY -> `src/app.rs` command handling + TC-542; opencode deepseek -> `src/todo.rs`,
`src/web` routes/hub/snapshot, `src/main.rs` wiring + TC-540, TC-541, TC-544, TC-545;
opencode glm -> page to-do panel + TC-543 + docs; cross-review deepseek <-> glm, AGY reviewed by deepseek.
## Slice C contract (shared MCP config, propagation, client, import)

### Config (`src/mcp/config.rs`, wired in `src/settings.rs`)

```toml
[[mcp.servers]]
name = "everything"                 # unique key, [A-Za-z0-9_-]{1,64}
command = "npx"                     # stdio transport (exclusive with url)
args = ["-y", "@modelcontextprotocol/server-everything"]
env = { API_TOKEN = { env = "MY_TOKEN" }, MODE = "demo" }   # literal or {env="VAR"}
# url = "https://mcp.example/mcp"   # streamable HTTP transport
# headers = { Authorization = { env = "JIRA_AUTH" } }
kind = "generic"                    # generic | jira | confluence
runners = ["codex", "opencode-glm"] # optional allow-list; absent = all runners
enabled = true
[mcp.servers.tools]                 # import tool names (jira/confluence only)
search = "search"
get = "get"
```

- `McpServerSpec` (Deserialize) is parsed from user and repo TOML. Merge in
  `DmuxSettings::apply_toml`: by `name`; later (repo) replaces earlier (user);
  `enabled = false` removes. Invalid entries (both/neither command and url, bad
  name, empty command) -> `config_warnings`, entry skipped.
- `resolve_servers(&[McpServerSpec], env: impl Fn(&str)->Option<String>) ->
  (Vec<ResolvedMcpServer>, Vec<String> warnings)`. A missing env var disables
  that server with warning `mcp server <name> disabled: env var <VAR> is not
  set` (never the value). `ResolvedMcpServer` implements `Debug` by hand with
  values redacted (`env`/`headers` values shown as `***`).
- `ResolvedMcpServer::applies_to(runner_id) -> bool`.

### Propagation (`src/mcp/propagate.rs`), pure functions + adapter hooks

| Runner | Function | Native form |
| --- | --- | --- |
| opencode presets / custom with `temp_config_template` whose JSON is an object | `merge_opencode_mcp(config_json: &str, servers) -> String` | adds `"mcp": {"<name>": {"type":"local","command":[cmd,..args],"environment":{..},"enabled":true}}` or `{"type":"remote","url":..,"headers":{..},"enabled":true}`; existing keys preserved; existing `mcp.<name>` wins |
| Codex | `codex_config_overrides(servers) -> Vec<String>` | pairs `-c`, `mcp_servers.<name>.command="npx"`, `-c`, `mcp_servers.<name>.args=["-y","x"]`, `-c`, `mcp_servers.<name>.env={K="v"}` or `mcp_servers.<name>.url="..."`; values TOML-encoded; inserted before the prompt / subcommand args |
| custom agents | `custom_mcp_json(servers) -> String` | `{"mcpServers":{"<name>":{"command":..,"args":[..],"env":{..}}|{"url":..,"headers":{..}}}}` written per turn to a temp file; `{mcp_config}` placeholder + env `DMUXAI_MCP_CONFIG=<path>`; removed after the turn (same lifecycle as `temp_config`) |
| Cursor | `cursor_mcp_guard(workspace, servers) -> Option<CursorMcpGuard>` | writes `<ws>/.cursor/mcp.json` (`mcpServers` shape) only if it does not exist; guard removes the file (and `.cursor` if it created it and it is empty) on drop; if the file exists -> no write and one notice |
| AGY | `agy_mcp_notice(servers) -> String` | AGY only has user-level `agy mcp add`; dmuxai never runs it (OQ-523); one session notice listing the `agy mcp add ...` commands |

Each adapter filters by `applies_to(runner_id)`. Secrets are passed only via the
child env / per-run temp file (mode 0600 on unix), never logged.

### Client (`src/mcp/client.rs`)

`McpClient::connect(&ResolvedMcpServer, timeout) -> Result<McpClient, McpError>`;
`list_tools() -> Vec<McpTool{name, description}>`; `call_tool(name, args:
Value) -> Value` (the `result`), all with a per-request timeout (default 20 s).
JSON-RPC 2.0, protocol version `2025-06-18`, `initialize` then
`notifications/initialized`.
- stdio: spawn with resolved env, newline-delimited JSON on stdin/stdout,
  stderr drained (kept last 4 KB for errors), kill_on_drop.
- streamable HTTP: POST JSON with `Accept: application/json, text/event-stream`,
  configured headers, `Mcp-Session-Id` echoed after initialize; response either
  JSON or SSE (`data:` lines; first message whose `id` matches).
- `McpError` always contains the server name; tool `isError: true` -> error.

### Mock (`src/bin/dmuxai-mock-mcp.rs` not shipped? -> `tests/support` or `src/mcp/mock.rs` cfg(test))

Deterministic in-repo mock with tools `echo`, `jira_search`, `jira_get`,
`confluence_search`, `confluence_get`, `fail`; fixture issues `DEMO-1`, `DEMO-2`
and page `1001` with fake text only. Served over stdio (test binary via
`std::env::current_exe` helper or a `dmuxai mcp-mock` hidden subcommand) and over
streamable HTTP on 127.0.0.1:0 (axum).

### Import (`POST /api/import`)

Body `{ "server": "<name>", "query": "<key|JQL|page id|CQL>" }`. Server must be
`kind` jira/confluence. Calls `tools.search` with `{ "query": q }` (or
`tools.get` with `{ "id": q }` when q looks like a key `^[A-Z][A-Z0-9]+-\d+$` or a
numeric page id); expects `structuredContent.items[]` or JSON text content with
`items[]` of `{ id, title, url?, body? }`; upserts to-dos with `source`
jira/confluence, `source_ref` = id, body = url + body (truncated). Response
`{ imported, updated, todos }`. Unknown server 404, wrong kind 400, tool error
502 `{ "error": "<server>: <message>" }`, no store 503. `GET /api/mcp/servers`
lists `name`, `kind`, `transport` (no env/headers/url query strings).

### Slice C implementation decisions

- **HTTP client:** `reqwest` with `default-features = false` and
  `rustls-no-provider`, plus `rustls` with the `ring` provider installed once
  (`install_default`, errors ignored). This avoids aws-lc (cmake/NASM on
  Windows) and keeps the static musl release build working. Both `http://` and
  `https://` are supported.
- **Mock entry point:** a hidden `dmuxai __mock-mcp` subcommand runs
  `mcp::mock::serve_stdio()`. It is not listed in `--help`. Tests use
  `env!("CARGO_BIN_EXE_dmuxai")`.
- **Settings to runtime:** `App` and `run_serve` resolve the list with
  `resolve_servers(&settings.mcp_servers, |k| std::env::var(k).ok())` and
  surface the warnings as config notices.
- **Adapter hooks** (INT-563):
  - The custom adapter gets the resolved list at spawn. Each turn:
    - merges `mcp` into the rendered `temp_config` when the template is JSON;
    - writes `custom_mcp_json` to a second temp file for `{mcp_config}` and
      `DMUXAI_MCP_CONFIG`, removed with the turn.
  - Codex adds the `codex_config_overrides` args to `app-server --stdio` at
    spawn.
  - Cursor holds a `cursor_mcp_guard` for the duration of each turn.
  - AGY: the App shows `agy_mcp_notice` once per session.
- **Import:** a `DashboardServices.mcp` (`Arc<McpRegistry>`, holding the
  resolved list and connecting lazily per request with a 20 s timeout) serves
  `/api/mcp/servers` and `/api/import`, in both `serve` and `--web`.
