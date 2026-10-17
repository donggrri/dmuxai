# SPEC: Local web dashboard (live graph, to-do board, MCP import)

Status: In progress (slices A and B implemented; slice C planned)
Owner: DmuxAI
Created: 2026-10-17
Baseline: `5ff8c93`
Related: SPEC-agent-graph-orchestration, SPEC-graph-default-status, SPEC-state-per-repo, SPEC-windows-desktop

## 1. Decision summary

Users want to watch which graph-engineering step is running, manage to-dos, and
pull work from Jira/Confluence without staring at the TUI or installing a
desktop app. DmuxAI serves a local web dashboard from the same binary: `dmuxai
--web` runs the TUI and a dashboard that share one in-process App/graph state,
and `dmuxai serve` runs a read-only dashboard over persisted graph runs without a
TTY. The server binds `127.0.0.1` by default, requires a random per-start access
token, and rejects foreign origins and hosts. The UI is static HTML/JS/CSS
embedded in the binary (no frontend build); the browser on Linux, or on Windows
via WSL `localhost` forwarding, opens the printed URL. Delivery is three PRs:
slice A (server, auth, live graph state), slice B (to-do board), slice C (MCP
client and Jira/Confluence import, mock-tested only).

## 2. Scope

### In scope

- Slice A: `dmuxai serve` and `dmuxai --web`; token auth; origin/host checks;
  bind policy; `GET /api/state`; `GET /api/events` (SSE); an embedded page that
  renders live graph progress (current node, status, runner/model, timings) and
  agent busy state.
- Slice B: to-do board persisted in the per-repo DB; add/edit/done/delete;
  "run with agent" and "run as graph" from a to-do (TUI `--web` mode only).
- Slice C: one shared MCP server list (`[[mcp.servers]]` in user config,
  merged with repo config) propagated to every runner in its native per-run
  form; the dashboard's own MCP client (stdio and streamable HTTP) on the same
  list; Jira/Confluence as configurable entries importing into to-dos.

### Out of scope / non-goals

- Remote/multi-user hosting, TLS, accounts. Non-loopback bind is opt-in only.
- A desktop app shell (the WSL launcher remains as-is).
- Editing chat history or driving arbitrary TUI keystrokes from the browser.
- Writing to Jira/Confluence (import is read-only).
- Connecting to any real company Jira/Confluence in tests or CI; only a local
  mock MCP server and fixtures are used.
- Fixing the known TUI dogfood issues (Ctrl+C graph continuation, orphaned
  opencode children, default_agent at startup); tracked separately.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-520 | Resolved (Dongjin, 2026-10-17): default port 7420; if taken, an ephemeral port is used and the actual URL is printed. | Dongjin | Done |
| OQ-521 | Resolved (Dongjin, 2026-10-17): `dmuxai serve` is view-only; running to-dos requires `dmuxai --web`, and the page disables the run controls with a hint in serve mode (UX-540). | Dongjin | Done |
| OQ-522 | Resolved (Dongjin, 2026-10-17): Jira/Confluence stay config entries only (documented example: the Atlassian remote MCP over streamable HTTP with env-var auth); no real connection; tests use the in-repo mock or `server-everything`. | Dongjin | Done |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Dashboard server | The HTTP server started by `--web` or `serve` | The TUI event loop |
| Access token | Random hex string generated per server start; required by every `/api/*` request | Provider credentials |
| Snapshot | The JSON document `dmuxai.dashboard/v1` describing agents and the active graph run | A persisted `GraphRun` file |
| Revision | Monotonic counter incremented only when snapshot content changes | Wall-clock time |
| Source | `tui` (in-process App) or `files` (polling `.dmuxai/graphs/*.json`) | Runner |
| Node timing | `started_at_ms` set on first observed `running`; `finished_at_ms` on first observed `completed`/`failed`/`skipped` | Agent activity timer |
| To-do | A user work item stored per repo: `open` -> `running` -> `done`, or `open` -> `done`; `deleted` is terminal and removes it | Graph node |

## 4. Requirements

