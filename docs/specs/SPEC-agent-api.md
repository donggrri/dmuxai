# SPEC: Agent API — headless CLI, HTTP API, MCP server and state-change events

Status: Implemented in v0.2.0 (slice G1)
Owner: DmuxAI
Created: 2026-10-17
Baseline: `1c1f692`
Related: SPEC-web-dashboard (FR-520..FR-542), SPEC-graph-allowed-runners

## 1. Decision summary

Other agents (for example a chat bot that drives DmuxAI for its user) must be
able to use DmuxAI without a terminal UI. This slice adds four surfaces that
share one contract:

- a headless CLI (`dmuxai run`, `dmuxai todo`, `dmuxai status`,
  `dmuxai events`) with `--json` output and stable exit codes;
- a documented, token-authenticated HTTP API on the existing dashboard server
  (`docs/API.md`);
- a stdio MCP server (`dmuxai mcp`) exposing the same operations as tools;
- an append-only state-change event log (`.dmuxai/events.jsonl`) fed by every
  runner, graph node and to-do transition, streamed over SSE and optionally
  pushed to a webhook or a local command.

Nothing here changes how the TUI behaves for a human user.

## 2. Scope

### In scope

- `dmuxai run <prompt> [--agent ID] [--mode ask|plan|implement] [--graph] [--json]`.
- `dmuxai todo add <title> [--body TEXT] [--json]`,
  `dmuxai todo list [--status open|running|done] [--json]`,
  `dmuxai todo run <id> [--agent ID | --graph] [--mode M] [--json]`.
- `dmuxai status [--json]` and `dmuxai events [--since N] [--limit N] [--follow] [--json]`.
- HTTP: `POST /api/prompt`, `GET /api/events/log?since=N`, state-change frames
  on the existing `GET /api/events` SSE stream (with `?since=N` replay), and
  the existing `/api/state` and `/api/todos*` routes documented as a stable API.
- `dmuxai mcp`: MCP over stdio (JSON-RPC 2.0, newline-delimited) with tools
  `todo_add`, `todo_list`, `run`, `status`, `events_since`.
- `[notify]` config: webhook POST and/or a command hook per event, and the
  stall detector (`stall_after_secs`).
- A discovery file so a local agent can find the running dashboard and token.

### Out of scope / non-goals

- Dashboard UI parity with the TUI (prompt box, pickers, palette): see
  SPEC-dashboard-parity.
- Event log rotation or compaction; the file grows until the user deletes it.
- Remote multi-user access control beyond the existing token and
  `--allow-remote` rules.
- Driving a TUI that is already running from `dmuxai run` (the headless run is
  its own process; use the HTTP API to drive a running `dmuxai --web`).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-760 | The snapshot SSE path `/api/events` is kept; the JSON catch-up lives at `/api/events/log` so existing dashboard clients are unaffected. Decided. | DmuxAI | Spec approval |
| OQ-761 | Webhook secrets never live in `dmuxai.toml`; the config names an environment variable (`webhook_secret_env`). Decided. | DmuxAI | Spec approval |
| OQ-762 | A runner is "stalled" when it is busy and DmuxAI has observed no runner output for `stall_after_secs` (default 600, 0 disables). Decided. | DmuxAI | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Event | One JSON line in `.dmuxai/events.jsonl` describing one state transition | Snapshot (full dashboard state) |
| seq | Per-workspace, strictly increasing event number starting at 1 | Snapshot `revision` |
| Headless run | A `dmuxai run` / `todo run` process that runs one prompt without a TUI and exits | `dmuxai serve` (view-only server) |

Event `to_state` vocabulary (`kind` in parentheses):

| kind | Transitions emitted |
| --- | --- |
| `agent` | `pending` (request accepted), `started`, `running` (phase change, `message` = phase), `completed`, `error`, `cancelled`, `stalled` |
| `node` | graph node status changes: `pending`, `ready`, `running`, `waiting_gate`, `completed`, `failed`, `skipped` |
| `graph` | graph run phase changes: `active`, `waiting_gate`, `completed`, `failed` |
| `todo` | `started` (to running), `completed` (to done), `failed` (running to open with an error), `open` (any other change to open) |

