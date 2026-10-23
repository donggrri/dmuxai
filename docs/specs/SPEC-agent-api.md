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

- `dmuxai run <prompt|-> [--agent ID] [--approve-plan] [--allow-sensitive] [--graph] [--json]`.
  `--mode` is not accepted (removed; see [SPEC-acp-runners](SPEC-acp-runners.md) FR-1318 / TC-1492).
- `dmuxai todo add <title> [--body TEXT] [--json]`,
  `dmuxai todo list [--status open|running|done] [--json]`,
  `dmuxai todo run <id> [--agent ID | --graph] [--json]` (same `--mode` rejection as `run`).
- Internal flags (accepted by the parser but not listed in `dmuxai --help`): `--read-only` on
  `run` / `todo run` runs the prompt's turn with the read-only turn policy; the MCP `run`
  tool passes it for legacy `mode` `ask`/`plan` (FR-767).
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

A direct `--agent` run (without `--graph`) runs one agent turn directly and emits only `agent` transitions (no `graph` or `node` events).

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
| FR-760 | Must | Given any `dmuxai` subcommand in §2, when it is parsed | Then `run`, `todo add/list/run`, `status`, `events`, `mcp` are recognized with the user-facing flags in §2; `dmuxai --help` lists those flags for each subcommand (not internal `--read-only`) | Unknown subcommands or flags print usage to stderr and exit 2; `--mode` on `run` / `todo run` is a usage error (exit 2, message that `--mode` was removed); existing `serve`/`--web`/no-arg behavior is unchanged | TC-760, TC-1492 |
| FR-761 | Must | Given `dmuxai run <prompt>` without a TTY | Then the prompt is submitted once (to `--agent` directly without building a graph, else the configured default agent via the judge path; `--graph` is accepted as a no-op for legacy scripts; `--read-only` runs with read-only turn policy; `--approve-plan` auto-approves a plan gate), the process waits for the turn or graph to finish, prints the final agent text (or one JSON object with `--json`) and exits | Exit 0 on success, 1 on agent error or failed graph, 130 on cancel/Ctrl+C (child turns are cancelled first), 3 unknown agent, 5 runner not in `allowed_runners`, 6 internal error; `--mode` → exit 2 (FR-1318) | TC-761, TC-762, TC-1492 |
| FR-762 | Must | Given `dmuxai todo add/list/run` | Then add creates a manual to-do and prints it, list prints to-dos (filtered by `--status`), run marks the to-do running, runs it like FR-761 and finishes it as done (success) or open with `last_error` (failure) | Missing to-do → exit 3; already running → exit 4; empty title → exit 2 | TC-763 |
| FR-763 | Must | Given `dmuxai status --json` | Then one JSON object with `workspace`, `version`, `agents` (id, label, allowed), `allowed_runners`, `graph_enabled`, `todos` counts, `last_graph` (id, phase, counts or null), `events` (`path`, `last_seq`) and `dashboard` (discovery data or null) | Unreadable stores are reported as `null` fields plus a `warnings` array, exit 0 | TC-764 |
| FR-764 | Must | Given `dmuxai events [--since N] [--limit N] [--follow] [--json]` | Then events with `seq > N` are printed oldest first, one JSON object per line with `--json`; `--follow` keeps printing new events until interrupted | Missing log → no output, exit 0; malformed lines are skipped | TC-765 |
| FR-765 | Must | Given a running dashboard server and a valid token | Then `POST /api/prompt {text, agent?, mode?}` submits the text exactly like Enter in the TUI (slash commands included) and returns 202 with `{accepted, conversation_id, agent}` | `serve` → 409; empty text → 400; unknown agent → 404; disallowed runner → 403; busy agent → 409; missing token / foreign Host or Origin → 401/403 (unchanged #27 rules) | TC-766 |
| FR-766 | Must | Given events in the log | Then `GET /api/events/log?since=N&limit=M` returns `{events, last_seq}` and the `GET /api/events` SSE stream sends each new event as an `event: state-change` frame; with `?since=N` it first replays events after N | `limit` is capped at 1000; invalid `since` → 400 | TC-767 |
| FR-767 | Must | Given `dmuxai mcp` on stdio | Then it answers `initialize`, `tools/list` and `tools/call` for `todo_add`, `todo_list`, `run`, `status`, `events_since` with JSON text content; `run` performs FR-761 (or FR-762 with `todo_id`) and returns its JSON | Optional legacy `mode` on `run`: `ask` or `plan` → child `dmuxai run … --read-only` (never `--mode`); `implement` or `agent` → normal write-capable run; omit → write-capable; any other value → tool error; unknown tool or bad arguments → JSON-RPC error or `isError: true` result; notifications get no response; stdout carries only protocol messages | TC-768 |
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
| FR-1650 | Must | Given a running `dmuxai --web` dashboard and `GET /api/state` (or an SSE `snapshot` frame) | Then the payload includes a top-level `turn` object: `{ "state", "conversation_id", "graph_id"?, "since_ms" }` where `state` is one of `idle`, `running`, `awaiting_approval`, `needs_input`, `done`, `failed`. `awaiting_approval` when the active conversation's graph is in phase `awaiting_approval` or holds a `pending_replan_plan` awaiting decision; `needs_input` when the graph is in `waiting_gate` (sensitive step gate); otherwise `state` reflects the active agent turn (running) or the most recent terminal agent run | Absent graph and no active turn → `idle` with the current `conversation_id` | TC-1650 |
| FR-1651 | Must | Given `awaiting_approval` for the active conversation's graph | Then the planning runner's `agents[].run.state` (and the matching `session.agents[].run.state`) is `awaiting_approval`, not `done`, until the plan is approved, edited, or rejected | Other agents keep their own run states | TC-1651 |
| FR-1652 | Must | Given a plan turn completes into `awaiting_approval` | Then the session transcript gains a system line `Plan waiting for approval — approve or reject in the dashboard (or POST the graph approve command)` after the `Planning with …` line, so the transcript tail is never only `Planning with …` once planning finished | Replan revisions that already pushed a notice still include this line when the gate opens | TC-1652 |
| FR-1653 | Must | Given `turn.state` is `awaiting_approval` on a dashboard with the chat session | Then the header run-status pill shows `Waiting for approval` with readable contrast in light and dark themes | View-only `serve` uses the same pill when a stored run awaits approval | TC-1653 |
| FR-1654 | Should | Given `docs/API.md` and README Agent API section | Then the `turn` object and `awaiting_approval` / `needs_input` states are documented on `GET /api/state` | — | TC-1654 |
| FR-1810 | Must | Given a headless `dmuxai run` process exits | Then the exit code is one of 0 success, 1 agent error or failed/cancelled graph, 2 usage, 3 unknown agent, 4 busy/conflict, 5 runner not in `allowed_runners`, 6 internal error, 7 plan approval or sensitive gate, 130 cancelled | A graph stopped by a cancelled step exits 130 (FR-1416) | TC-1810 |
| FR-1811 | Must | Given `dmuxai todo run <id>` | Then it uses the same exit codes as FR-1810, plus 3 for a missing to-do, 4 for an already running to-do, and 2 for an empty `todo add` title | `todo add`/`list` exit 2 on usage errors | TC-1811 |
| FR-1812 | Must | Given `dmuxai status` or `dmuxai events` | Then `status` prints its JSON and exits 0 even when stores are unreadable (warnings recorded); `events` prints available events and exits 0 when the log is missing | malformed event lines are skipped; unreadable workspace → 2 | TC-1812 |
| FR-1813 | Must | Given `--json` on `run` / `todo run` | Then stdout contains exactly one JSON object with `ok`, `exit_code`, `conversation_id`, `agent`, `graph_id` (`null` for direct `--agent` runs without `--graph`), `text`, `messages`, `error`, `todo`, `triage`, `plan`, and `events` (`{first_seq,last_seq}` or nulls) | error runs use the same object with `ok: false` | TC-1813 |
| FR-1814 | Must | Given `--agent ID` naming an unknown runner | Then the run exits 3 with `error: "unknown agent 'ID'"` and no turn starts | matching is case-insensitive after trim | TC-1814 |
| FR-1815 | Must | Given an empty prompt or empty stdin for `run -` | Then parsing or reading stdin fails with exit 2 and a one-line message | whitespace-only prompts are empty | TC-1815 |
| FR-1816 | Must | Given an unknown flag or subcommand | Then usage is printed to stderr and the process exits 2 | `--mode` is also exit 2 with a removed-mode message | TC-1816 |
| FR-1817 | Must | Given a flag that requires a value but has none or a flag-like value | Then parsing fails with exit 2 and `<flag> needs a value` | `value()` never consumes another flag | TC-1817 |
| FR-1818 | Must | Given a graph run reaches `AwaitingApproval` without `--approve-plan` | Then headless exits 7 with the plan text and cancels the run | `--approve-plan` is required for complex plans | TC-1818 |
| FR-1819 | Must | Given a sensitive gate (`WaitingGate`) without `--allow-sensitive` (or `--agent` + `--approve-plan`) | Then headless exits 7 | `--allow-sensitive` auto-confirms every gate | TC-1819 |
| FR-1820 | Must | Given the selected runner's binary cannot be spawned | Then the turn emits an agent error and the run exits 1 | the error names `failed to start <program>` | TC-1820 |
| FR-1821 | Must | Given `--graph` on `dmuxai run` | Then the flag is accepted and the run follows the normal judge path (no-op for legacy scripts) | `--graph` with `--agent` is rejected at parse time | TC-1821 |
| FR-1822 | Must | Given the same feature in headless vs dashboard mode | Then headless uses CLI flags for gates/approval, has no workspace switch or live prompt API, and persists conversations to the same per-repo DB; `serve` is view-only and never runs agents | dashboard uses `/graph continue` and `POST /api/prompt` | TC-1822 |
| FR-1823 | Must | Given an agent error mid-turn | Then the run exits 1 with the last system error or `run failed` | cancellation exits 130 instead | TC-1823 |
| FR-1824 | Must | Given `--read-only` | Then the turn runs under the read-only turn policy | internal flag; not listed in `--help` | TC-1824 |
| FR-1761 | Must | Given headless early-return paths in `App::run_headless`, `headless_idle_action`, `headless_outcome_exit_7` | Then failures are built via `HeadlessOutcome::failure(...)` with magic exit codes replaced by `dmuxai::agent_api::exit::*` constants; JSON output stays byte-for-byte identical | no logic or field value changes | TC-1761 |
| FR-1762 | Must | Given the `SnapshotSource` enum | Then a new `App` variant serializes as `"app"` and round-trips; it behaves like `Tui` (live, writable), not view-only like `Files` | — | TC-1762 |
| FR-1763 | Must | Given a headless `dmuxai run` or `dmuxai --web` dashboard hub | Then the snapshot `source` is `"app"` | real interactive TUI keeps `tui`; `serve` keeps `files` | TC-1763 |
| FR-1764 | Must | Given a headless `dmuxai run` with explicit `--agent` and without `--graph` | Then the turn runs directly on the agent without creating a graph or graph session, `graph_id` is null, and on success the process exits 0 with the agent text | no `graph` or `node` events or run files are created | TC-1764 |
| FR-1765 | Must | Given an explicit `--agent` headless direct run where the agent fails | Then the process exits 1 with `ok: false` and the error is reported once directly from the agent | no "Graph failed" or "graph node" messages or events appear in output | TC-1765 |
| FR-1766 | Must | Given an explicit `--agent` headless direct run | Then the JSON output retains `triage` with `route: simple`, `reason: "override"`, `source: override`, `runner`, and the resolved `model`, while `plan` is absent | explicit `--model` overrides the runner model default in triage | TC-1766 |
| FR-1767 | Must | Given an explicit `--agent` headless direct run with a prompt matching a sensitive keyword | Then without `--allow-sensitive` or `--approve-plan`, no turn starts and headless exits 7 (`APPROVAL_REQUIRED`) with the sensitive gate note; with `--approve-plan` or `--allow-sensitive`, the direct turn runs | keeps the sensitive gate rule without constructing a graph | TC-1767 |
| FR-1768 | Must | Given an explicit `--agent` headless CLI run | Then `.dmuxai/events.jsonl` records only `agent` lifecycle events and no `graph` or `node` events | `graph_id` remains null in JSON output | TC-1768 |

### Code-vs-spec mismatches

| ID | Spec says | Code does (file:function) | Proposed resolution |
| --- | --- | --- | --- |
| OQ-1810 | `dmuxai run --graph` is a no-op for legacy scripts | `src/cli.rs` `run_headless` passes `graph` into `HeadlessRequest` and `todo run --graph` uses `RunMode::Graph` | Clarify that `--graph` is a no-op only for `run`; `todo run --graph` selects graph mode |

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
| TC-1650 | FR-1650 | integration | `dmuxai --web` hub with a graph run in `awaiting_approval` | `GET /api/state` or hub snapshot after `publish_dashboard` | `turn.state` is `awaiting_approval` with `graph_id` set | `src/web/server.rs` `tc_1650_state_includes_turn_awaiting_approval`; `src/app.rs` `tc_1650_dashboard_turn_awaiting_approval` |
| TC-1651 | FR-1651 | unit (app) | Plan gate harness at `awaiting_approval` with planner run `done` | `publish_dashboard` | Planner agent `run.state` is `awaiting_approval` | `src/app.rs` `tc_1651_agent_run_awaiting_approval_not_done` |
| TC-1652 | FR-1652 | unit (app) | Complete a valid plan turn | Inspect `session_view` transcript | Last system line is the plan-waiting message after `Planning with …` | `src/app.rs` `tc_1652_transcript_plan_waiting_line` |
| TC-1653 | FR-1653 | unit (assets) | `app.css` and `app.js` | Static review | `.run-status-awaiting-approval` exists; `updateRunStatus` handles `turn.state === 'awaiting_approval'` | `src/web/assets.rs` `tc_1653_waiting_for_approval_status_assets` |
| TC-1654 | FR-1654 | manual | `docs/API.md`, README | Read-through | `turn` documented on `/api/state` | Manual review |
| TC-1810 | FR-1810 | unit (app) | scripted agent success/error/cancel | run headless | exit codes 0/1/130 and 3/5/6 boundaries | `src/app.rs` `headless_run_returns_agent_text`, `headless_run_reports_agent_error`, `headless_run_cancel_exits_130`, `headless_run_rejects_unknown_and_disallowed_agent` |
| TC-1811 | FR-1811 | integration (CLI) | temp workspace + fake agent | `todo add/run/list` | exit codes for missing/running/busy to-dos | `tests/agent_api_cli.rs` `todo_cli_add_list_run` |
| TC-1812 | FR-1812 | integration | temp workspace | `status --json`, `events --since` | exit 0 on missing log; malformed lines skipped | `tests/agent_api_cli.rs` `status_json_has_contract_keys`, `events_cli_since_json` |
| TC-1813 | FR-1813 | integration | fake agent | `dmuxai run ... --json` | one JSON object with all documented fields | `tests/agent_api_cli.rs` `headless_run_json_success_and_failure` |
| TC-1814 | FR-1814 | unit (app) | `test_app()` | unknown agent request | exit 3, JSON error | `src/app.rs` `headless_run_rejects_unknown_and_disallowed_agent` |
| TC-1815 | FR-1815 | unit | parse/read_prompt | empty prompt/stdin | exit 2 | planned — no automated test yet (Stream B) |
| TC-1816 | FR-1816 | unit | parse | unknown flag/subcommand | exit 2 | `src/cli.rs` `parse_cli_agent_api_subcommands` |
| TC-1817 | FR-1817 | unit | parse | flag missing value | exit 2 | `src/cli.rs` `parse_cli_agent_api_subcommands` |
| TC-1818 | FR-1818 | integration (CLI) | fake agent plan | `dmuxai run ...` without `--approve-plan` | exit 7 with plan JSON | `tests/cli.rs` `run_complex_needs_approve_plan` |
| TC-1819 | FR-1819 | unit (app) | scripted agent sensitive keyword | headless run | exit 7 without flag | planned — no automated test yet (Stream B) |
| TC-1820 | FR-1820 | unit | custom agent missing binary | run turn | agent error, exit 1 | planned — no automated test yet (Stream B) |
| TC-1821 | FR-1821 | unit | parse | `--graph` flag | accepted | `src/cli.rs` `parse_cli_agent_api_subcommands` |
| TC-1822 | FR-1822 | integration | headless vs `--web` | compare gates/persistence | documented behavior holds | planned — no automated test yet (Stream B) |
| TC-1823 | FR-1823 | unit (app) | scripted agent error | run headless | exit 1 | `src/app.rs` `headless_run_reports_agent_error` |
| TC-1824 | FR-1824 | unit (app) | `--read-only` | run headless | read-only policy | planned — no automated test yet (Stream B) |
| TC-1761 | FR-1761 | unit (app) | `test_app()` | construct `HeadlessOutcome::failure(...)` | `ok=false`, given exit code, error text, all other optionals `None`; serialized JSON matches the old literal keys | `src/app.rs` `tc_1761_headless_outcome_failure_builder_defaults` |
| TC-1762 | FR-1762 | unit | `SnapshotSource::App` | serde round-trip | `"app"` | `src/web/snapshot.rs` `tc_1762_snapshot_source_app_serializes_as_app` |
| TC-1763 | FR-1763 | unit | `DashboardHub::new(SnapshotSource::App, ...)` | `hub.snapshot()` | source is `App` and serializes `"app"` | `src/app.rs` `tc_1763_headless_dashboard_source_is_app` |
| TC-1764 | FR-1764 | unit (app) | scripted agent with hub event sink | `run_headless` with explicit agent | exit 0, text returned, `graph_id` None, `last_graph_run` None, event log has `agent` events but no `graph`/`node` events | `src/app.rs` `tc_1764_headless_explicit_agent_runs_without_graph` |
| TC-1765 | FR-1765 | unit (app) | scripted agent returning error | `run_headless` with explicit agent | exit 1, `ok: false`, agent error reported once, no message contains "Graph failed" or "graph node" | `src/app.rs` `tc_1765_headless_explicit_agent_error_reported_once` |
| TC-1766 | FR-1766 | unit (app) | scripted agent | `run_headless` with explicit agent | `outcome.triage` has route simple, reason override, source override, runner, resolved model, and plan is None | `src/app.rs` `tc_1766_headless_explicit_agent_keeps_override_triage` |
| TC-1767 | FR-1767 | unit (app) | scripted agent, sensitive prompt | `run_headless` with explicit agent without flags, then with `approve_plan` | exit 7 and no turn started; with `approve_plan` turn runs to exit 0 | `src/app.rs` `tc_1767_headless_direct_sensitive_prompt_needs_flag` |
| TC-1768 | FR-1768 | integration (CLI unix) | fake agent sandbox | `dmuxai run hi --agent fake --json` | exit 0, `graph_id` null, events contain `agent` events and no `graph`/`node` events | `tests/agent_api_cli.rs` `tc_1768_cli_run_agent_emits_no_graph_events` |

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