### Slice A: server, security, live graph

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-520 | Must | Given the CLI, when the user runs `dmuxai serve [--port N] [--bind ADDR] [--allow-remote]` or `dmuxai --web [--port N] [--bind ADDR] [--allow-remote]` | `serve` starts the dashboard without requiring a TTY; `--web` starts the TUI plus the dashboard; `--help` documents both | Unknown flags or a missing/invalid flag value exit 2 with usage; `--help`/`--version` keep current behavior | TC-520, TC-531 |
| FR-521 | Must | Given a server start | A fresh random access token of at least 32 lowercase hex characters is generated and the URL `http://<host>:<port>/?token=<token>` is printed (stdout for `serve`; a TUI startup notice for `--web`) | Two starts never reuse a token | TC-521, TC-531 |
| FR-522 | Must | Given a request to any `/api/*` path | It succeeds only with `Authorization: Bearer <token>` or `?token=<token>` (constant-time compare) | Missing or wrong token returns 401 with no state in the body | TC-522 |
| FR-523 | Must | Given a request with an `Origin` header, or a `Host` header | For loopback binds the Host must be `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>`; for opt-in remote binds any Host is accepted (the token is the guard). In both cases a present Origin must equal `http://<Host>` | Mismatch, a missing Host, or `Origin: null` returns 403 before auth; protects against cross-site and DNS-rebinding access | TC-523 |
| FR-524 | Must | Given `--bind ADDR` | Loopback addresses bind normally; a non-loopback address binds only with `--allow-remote` and prints a warning that anyone who can reach the port and has the token can control agents | Non-loopback without `--allow-remote` refuses to start (exit 2) | TC-524 |
| FR-525 | Must | Given a graph run is active or the latest persisted run exists | `GET /api/state` returns snapshot `dmuxai.dashboard/v1` with `revision`, `generated_at_ms` (last change), `sent_at_ms` (server clock at send time, used for live elapsed), `source`, `agents[]` (`id`, `label`, `busy`, `phase`, `model`), and `graph` (`id`, `phase`, `orchestrator_model`, `current_nodes[]`, `waiting_gate_node`, `counts`, `nodes[]` with `id`, `role`, `job`, `runner`, `model_id`, `status`, `attempt`, `started_at_ms`, `finished_at_ms`, `elapsed_ms`, and `edges[]`) | No graph gives `graph: null`; long `job`/prompt text is truncated to 200 chars | TC-525 |
| FR-526 | Must | Given successive snapshots | Node timings follow the state model per attempt (a retry with a higher `attempt` restarts `started_at_ms` and clears `finished_at_ms`); `revision` increases only when content changes; a different graph id resets timings | A node that skips `running` and finishes directly has `started_at_ms: null`. In `serve` mode timings are when the server first observed the status (persisted runs carry no timestamps) | TC-526 |
| FR-527 | Must | Given `GET /api/events` with a valid token | The response is `text/event-stream`; the first event is `event: snapshot` with the current snapshot, then one `snapshot` event per revision change, plus keep-alive comments | Slow clients receive the latest snapshot (intermediate revisions may be skipped); disconnects do not affect the TUI | TC-527 |
| FR-528 | Must | Given `dmuxai serve` in a workspace | The snapshot source is `files`: the most recently modified parseable `.dmuxai/graphs/*.json`, re-read at least every second | Missing directory gives `graph: null`; a malformed or partially written file is ignored and the previous snapshot is kept | TC-528 |
| FR-529 | Must | Given `GET /` and `/assets/*` | The embedded page renders the graph as an ordered node list with status badges, runner/model, live elapsed time, the current node highlighted, and agent busy chips; it reads the token from the URL, uses `/api/events`, and falls back to polling `/api/state` if SSE fails | Static responses carry `Cache-Control: no-store` and a CSP of `default-src 'self'`; static assets contain no workspace data and do not require the token | TC-529 |
| FR-530 | Must | Given `dmuxai --web` | The App publishes its agents and active graph session to the dashboard at least on every state change in the event loop (source `tui`) | Dashboard publishing never blocks the TUI loop and server errors surface as a TUI notice, not a crash | TC-530 |
| NFR-520 | Must | Given the build | No frontend toolchain is needed: assets are embedded with `include_str!`; Linux and Windows (`cargo clippy -D warnings`) both build | Windows CI stays green | TC-529, CI |
| NFR-521 | Must | Given any snapshot or log line | The access token is never included in `/api/state` or SSE payloads | Only the startup URL contains it | TC-525 |