Event schema (`v` = 1). Unknown fields must be ignored by readers.

```json
{"v":1,"seq":12,"ts":"2026-10-17T11:22:33.456Z","ts_ms":1792236153456,
 "workspace":"/repo","kind":"node","task":"graph-plan-id","todo":null,
 "node":"implement-1","runner":"agy","model":"gemini-3.8-flash-high",
 "from_state":"ready","to_state":"running","message":null}
```

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-760 | Must | Given any `dmuxai` subcommand in §2, when it is parsed | Then `run`, `todo add/list/run`, `status`, `events`, `mcp` are recognized with the flags in §2 and `--help` lists them | Unknown subcommands or flags print usage to stderr and exit 2; existing `serve`/`--web`/no-arg behavior is unchanged | TC-760 |
| FR-761 | Must | Given `dmuxai run <prompt>` without a TTY | Then the prompt is submitted once (to `--agent`, else the configured default agent; `--graph` forces the graph planner, `--agent` forces single-agent), the process waits for the turn or graph to finish, prints the final agent text (or one JSON object with `--json`) and exits | Exit 0 on success, 1 on agent error or failed graph, 130 on cancel/Ctrl+C (child turns are cancelled first), 3 unknown agent, 5 runner not in `allowed_runners`, 6 internal error | TC-761, TC-762 |
| FR-762 | Must | Given `dmuxai todo add/list/run` | Then add creates a manual to-do and prints it, list prints to-dos (filtered by `--status`), run marks the to-do running, runs it like FR-761 and finishes it as done (success) or open with `last_error` (failure) | Missing to-do → exit 3; already running → exit 4; empty title → exit 2 | TC-763 |
| FR-763 | Must | Given `dmuxai status --json` | Then one JSON object with `workspace`, `version`, `agents` (id, label, allowed), `allowed_runners`, `graph_enabled`, `todos` counts, `last_graph` (id, phase, counts or null), `events` (`path`, `last_seq`) and `dashboard` (discovery data or null) | Unreadable stores are reported as `null` fields plus a `warnings` array, exit 0 | TC-764 |
| FR-764 | Must | Given `dmuxai events [--since N] [--limit N] [--follow] [--json]` | Then events with `seq > N` are printed oldest first, one JSON object per line with `--json`; `--follow` keeps printing new events until interrupted | Missing log → no output, exit 0; malformed lines are skipped | TC-765 |
| FR-765 | Must | Given a running dashboard server and a valid token | Then `POST /api/prompt {text, agent?, mode?}` submits the text exactly like Enter in the TUI (slash commands included) and returns 202 with `{accepted, conversation_id, agent}` | `serve` → 409; empty text → 400; unknown agent → 404; disallowed runner → 403; busy agent → 409; missing token / foreign Host or Origin → 401/403 (unchanged #27 rules) | TC-766 |
| FR-766 | Must | Given events in the log | Then `GET /api/events/log?since=N&limit=M` returns `{events, last_seq}` and the `GET /api/events` SSE stream sends each new event as an `event: state-change` frame; with `?since=N` it first replays events after N | `limit` is capped at 1000; invalid `since` → 400 | TC-767 |
| FR-767 | Must | Given `dmuxai mcp` on stdio | Then it answers `initialize`, `tools/list` and `tools/call` for `todo_add`, `todo_list`, `run`, `status`, `events_since` with JSON text content; `run` performs FR-761 (or FR-762 with `todo_id`) and returns its JSON | Unknown tool or bad arguments → JSON-RPC error or `isError: true` result; notifications get no response; stdout carries only protocol messages | TC-768 |
| FR-768 | Must | Given any agent turn, graph node, graph phase or to-do transition in a TUI, `--web` or headless process (`serve` only reads the log, so it never duplicates events) | Then exactly one event per transition is appended to `<workspace>/.dmuxai/events.jsonl` with the §3 schema and a `seq` one greater than the previous line, even when two DmuxAI processes write the same workspace | Unchanged state publishes no event; a write failure never blocks or crashes the runner | TC-769, TC-770, TC-775 |
| FR-769 | Must | Given a runner is busy and no runner output arrives for `stall_after_secs` | Then one `stalled` event is emitted for that turn; the next output resets the timer | `stall_after_secs = 0` disables it | TC-771 |
| FR-770 | Must | Given `[notify] webhook_url` | Then each event is POSTed as JSON to the URL, with header `secret_header` (default `X-Dmuxai-Secret`) set from env var `webhook_secret_env` when configured | Redirects are not followed; each request has `timeout_secs` (default 5); delivery runs off the UI thread through a bounded queue of 256 events (an event that does not fit is dropped and the drop is logged); failures are appended to `.dmuxai/notify.log` without the secret | TC-772 |
| FR-771 | Must | Given `[notify] command = [...]` | Then the command runs once per event with the event JSON on stdin, in the workspace directory, without a shell unless the user configured one | A failing or slow command (killed after `timeout_secs`) never blocks runners; failure is logged like FR-770 | TC-773 |
| FR-772 | Should | Given `[notify] events = [...]` | Then only events whose `to_state` is listed are delivered to the webhook/command (the log keeps everything) | Unknown names are config warnings | TC-772 |
| FR-773 | Must | Given a dashboard server starts | Then `<workspace>/.dmuxai/web.json` holds `{base_url, port, pid, token}` (owner-only permissions on Unix) and is removed on clean exit; `DMUXAI_WEB_TOKEN` (32..128 chars of `[A-Za-z0-9_-]`) pins the token | An invalid `DMUXAI_WEB_TOKEN` is ignored with a warning and a random token is used | TC-774 |
| NFR-760 | Must | Given any `--json` output | Then stdout carries only the JSON document(s); diagnostics go to stderr | — | TC-761, TC-765 |
| NFR-761 | Must | Given tests for notify | Then they use a local mock HTTP listener and no network | — | TC-772 |
| NFR-762 | Must | Given the headless CLI and MCP `run` | Then they respect `[graph] allowed_runners` exactly like dashboard to-do runs (FR-542) | — | TC-762 |
| UX-760 | Should | Given README | Then a "Calling dmuxai from an agent" section shows the CLI, HTTP and MCP entry points and the event schema | — | Manual review (docs) |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-760 | FR-760 | unit | none | parse each subcommand/flag set, unknown flags | expected `CliAction`; unknown → Unknown (exit 2) | `src/cli.rs` `parse_cli_agent_api_subcommands` |
| TC-761 | FR-761, NFR-760 | integration (binary test unix; in-process `headless_run_*` tests on all platforms) | temp workspace, fake custom agent script that prints JSONL text | `dmuxai run "hi" --agent fake --json` | exit 0, stdout is one JSON object with `ok: true` and the agent text; failing script → exit 1 `ok: false` | `tests/agent_api_cli.rs` `headless_run_json_success_and_failure`, `src/app.rs` `headless_run_returns_agent_text`, `headless_run_reports_agent_error`, `headless_run_cancel_exits_130` |
| TC-762 | FR-761, NFR-762 | integration | config with `allowed_runners = ["fake"]` | `dmuxai run x --agent other --json` and unknown agent | exit 5 / exit 3 with JSON error | `tests/agent_api_cli.rs` `headless_run_respects_allowed_runners` |
| TC-763 | FR-762 | integration (unix) | temp workspace + fake agent | `todo add`, `todo list --json`, `todo run <id> --agent fake`, missing id | to-do created, listed, done after run; missing → exit 3 | `tests/agent_api_cli.rs` `todo_cli_add_list_run` |
| TC-764 | FR-763 | integration | temp workspace | `dmuxai status --json` | JSON with all §FR-763 keys | `tests/agent_api_cli.rs` `status_json_has_contract_keys` |
| TC-765 | FR-764, NFR-760 | unit + integration | log with 3 events and one bad line | `read_since(1)`; `dmuxai events --since 1 --json` | events 2 and 3 only, bad line skipped | `src/events.rs` `read_since_skips_malformed_and_filters`; `tests/agent_api_cli.rs` `events_cli_since_json` |
| TC-766 | FR-765 | integration | router with command channel double | `POST /api/prompt` valid, empty, serve, no token | 202 / 400 / 409 / 401 | `src/web/server.rs` `prompt_route_contract` |
| TC-767 | FR-766 | integration | hub with event sink | append events, `GET /api/events/log?since=1`, SSE with `since` | JSON catch-up; SSE has `state-change` frames | `src/web/server.rs` `events_log_and_sse_state_change` |
| TC-768 | FR-767 | unit | in-memory MCP server with a stub backend | initialize, tools/list, tools/call each tool, unknown tool, notification | correct JSON-RPC responses, no response to notification | `src/mcp_server.rs` `mcp_stdio_protocol_and_tools` |
| TC-769 | FR-768 | unit | tracker fed snapshot sequences | node/graph/todo transitions, repeated identical snapshot | one event per transition, none for repeats | `src/events.rs` `tracker_emits_one_event_per_transition` |
| TC-770 | FR-768 | unit | two `EventLog` handles on one file in threads | concurrent appends | seqs are 1..N unique and increasing; reopen continues from N | `src/events.rs` `event_log_seq_is_monotonic_across_handles` |
| TC-771 | FR-769 | unit | sink with stall 10 s, fake clock | busy, no output 11 s, tick twice; output; tick | one `stalled` event; reset after output | `src/events.rs` `stall_detector_emits_once_and_resets` |
| TC-772 | FR-770, FR-772, NFR-761 | integration | local mock HTTP listener; secret env | deliver events; filter; redirect response; slow server | body is event JSON, secret header set, filtered events skipped, 302 not followed, timeout logged | `src/notify.rs` `webhook_posts_json_with_secret_no_redirect` |
| TC-773 | FR-771 | integration (unix) | command `cat > file` | deliver one event | file contains the event JSON | `src/notify.rs` `command_hook_receives_event_on_stdin` |
| TC-774 | FR-773 | unit | temp workspace | write/remove discovery; env token valid/invalid | file content and mode; invalid env ignored | `src/web/discovery.rs` `discovery_file_and_token_env` |
| TC-775 | FR-768 | unit | App with a hub event sink and a scripted agent | start a turn, stream output, complete it | `started` then `completed` agent events with runner and conversation; the CLI run reports their seq range | `src/app.rs` `app_emits_agent_lifecycle_events` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | New append-only `.dmuxai/events.jsonl` and `.dmuxai/notify.log`, `.dmuxai/web.json`; `.dmuxai/` is already git-ignored. No SQLite schema change. | Deleting the files is safe; seq restarts at 1 | TC-770 |
| Security / permissions | HTTP routes reuse the #27 token, Host and Origin checks; `serve` stays view-only for prompt/run. The discovery file is owner-only on Unix. Webhook secrets come from env, are never logged or written to the event log. | Unchanged #27 behavior | TC-766, TC-772, TC-774 |
| Provider protocol | Headless runs use the same adapters, model policy and allowed-runner checks as the TUI | — | TC-761, TC-762 |
| Platform | Command hook and file-mode checks are Unix-gated in tests; the rest runs on Linux CI | — | TC-773, TC-774 |

## 7. Implementation plan

See [PLAN-agent-api](../plans/PLAN-agent-api.md) and
[TASKS-agent-api](../tasks/TASKS-agent-api.md).

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] The diff has been reviewed against scope and non-goals.
- [x] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets` pass.
- [x] `../reviews/REVIEW-agent-api.md` records commands, result, residual risk, and rollback notes.