### Slice B: to-do board

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-540 | Must | Given the per-repo DB | To-dos persist in an additive `todos` table (`id`, `title`, `body`, `status` `open`/`running`/`done`, `source` `manual`/`jira`/`confluence`, `source_ref`, `run_kind`, `run_ref`, `last_error`, `created_at_ms`, `updated_at_ms`) and survive restart; list order is open/running first, then by `created_at_ms` | Table created with `CREATE TABLE IF NOT EXISTS` without bumping `user_version`, so older binaries still open the DB; `(source, source_ref)` is unique when `source_ref` is set | TC-540 |
| FR-541 | Must | Given `GET/POST /api/todos`, `PATCH/DELETE /api/todos/{id}` with a valid token, and a JSON body | List, create (`title`, optional `body`), edit `title`/`body`, set `status` to `open` or `done`, delete; the to-do list appears in the snapshot `todos[]` and on SSE | Empty/whitespace title or title > 200 chars -> 400; body > 20000 chars -> 400; unknown id -> 404; non-JSON content type -> 415; setting `running` directly -> 400; all mutating routes need the token and the Origin/Host checks of FR-522/FR-523 | TC-541, TC-544 |
| FR-542 | Must | Given `POST /api/todos/{id}/run` with `{ "mode": "agent", "agent": "<runner id>" }` or `{ "mode": "graph" }` in `--web` mode | The prompt `title` + blank line + `body` is submitted to that agent, or through the graph planner, exactly like a TUI prompt; the to-do becomes `running` with `run_kind`/`run_ref` (agent id, or graph id); when the turn or graph completes it becomes `done`, and on failure or cancel it returns to `open` with `last_error` | `serve` mode -> 409 `requires dmuxai --web`; unknown agent -> 404; agent busy or a graph already active -> 409; runner not in `[graph] allowed_runners` -> 403; already running -> 409. The run never bypasses the agent's current mode (Ask/Plan/Implement) | TC-542, TC-545 |
| UX-540 | Should | Given the page | A to-do panel: add form (title, optional body), list with inline edit, done checkbox, delete, and `Run` controls (agent select from snapshot agents, `Run as graph`); running to-dos show their run link and the graph view | Errors show inline next to the item. When the snapshot `source` is `files` (`dmuxai serve`), the run controls are disabled and a hint says to start `dmuxai --web`; add/edit/done/delete stay enabled | TC-543 |

### Slice C: shared MCP config, per-runner propagation, dashboard MCP client and Jira/Confluence import (planned)

MCP servers are defined once and used by every runner and by the dashboard.

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| INT-560 | Must | Given `[[mcp.servers]]` in the user config (`~/.config/dmuxai/dmuxai.toml`) and/or the repo `dmuxai.toml` | One merged list: entries keyed by `name`; a repo entry overrides the user entry of the same name; `enabled = false` removes it. Each entry has `transport = "stdio"` (`command`, `args`, `env`) or `transport = "http"` (`url`, `headers`), optional `kind` (`jira`, `confluence`, `generic`) and optional `runners` allow-list | Secret values are literal or `{ env = "VAR" }` references resolved at runtime; a missing env var disables that server with a warning; secrets never reach the DB, logs, snapshot or the repo; invalid entries become config warnings | TC-560 |
| INT-561 | Must | Given the merged list and a runner turn | The servers reach the runner in its native form without editing user-owned config files: opencode via the per-run `OPENCODE_CONFIG` temp file `mcp` key (`type: local`/`remote`); Codex via per-run `-c mcp_servers.<name>.*` overrides; custom agents via a per-run JSON file (`{"mcpServers":{...}}`) exposed as `{mcp_config}` placeholder and `DMUXAI_MCP_CONFIG` env var; Cursor and AGY per the runner matrix in section 6 | A runner that cannot accept per-run MCP config is reported once per session as a notice naming the runner, and its turn still runs; generated files live in a temp dir removed after the turn | TC-563, TC-564, TC-565, TC-566 |
| INT-562 | Must | Given the dashboard | Its own MCP client uses the same merged list for `initialize`, `tools/list` and `tools/call` over stdio and streamable HTTP | Server crash or timeout -> error with the server name; no retry storm | TC-561 |
| FR-561 | Must | Given a server with `kind = "jira"` or `"confluence"` | `POST /api/import` with a Jira key/JQL or Confluence page id/CQL calls the configured tool names (`tools.search`, `tools.get`, overridable per entry) and creates to-dos with `source` and `source_ref` | Duplicate `source_ref` updates instead of duplicating; tool errors -> 502 with the server name; entries are config-only, never auto-connected | TC-562 |
| NFR-560 | Must | Given tests and CI | Only `@modelcontextprotocol/server-everything` or `server-filesystem` via `npx` (`#[ignore]` smoke, opt-in) and an in-repo tiny mock MCP server (default, deterministic) are used; never real Atlassian services | Smoke tests that need `npx` are skipped, not reported as passing, when unavailable | TC-560, TC-562 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-520 | FR-520 | unit | none | `parse_cli` over `serve`, `serve --port 9000`, `serve --bind 127.0.0.1`, `serve --bind 0.0.0.0 --allow-remote`, `--web`, `--web --port 0`, `serve --port`, `serve --port x`, `serve --nope` | Correct `CliAction::Serve/RunWeb` options; malformed -> `Unknown`/error | `src/main.rs` `parse_cli_serve_and_web_flags` |
| TC-521 | FR-521 | unit | none | `generate_token()` twice; `dashboard_url(addr, token)` | >=32 lowercase hex, distinct; URL format exact | `src/web` `token_is_random_hex_and_url_has_token` |
| TC-522 | FR-522 | integration | router with a fixed token | `GET /api/state` without token / wrong token / Bearer / `?token=` | 401 / 401 / 200 / 200; 401 body has no `revision` | `src/web` `api_requires_token` |
| TC-523 | FR-523 | integration | router bound to 127.0.0.1:port | Origin `http://evil.example`; Host `evil.example:port`; Origin `http://localhost:port` | 403 / 403 / 200 | `src/web` `foreign_origin_and_host_are_rejected` |
| TC-524 | FR-524 | unit | none | `check_bind(127.0.0.1)`, `check_bind(::1)`, `check_bind(0.0.0.0, false)`, `check_bind(0.0.0.0, true)` | ok / ok / error / ok with warning text | `src/web` `bind_policy_requires_opt_in_for_non_loopback` |
| TC-525 | FR-525, NFR-521 | unit | `GraphRun` fixture with completed, running, pending nodes; 300-char job | `snapshot_from_run` and serialize | `current_nodes == ["step-2"]`, counts, runner/model, truncated job, stable field names; JSON contains no token | `src/web` `snapshot_reports_current_node_runner_and_counts` |
| TC-526 | FR-526 | unit | hub with injected clock | publish running@1000, completed@4000; publish identical twice; publish new graph id | started 1000, finished 4000, elapsed 3000; revision unchanged on identical; timings reset | `src/web` `hub_tracks_node_timings_and_revisions` |
| TC-527 | FR-527 | integration | server on 127.0.0.1:0 | open `/api/events?token=`; then publish a change | content-type `text/event-stream`; first `event: snapshot` frame within 2s; second frame with higher revision | `src/web` `sse_streams_initial_and_updated_snapshots` |
| TC-528 | FR-528 | unit | temp workspace with `a.json`, newer `b.json`, then malformed newest `c.json` | `FileGraphSource::poll` | uses `b`; malformed `c` keeps `b`; missing dir -> `None` | `src/web` `file_source_picks_latest_valid_run` |
| TC-529 | FR-529, NFR-520 | integration | router | `GET /`, `/assets/app.js`, `/assets/app.css` without token | 200, `Cache-Control: no-store`, CSP header; HTML references `/assets/app.js`; JS references `/api/events`, `/api/state` and `token` | `src/web` `static_page_is_embedded_and_hardened` |
| TC-530 | FR-530 | unit | App test helper with an active graph session running `step-1` on a custom agent | `publish_dashboard` | hub snapshot has source `tui`, graph `current_nodes == ["step-1"]`, the agent busy | `src/app.rs` `app_publishes_graph_state_to_dashboard` |
| TC-531 | FR-520, FR-521 | integration | temp workspace, binary | spawn `dmuxai serve --port 0`, read first stdout URL line, raw HTTP GET `/api/state` with and without token | URL has token; 200 JSON with `"schema":"dmuxai.dashboard/v1"`; 401 without | `tests/web_serve.rs` `serve_prints_token_url_and_serves_state` |
| TC-540 | FR-540 | unit | temp DB file | create, update, reopen store, list; duplicate `(source, source_ref)` upsert | persisted fields and order; upsert updates instead of duplicating; DB opened by `ConversationEngine` afterwards still works | `src/todo.rs` `todo_store_persists_and_orders` |
| TC-541 | FR-541 | integration | router with token + temp store, no command channel | POST, GET, PATCH title/body/status, DELETE; snapshot after each | 201/200/200/204 and `todos[]` in `/api/state` reflects each change with a higher revision | `src/web` `todo_routes_crud_and_snapshot` |
| TC-542 | FR-542 | unit | App test helper with a hub, a store and the command channel | run a to-do as agent, then complete the turn; run as graph and finish the graph; run while busy; run a disallowed runner | prompt sent to the agent with title/body; status running -> done; graph links run_ref; 409; 403 | `src/app.rs` `dashboard_run_todo_with_agent_and_graph` |
| TC-543 | UX-540 | integration | router | GET `/` and `/assets/app.js` | markup has `id="todos"` and the add form; JS calls `/api/todos` and contains the serve-mode view-only hint (`dmuxai --web`) | `src/web` `static_page_has_todo_panel` |
| TC-544 | FR-541 | integration | router | invalid bodies: empty title, 201-char title, non-JSON, unknown id, status `running`, missing token, foreign Origin | 400/400/415/404/400/401/403 | `src/web` `todo_routes_reject_invalid_requests` |
| TC-545 | FR-542 | integration | router without a command channel (`serve`) and with a channel whose App replies busy | POST run | 409 with `requires dmuxai --web`; 409 busy | `src/web` `todo_run_requires_web_mode` |
| TC-560 | INT-560, NFR-560 | unit | user + repo TOML fixtures | merge | override by name, `enabled=false` removal, env refs resolved, missing env disables with warning, no secret in `Debug`/serialized output | slice C |
| TC-561 | INT-562, NFR-560 | integration | in-repo mock MCP server over stdio and over streamable HTTP on 127.0.0.1:0 | initialize, tools/list, tools/call | parsed results; timeout/crash error names the server; optional `#[ignore]` smoke with `npx @modelcontextprotocol/server-everything` | slice C |
| TC-562 | FR-561, NFR-560 | integration | mock MCP with Jira/Confluence fixture tools | import twice; tool error | to-dos created once, `source_ref` set; 502 | slice C |
| TC-563 | INT-561 | unit | merged list fixture | build opencode per-run config | temp JSON has `mcp.<name>` with `type local`/`remote`, existing permission keys preserved | slice C |
| TC-564 | INT-561 | unit | merged list fixture | build Codex argv | `-c mcp_servers.<name>.command=...`/`args`/`url` overrides; no `~/.codex/config.toml` write | slice C |
| TC-565 | INT-561 | unit | custom agent spec with `{mcp_config}` | build argv/env | per-run file with `mcpServers`, `DMUXAI_MCP_CONFIG` set, file removed after turn | slice C |
| TC-566 | INT-561 | unit | Cursor/AGY runner | build turn | behavior per section 6 runner matrix (supported path or one notice) | slice C |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Slice A: no schema change; reads `.dmuxai/graphs/*.json` only. Slice B adds a `todos` table (additive). | Remove the flag/subcommand to roll back | TC-528, TC-540 |
| HTTP API | `dmuxai.dashboard/v1` JSON; additive fields only within v1 | Unknown fields ignored by the page | TC-525 |
| Security / permissions | Loopback default, per-start token, Origin/Host allow-list, opt-in remote bind with warning, token never in payloads; static assets carry no data | Must not weaken provider sandboxes or `allowed_runners` | TC-522..TC-524, TC-529 |
| Secrets | MCP secrets are referenced by env var name; never persisted or rendered | — | TC-560 |
| Company data | No real Jira/Confluence in tests, CI or dogfood prompts | — | NFR-560 |
| Platform | Linux native; Windows via WSL with the Windows browser on `localhost`; native Windows build compiles and passes tests | Unix-only tests are `#[cfg(unix)]` | CI |

### MCP runner matrix (slice C; verified against each CLI's `--help` on 2026-10-17)

| Runner | Native MCP config | Per-run path chosen by DmuxAI | Status |
| --- | --- | --- | --- |
| opencode | `mcp` key in opencode config | Merge into the per-run `OPENCODE_CONFIG` temp file DmuxAI already generates | Supported |
| Codex | `[mcp_servers.<name>]` in `~/.codex/config.toml`; `-c key=value` overrides | Per-run `-c mcp_servers.<name>.*` flags | Supported |
| Custom (`[[agents.custom]]`) | Varies | `{mcp_config}` placeholder + `DMUXAI_MCP_CONFIG` env (Claude/Cursor-style `mcpServers` JSON) | Supported (opt-in per agent) |
| Cursor (`cursor-agent`) | `.cursor/mcp.json` (project) or `~/.cursor/mcp.json`; no per-run flag found | Only if the project `.cursor/mcp.json` does not exist: write it for the run and remove it after; otherwise notice | Partial; cursor is not an allowed runner on the box |
| AGY | `agy mcp add/remove` (user-level store); no per-run flag found | Not propagated automatically; notice with the equivalent `agy mcp add` commands (not executed) | Unsupported per-run (OQ-523) |

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-523 | Resolved (Dongjin, 2026-10-17): print the `agy mcp add` commands only; DmuxAI never modifies AGY's user-level config. | Dongjin | Done |

## 7. Implementation plan

See [PLAN-web-dashboard](../plans/PLAN-web-dashboard.md) and
[TASKS-web-dashboard](../tasks/TASKS-web-dashboard.md).

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | `dmuxai::web` lib module: snapshot types, hub, token, bind policy, router, SSE, file source, embedded assets | `src/web/*.rs`, `src/web/assets/*` | FR-521..FR-529, NFR-520, NFR-521 | TC-521..TC-529 |
| 2 | CLI `serve` / `--web` and server startup | `src/main.rs` | FR-520, FR-521, FR-524 | TC-520, TC-531 |
| 3 | App publishes snapshots | `src/app.rs` | FR-530 | TC-530 |
| 4 | Slice B to-do board | `src/web`, `src/conversation.rs` | FR-540..UX-540 | TC-540..TC-543 |
| 5 | Slice C shared MCP config, runner propagation, dashboard MCP client, import | `src/mcp/*`, `src/settings.rs`, `src/agent/*`, `src/custom_agent.rs`, `src/web` | INT-560..NFR-560 | TC-560..TC-566 |

## 8. Review and test exit criteria

- [ ] All Must requirements of the delivered slice map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy (Linux and Windows CI).
- [ ] External-provider smoke tests are marked run, skipped, or blocked with a
  reason; skipped is never reported as passing.
- [ ] [REVIEW-web-dashboard](../reviews/REVIEW-web-dashboard.md) records commands, result, residual risk, and rollback
  notes.
