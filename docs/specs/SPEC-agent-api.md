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

- `dmuxai run <prompt|-> [--agent ID] [--model M] [--approve-plan] [--allow-sensitive] [--graph] [--progress] [--json]`.
  `--mode` is not accepted (removed; see [SPEC-acp-runners](SPEC-acp-runners.md) FR-1318 / TC-1492).
- `dmuxai todo add <title> [--body TEXT] [--json]`,
  `dmuxai todo list [--status open|running|done] [--json]`,
  `dmuxai todo run <id> [--agent ID | --graph] [--model M] [--progress] [--json]` (same `--mode` rejection as `run`).
- Internal flags (accepted by the parser but not listed in `dmuxai --help`): `--read-only` on
  `run` / `todo run` runs the prompt's turn with the read-only turn policy. The MCP
  `run` tool has no read-only field; its deprecated `mode` argument is rejected
  (FR-2511).
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
| `activity` | runner activity and progress heartbeats: `tool` (ToolProgress state change), `file_edit` (file-edit Activity item), `output` (text delta heartbeat at most once every 30s) |
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

Progress line schema (emitted to STDERR when `--progress` is enabled, one JSON object per line):

```json
{"type":"progress","ts_ms":1792236153456,"agent":"cursor","kind":"started","summary":"started"}
```

- `type`: constant `"progress"`
- `ts_ms`: epoch milliseconds (`u64`)
- `agent`: runner id string
- `kind`: one of `started`, `tool`, `file_edit`, `activity`, `heartbeat`, `status`, `notice`, `text`, `completed`, `error`, `cancelled`
- `summary`: single-line string at most 160 characters (cut with `…` if longer; newlines replaced by spaces). `heartbeat` summaries look like `running <turn seconds>s; last output <seconds>s ago (type=<type or event>)`; `file_edit` summaries carry a workspace-relative path and never file content.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-760 | Must | Given any `dmuxai` subcommand in §2, when it is parsed | Then `run`, `todo add/list/run`, `status`, `events`, `mcp` are recognized with the user-facing flags in §2; `dmuxai --help` lists those flags for each subcommand (not internal `--read-only`) | Unknown subcommands or flags print usage to stderr and exit 2; `--mode` on `run` / `todo run` is a usage error (exit 2, message that `--mode` was removed); existing `serve`/`--web`/no-arg behavior is unchanged | TC-760, TC-1492 |
| FR-761 | Must | Given `dmuxai run <prompt>` without a TTY | Then the prompt is submitted once (to `--agent` directly without building a graph, else the configured default agent via the judge path; `--graph` is accepted as a no-op for legacy scripts; `--read-only` runs with read-only turn policy; `--approve-plan` auto-approves a plan gate), the process waits for the turn or graph to finish, prints the final agent text (or one JSON object with `--json`) and exits | Exit 0 on success, 1 on agent error or failed graph, 130 on cancel/Ctrl+C/SIGTERM/SIGHUP (child turns are cancelled first; SIGTERM/SIGHUP behave like Ctrl+C, see SPEC-process-supervisor FR-1790), 3 unknown agent, 5 runner not in `allowed_runners`, 6 internal error; `--mode` → exit 2 (FR-1318) | TC-761, TC-762, TC-1492 |
| FR-762 | Must | Given `dmuxai todo add/list/run` | Then add creates a manual to-do and prints it, list prints to-dos (filtered by `--status`), run marks the to-do running, runs it like FR-761 and finishes it as done (success) or open with `last_error` (failure) | Missing to-do → exit 3; already running → exit 4; empty title → exit 2 | TC-763 |
| FR-763 | Must | Given `dmuxai status --json` | Then one JSON object with `workspace`, `version`, `agents` (id, label, allowed), `allowed_runners`, `graph_enabled`, `todos` counts, `last_graph` (id, phase, counts or null), `events` (`path`, `last_seq`) and `dashboard` (discovery data or null) | Unreadable stores are reported as `null` fields plus a `warnings` array, exit 0 | TC-764 |
| FR-764 | Must | Given `dmuxai events [--since N] [--limit N] [--follow] [--json]` | Then events with `seq > N` are printed oldest first, one JSON object per line with `--json`; `--follow` keeps printing new events until interrupted | Missing log → no output, exit 0; malformed lines are skipped | TC-765 |
| FR-765 | Must | Given a running dashboard server and a valid token | Then `POST /api/prompt {text, agent?, mode?}` submits the text exactly like Enter in the TUI (slash commands included) and returns 202 with `{accepted, conversation_id, agent}` | `serve` → 409; empty text → 400; unknown agent → 404; disallowed runner → 403; busy agent → 409; missing token / foreign Host or Origin → 401/403 (unchanged #27 rules) | TC-766 |
| FR-766 | Must | Given events in the log | Then `GET /api/events/log?since=N&limit=M` returns `{events, last_seq}` and the `GET /api/events` SSE stream sends each new event as an `event: state-change` frame; with `?since=N` it first replays events after N | `limit` is capped at 1000; invalid `since` → 400 | TC-767 |
| FR-767 | Must | Given `dmuxai mcp` on stdio | Then it answers `initialize`, `tools/list` and `tools/call` for `todo_add`, `todo_list`, `run`, `status`, `events_since` with JSON text content; `run` performs FR-761 (or FR-762 with `todo_id`) and returns its JSON | `run` accepts `prompt`, `agent`, `model`, `approve_plan`, `allow_sensitive`, `graph` and `todo_id` (see [SPEC-mcp-run-parity](SPEC-mcp-run-parity.md) FR-2509..FR-2512): `model` requires `agent` and conflicts with `graph`; `approve_plan` and `allow_sensitive` forward their CLI flags; exit code `7` is an `isError` result with `approval_required: true`. The deprecated `mode` argument is rejected (FR-2511). Unknown tool or bad arguments → JSON-RPC error or `isError: true` result; notifications get no response; stdout carries only protocol messages | TC-768, TC-2509, TC-2510, TC-2511 |
| FR-768 | Must | Given any agent turn, graph node, graph phase or to-do transition in a TUI, `--web` or headless process (`serve` only reads the log, so it never duplicates events) | Then exactly one event per transition is appended to `<workspace>/.dmuxai/events.jsonl` with the §3 schema and a `seq` one greater than the previous line, even when two DmuxAI processes write the same workspace | Unchanged state publishes no event; a write failure never blocks or crashes the runner | TC-769, TC-770, TC-775 |
| FR-769 | Must | Given a runner is busy and no runner output arrives for `stall_after_secs` | Then one `stalled` event is emitted for that turn; the next output resets the timer | `stall_after_secs = 0` disables it | TC-771 |
| FR-770 | Must | Given `[notify] webhook_url` | Then each event is POSTed as JSON to the URL, with header `secret_header` (default `X-Dmuxai-Secret`) set from env var `webhook_secret_env` when configured (when configured in a workspace `dmuxai.toml`, requires workspace trust via SPEC-workspace-config-trust) | Redirects are not followed; each request has `timeout_secs` (default 5); delivery runs off the UI thread through a bounded queue of 256 events (an event that does not fit is dropped and the drop is logged); failures are appended to `.dmuxai/notify.log` without the secret | TC-772 |
| FR-771 | Must | Given `[notify] command = [...]` | Then the command runs once per event with the event JSON on stdin, in the workspace directory, without a shell unless the user configured one (in a workspace `dmuxai.toml`, requires workspace trust via SPEC-workspace-config-trust) | A failing or slow command (killed after `timeout_secs`) never blocks runners; failure is logged like FR-770 | TC-773 |
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
| FR-1810 | Must | Given a headless `dmuxai run` process exits | Then the exit code is one of 0 success, 1 agent error or failed/cancelled graph, 2 usage, 3 unknown agent, 4 busy/conflict, 5 runner not in `allowed_runners`, 6 internal error, 7 plan approval or sensitive gate or an agent that stopped at its own plan review (FR-2121), 130 cancelled | A graph stopped by a cancelled step exits 130 (FR-1416) | TC-1810 |
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
| FR-1824 | Must | Given `--read-only` | Then the turn reaches the selected runner with `TurnPolicy::ReadOnly`, including when `--agent` and/or `--model` are also given and when the default/triage or graph path routes it; write runs without `--read-only` stay write-capable | internal flag; not listed in `--help` | TC-1824 |
| See FR-2540 in [SPEC-headless-read-only-write-guard](./SPEC-headless-read-only-write-guard.md) | Must | Given a headless read-only run in a git worktree | Then DmuxAI compares a content-aware worktree fingerprint (porcelain v2 plus per-entry staged blob hash and worktree content summary, excluding `.dmuxai/`) before submit and after the run; if the worktree changed and the run would otherwise succeed, the outcome is `ok: false` with `error_kind` `policy` and exit 1 | no snapshot when not a git repo; write-capable runs unchanged | TC-2540, TC-2541, TC-2542; [SPEC-headless-read-only-write-guard.md](./SPEC-headless-read-only-write-guard.md) |
| FR-1761 | Must | Given headless early-return paths in `App::run_headless`, `headless_idle_action`, `headless_outcome_exit_7` | Then failures are built via `HeadlessOutcome::failure(...)` with magic exit codes replaced by `dmuxai::agent_api::exit::*` constants; JSON output stays byte-for-byte identical | no logic or field value changes | TC-1761 |
| FR-1762 | Must | Given the `SnapshotSource` enum | Then a new `App` variant serializes as `"app"` and round-trips; it behaves like `Tui` (live, writable), not view-only like `Files` | — | TC-1762 |
| FR-1763 | Must | Given a headless `dmuxai run` or `dmuxai --web` dashboard hub | Then the snapshot `source` is `"app"` | real interactive TUI keeps `tui`; `serve` keeps `files` | TC-1763 |
| FR-1764 | Must | Given a headless `dmuxai run` with explicit `--agent` and without `--graph` | Then the turn runs directly on the agent without creating a graph or graph session, `graph_id` is null, and on success the process exits 0 with the agent text | no `graph` or `node` events or run files are created | TC-1764 |
| FR-1765 | Must | Given an explicit `--agent` headless direct run where the agent fails | Then the process exits 1 with `ok: false` and the error is reported once directly from the agent | no "Graph failed" or "graph node" messages or events appear in output | TC-1765 |
| FR-1766 | Must | Given an explicit `--agent` headless direct run | Then the JSON output retains `triage` with `route: simple`, `reason: "override"`, `source: override`, `runner`, and the resolved `model`, while `plan` is absent | explicit `--model` overrides the runner model default in triage | TC-1766 |
| FR-1767 | Must | Given an explicit `--agent` headless direct run with a prompt matching a sensitive keyword | Then without `--allow-sensitive` or `--approve-plan`, no turn starts and headless exits 7 (`APPROVAL_REQUIRED`) with the sensitive gate note; with `--approve-plan` or `--allow-sensitive`, the direct turn runs | keeps the sensitive gate rule without constructing a graph | TC-1767 |
| FR-1768 | Must | Given an explicit `--agent` headless CLI run | Then `.dmuxai/events.jsonl` records only `agent` lifecycle events and no `graph` or `node` events | `graph_id` remains null in JSON output | TC-1768 |
| FR-1769 | Must | Given `dmuxai run` / `todo run` flags parsed by the table-driven parser | Then `--agent`, `--model`, `--graph`, `--read-only`, `--approve-plan`, `--allow-sensitive`, `--json`, and `--progress` are recognized and `RunOptions.progress` is true iff `--progress` is present | unknown flags, missing values, and `--` handling keep their existing messages | TC-1769 |
| FR-1770 | Must | Given the table-driven flag parser | Then every existing error message (`unexpected argument`, `<flag> needs a value`, removed `--mode`, agent/graph conflict, model rules, empty prompt, `todo add` title, numeric limits, etc.) is still produced for the same input | boundary cases in TC-1770 | TC-1770 |
| FR-1771 | Must | Given `dmuxai --help` | Then the usage text contains `--progress` and does not contain `--read-only` | help exits 0 | TC-1771 |
| FR-1772 | Must | Given an `AgentEvent` mapped to a progress line | Then the line shape matches `{"type":"progress","ts_ms":<u64>,"agent":"<runner id>","kind":"<kind>","summary":"<text>"}` and summary is single-line, at most 160 characters cut with `…` | multiline summaries replace newlines with spaces; non-progress events return None | TC-1772 |
| FR-1773 | Must | Given streaming `TextDelta` events for an agent | Then at most one `text` line per agent is emitted every 2 seconds (summary = last 160 chars received since previous text line, newlines replaced with spaces), and pending text is flushed before `completed`, `error`, or `cancelled` | empty delta text does not emit an extra line | TC-1773 |
| FR-1774 | Must | Given a headless run with `--progress` | Then live progress lines are written to stderr in order as events occur, while stdout carries only the final output; without `--progress` stderr receives no progress lines | errors and cancellations still flush pending progress lines | TC-1774 |
| FR-1775 | Must | Given an active turn in a process with an event hub | Then activity events (`kind: "activity"`) are emitted to the event log: `tool` on `ToolProgress` state changes, `file_edit` on file-edit `ActivityItem`s, and an `output` heartbeat at most once every 30 seconds for `TextDelta` | unchanged tool state and frequent text deltas do not emit duplicate activity events | TC-1775 |
| FR-1776 | Must | Given `dmuxai status --json` | Then each `agents[].run` in `running` state includes `last_output_at_ms` equal to the latest `agent` or `activity` event `ts_ms` for that runner since the run started (equal to `started_at_ms` when no output yet) | stalled and pending transitions do not reset `last_output_at_ms`; terminal states preserve it | TC-1776 |
| FR-1777 | Must | Given `dmuxai run <prompt> --agent ID --json --progress` | Then stderr contains at least a `started` and a `completed` progress JSON line while stdout is exactly one JSON object; without `--progress` stderr contains no progress lines | exit code and JSON contract match non-progress runs | TC-1777 |
| FR-1778 | Must | Given an explicit `--agent` headless direct run | Then when `settings.stage_skills.chat` is true the prompt sent to the agent is prefixed with the implement stage skill prefix, and when it is false the prompt is sent unchanged | single 1-node chat turns apply chat stage skills only when enabled | TC-1778 |
| FR-2120 | Must | Given two `dmuxai` processes use the same workspace conversation database | Then opening the database retries on SQLITE_BUSY/SQLITE_LOCKED with a bounded backoff (about 20 s), takes no write transaction while the schema is current, and a database that stays locked ends the headless run with exit 4 and a message that asks the caller to retry | no exclusive file lock is taken; only SQLite's own locking applies; a database that is released inside the budget is opened normally; errors other than busy keep their previous exit code 6; the same mapping covers the session registration and turn recovery that follow the open | TC-2120 |
| FR-2140 | Must | Given the conversation database is locked by another process *during* a turn, after it opened fine | Then saving the agent's reply retries on SQLITE_BUSY/SQLITE_LOCKED with the same bounded backoff as the open phase (about 20 s of waiting per turn; the saves of a turn share that budget, and a spent budget only probes the database without waiting), and a save that still fails ends the headless run with `ok: false`, exit 4 (busy database, `error_kind` `conflict`) or exit 6 (any other database error, `error_kind` `internal`) | the reply text stays in the JSON `text` and in the transcript, the `error` says the reply could not be saved, the system note "Could not save &lt;agent&gt; output" is kept once per turn, the interactive TUI shows the note without crashing, the retry never blocks longer than the budget, a database that is released after the budget still receives the rest of the reply, and once the budget is spent shutdown closes the session once with a short busy timeout so it does not wait the default connection timeout twice | TC-2140 |
| FR-2430 | Must | Given a custom JSONL agent (`[[agents.custom]]` with `output = "jsonl"`) and a stdout line that matches `tool_when = { path = "...", equals = "..." }`, or the built-in opencode preset (`opencode run --format json` lines whose `type` is `tool_use` or whose `part.type` is `tool`), when the line is read | Then an `AgentEvent::Activity` tool item is emitted so `--progress` prints a `kind:"tool"` line whose summary is the tool name (`tool_name_path`) plus the first line of its title or safe input (`tool_title_path`), cut and flattened like every other summary; both started and completed states are accepted | lines that do not match and unknown JSON shapes emit nothing and never crash; other JSONL agents can supply the same mapping in their spec | TC-2430 |
| FR-2431 | Must | Given a tool event for an edit, write, patch, multi-edit or replace tool that carries a file path | Then `--progress` prints `kind:"file_edit"` with the workspace-relative path in the summary | never includes the old/new content, the raw tool output, or an absolute path when the file is inside the workspace; a tool event without a file path keeps `kind:"tool"` | TC-2431 |
| FR-2432 | Must | Given a headless turn is running and `--progress` is on | Then a `kind:"heartbeat"` line is written to stderr about every 30 s (fixed constant) with a summary like `running 150s; last output 12s ago (type=step_start)`: seconds since turn start, seconds since the last child stdout line, and that line's `type` or `event` value when available | works for every runner (built-in and custom); no heartbeat is emitted after `completed`, `error`, or `cancelled`; heartbeats are not stored in the transcript and do not reset idle or stall watchdogs | TC-2432 |
| FR-2433 | Must | Given any progress summary produced from a tool event | Then the summary contains only a short tool name and the first line of a safe title/input field, with the same 160-character cut and newline flattening as FR-1772 | file contents, environment values, tokens, full tool outputs, and other payload fields are never included | TC-2433 |

### Code-vs-spec mismatches

| ID | Spec says | Code does (file:function) | Proposed resolution |
| --- | --- | --- | --- |
| OQ-1810 | `dmuxai run --graph` is a no-op for legacy scripts | `src/cli.rs` `run_headless` passes `graph` into `HeadlessRequest` and `todo run --graph` uses `RunMode::Graph` | Clarify that `--graph` is a no-op only for `run`; `todo run --graph` selects graph mode |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-760 | FR-760 | unit | none | parse each subcommand/flag set, unknown flags | expected `CliAction`; unknown → Unknown (exit 2) | `src/cli.rs` `tc_760_parse_cli_agent_api_subcommands` |
| TC-761 | FR-761, NFR-760 | integration (binary test unix; in-process `headless_run_*` tests on all platforms) | temp workspace, fake custom agent script that prints JSONL text | `dmuxai run "hi" --agent fake --json` | exit 0, stdout is one JSON object with `ok: true` and the agent text; failing script → exit 1 `ok: false` | `src/app.rs` `tc_761_headless_run_returns_agent_text`, `tc_1694_headless_run_reports_agent_error`, `tests/agent_api_cli.rs` `headless_run_json_success_and_failure` |
| TC-762 | FR-761, NFR-762 | integration | config with `allowed_runners = ["fake"]` | `dmuxai run x --agent other --json` and unknown agent | exit 5 / exit 3 with JSON error | `src/app.rs` `tc_762_headless_run_rejects_unknown_and_disallowed_agent`, `tests/agent_api_cli.rs` `headless_run_respects_allowed_runners` |
| TC-763 | FR-762 | integration (unix) | temp workspace + fake agent | `todo add`, `todo list --json`, `todo run <id> --agent fake`, missing id | to-do created, listed, done after run; missing → exit 3 | `tests/agent_api_cli.rs` `tc_763_todo_cli_add_list_run` |
| TC-764 | FR-763 | integration | temp workspace | `dmuxai status --json` | JSON with all §FR-763 keys | `tests/agent_api_cli.rs` `tc_764_status_json_has_contract_keys` |
| TC-765 | FR-764, NFR-760 | unit + integration | log with 3 events and one bad line | `read_since(1)`; `dmuxai events --since 1 --json` | events 2 and 3 only, bad line skipped | `src/events.rs` `tc_765_read_since_skips_malformed_and_filters`; `tests/agent_api_cli.rs` `events_cli_since_json` |
| TC-766 | FR-765 | integration | router with command channel double | `POST /api/prompt` valid, empty, serve, no token | 202 / 400 / 409 / 401 | `src/web/server.rs` `tc_766_prompt_route_contract` |
| TC-767 | FR-766 | integration | hub with event sink | append events, `GET /api/events/log?since=1`, SSE with `since` | JSON catch-up; SSE has `state-change` frames | `src/web/server.rs` `tc_767_events_log_and_sse_state_change` |
| TC-768 | FR-767 | unit | in-memory MCP server with a stub backend | initialize, tools/list, tools/call each tool, unknown tool, notification | correct JSON-RPC responses, no response to notification | `src/mcp_server.rs` `tc_768_mcp_stdio_protocol_and_tools` |
| TC-769 | FR-768 | unit | tracker fed snapshot sequences | node/graph/todo transitions, repeated identical snapshot | one event per transition, none for repeats | `src/events.rs` `tc_769_tracker_emits_one_event_per_transition` |
| TC-770 | FR-768 | unit | two `EventLog` handles on one file in threads | concurrent appends | seqs are 1..N unique and increasing; reopen continues from N | `src/events.rs` `tc_770_event_log_seq_is_monotonic_across_handles` |
| TC-771 | FR-769 | unit | sink with stall 10 s, fake clock | busy, no output 11 s, tick twice; output; tick | one `stalled` event; reset after output | `src/events.rs` `tc_771_stall_detector_emits_once_and_resets` |
| TC-772 | FR-770, FR-772, NFR-761 | integration | local mock HTTP listener; secret env | deliver events; filter; redirect response; slow server | body is event JSON, secret header set, filtered events skipped, 302 not followed, timeout logged | `src/notify.rs` `tc_772_webhook_posts_json_with_secret_no_redirect` |
| TC-773 | FR-771 | integration (unix) | command `cat > file` | deliver one event | file contains the event JSON | `src/notify.rs` `tc_773_command_hook_receives_event_on_stdin` |
| TC-774 | FR-773 | unit | temp workspace | write/remove discovery; env token valid/invalid | file content and mode; invalid env ignored | `src/web/discovery.rs` `tc_774_discovery_file_and_token_env` |
| TC-775 | FR-768 | unit | App with a hub event sink and a scripted agent | start a turn, stream output, complete it | `started` then `completed` agent events with runner and conversation; the CLI run reports their seq range | `src/app.rs` `tc_775_app_emits_agent_lifecycle_events` |
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
| TC-1824 | FR-1824 | unit (app, runners) | `--read-only` with and without `--agent`/`--model`, plus the default/triage and graph paths; cursor/custom/grok runner mappings | run headless | every runner request carries `TurnPolicy::ReadOnly` (cursor `--mode ask` without `--force`, custom/opencode temp config edit+bash `deny`, grok `--permission-mode plan`); a normal run stays write-capable | `src/app.rs` `tc_1824_headless_read_only_reaches_runner`, `tc_1824_headless_read_only_without_agent_and_graph_reaches_runner`, `tc_1824_headless_normal_run_stays_write_capable`; `src/agent/cursor.rs` `tc_1824_cursor_read_only_argv_has_no_force`; `src/agent/custom.rs` `tc_1824_custom_read_only_temp_config_denies_edit_and_bash`; `src/agent/acp_runner.rs` `tc_1824_grok_read_only_uses_plan_permission_mode` |
| See TC-2540 in [SPEC-headless-read-only-write-guard](./SPEC-headless-read-only-write-guard.md) | See FR-2540 in [SPEC-headless-read-only-write-guard](./SPEC-headless-read-only-write-guard.md) | unit | temp git repo | snapshot before/after `.dmuxai/` writes, dirty tracked content edits, staging, and untracked file creation + content edit | `.dmuxai/` ignored; every content edit changes the fingerprint | `src/workspace.rs` `tc_2540_git_worktree_fingerprint_excludes_dmuxai_and_sees_content_edits` |
| See TC-2541 in [SPEC-headless-read-only-write-guard](./SPEC-headless-read-only-write-guard.md) | See FR-2540 in [SPEC-headless-read-only-write-guard](./SPEC-headless-read-only-write-guard.md) | integration (CLI) | fake agent `readonly_write` + `--read-only` | `dmuxai run ... --json` | `ok: false`, `error_kind: policy`, exit 1 | `tests/agent_api_cli.rs` `tc_2541_headless_read_only_detects_workspace_write` |
| See TC-2542 in [SPEC-headless-read-only-write-guard](./SPEC-headless-read-only-write-guard.md) | See FR-2540 in [SPEC-headless-read-only-write-guard](./SPEC-headless-read-only-write-guard.md) | unit (app) | read-only headless run with workspace write | `run_headless` | policy error on success path | `src/app.rs` `tc_2542_headless_read_only_workspace_write_is_policy_error` |
| TC-1761 | FR-1761 | unit (app) | `test_app()` | construct `HeadlessOutcome::failure(...)` | `ok=false`, given exit code, error text, all other optionals `None`; serialized JSON matches the old literal keys | `src/app.rs` `tc_1761_headless_outcome_failure_builder_defaults` |
| TC-1762 | FR-1762 | unit | `SnapshotSource::App` | serde round-trip | `"app"` | `src/web/snapshot.rs` `tc_1762_snapshot_source_app_serializes_as_app` |
| TC-1763 | FR-1763 | unit | `DashboardHub::new(SnapshotSource::App, ...)` | `hub.snapshot()` | source is `App` and serializes `"app"` | `src/cli.rs` `tc_1763_headless_dashboard_source_is_app` |
| TC-1764 | FR-1764 | unit (app) | scripted agent with hub event sink | `run_headless` with explicit agent | exit 0, text returned, `graph_id` None, `last_graph_run` None, event log has `agent` events but no `graph`/`node` events | `src/app.rs` `tc_1764_headless_explicit_agent_runs_without_graph` |
| TC-1765 | FR-1765 | unit (app) | scripted agent returning error | `run_headless` with explicit agent | exit 1, `ok: false`, agent error reported once, no message contains "Graph failed" or "graph node" | `src/app.rs` `tc_1765_headless_explicit_agent_error_reported_once` |
| TC-1766 | FR-1766 | unit (app) | scripted agent | `run_headless` with explicit agent | `outcome.triage` has route simple, reason override, source override, runner, resolved model, and plan is None | `src/app.rs` `tc_1766_headless_explicit_agent_keeps_override_triage` |
| TC-1767 | FR-1767 | unit (app) | scripted agent, sensitive prompt | `run_headless` with explicit agent without flags, then with `approve_plan` | exit 7 and no turn started; with `approve_plan` turn runs to exit 0 | `src/app.rs` `tc_1767_headless_direct_sensitive_prompt_needs_flag` |
| TC-1768 | FR-1768 | integration (CLI unix) | fake agent sandbox | `dmuxai run hi --agent fake --json` | exit 0, `graph_id` null, events contain `agent` events and no `graph`/`node` events | `tests/agent_api_cli.rs` `tc_1768_cli_run_agent_emits_no_graph_events` |
| TC-1769 | FR-1769 | unit | `src/cli.rs` tests | parse `run x --agent a --model m --progress --json` and `todo run abc --agent a --progress` | `RunOptions.progress` true; absent false | `src/cli.rs` `tc_1769_flag_table_parses_run_flags_and_progress` |
| TC-1770 | FR-1770 | unit | `src/cli.rs` tests | invalid inputs | exact error messages unchanged | `src/cli.rs` `tc_1770_flag_table_errors_unchanged` |
| TC-1771 | FR-1771 | unit | `src/cli.rs` tests / `dmuxai --help` | help text | contains `--progress`, not `--read-only` | `src/cli.rs` `tc_1771_help_lists_progress_not_read_only` |
| TC-1772 | FR-1772 | unit | `src/progress.rs` | format each `AgentEvent` kind | shape has documented JSON keys; multiline/long summaries become one line ≤160 chars cut with `…` | `src/progress.rs` `tc_1772_progress_line_shape_and_cut` |
| TC-1773 | FR-1773 | unit | `src/progress.rs` | observe multiple `TextDelta`s within 2 s, then complete | coalesced into one text line; pending text flushed before completed | `src/progress.rs` `tc_1773_progress_text_coalesced_every_2s` |
| TC-1774 | FR-1774 | unit (app) | scripted agent with progress sink installed, then without sink | run headless | sink receives `started`, `text`, `completed` in order; without sink nothing produced | `src/app.rs` `tc_1774_headless_progress_sink_receives_lines` |
| TC-1775 | FR-1775 | unit (app) | App with hub event sink | tool progress, file edit, 50 text deltas in short time | `activity` events logged for tool and file_edit; at most one `output` heartbeat logged | `src/app.rs` `tc_1775_activity_events_logged_and_throttled` |
| TC-1776 | FR-1776 | unit (agent_api) | `events.jsonl` with started + activity | `status_json` | running run has `last_output_at_ms` equal to activity ts | `src/agent_api.rs` `tc_1776_status_json_last_output_at` |
| TC-1777 | FR-1777 | integration (CLI) | fake agent sandbox | `dmuxai run hi --agent fake --json --progress` and without `--progress` | stderr has `started` and `completed` progress JSON lines, stdout is exactly one JSON object; without `--progress` stderr has no progress lines | `tests/agent_api_cli.rs` `tc_1777_cli_progress_lines_before_final_json` |
| TC-1778 | FR-1778 | unit (app) | scripted agent with stage skill configured | `run_headless` with explicit agent, chat enabled then disabled | prompt received contains the skill prefix when chat is enabled and equals the user prompt when disabled | `src/app.rs` `tc_1778_headless_direct_applies_chat_stage_skills` |
| TC-2120 | FR-2120 | unit | conversation database with a current schema, a second connection holding `BEGIN IMMEDIATE` | open the engine while the lock is held, then release the lock inside the budget; map the still-busy open to an exit code | the open takes no write lock before the `runtime_sessions` insert; a held lock ends as a busy error with a retry message, a released lock opens normally, `is_busy` matches SQLITE_BUSY/SQLITE_LOCKED only, and the headless mapping is exit 4 (busy) / 6 (other) | `src/conversation.rs` `tc_2120_open_of_current_schema_takes_no_write_lock`, `tc_2120_open_succeeds_once_the_lock_is_released`, `tc_2120_is_busy_matches_busy_and_locked_codes`, `tc_2120_a_locked_database_produces_a_busy_sqlite_error`; `src/cli.rs` `tc_2120_busy_conversation_open_maps_to_exit_4` |
| TC-2140 | FR-2140 | unit + integration (app) | conversation database on disk, a second connection holding `BEGIN EXCLUSIVE` | save a turn output while the lock is held, then release the lock inside the budget; run a headless turn whose reply is saved while the lock is held for a short time; run a headless turn whose lock outlives the budget and is released afterwards; close the session while the lock is still held after a spent budget | the busy save is retried (more than one attempt) and the reply is persisted once the lock is released; a lock that outlasts the budget ends as a busy error; a spent budget still reaches the database, so the reply is stored once the lock is released; the short-lock headless run succeeds and the reply is stored in the database and in the JSON `text`; a close that fails against the locked database is attempted once and the fallback close returns without waiting again | `src/conversation.rs` `tc_2140_busy_save_is_retried_and_persisted_once_the_lock_is_released`, `tc_2140_a_locked_database_fails_the_save_with_a_busy_error`, `tc_2140_a_spent_budget_still_saves_once_the_lock_is_released`, `tc_2140_a_busy_close_is_attempted_once_and_does_not_wait_twice`; `src/app.rs` `tc_2140_short_database_lock_is_retried_and_the_reply_is_persisted`, `tc_2140_a_spent_budget_stores_the_rest_of_the_reply_after_the_lock_is_released`, `tc_2140_shutdown_after_a_spent_budget_closes_once` |
| TC-2141 | FR-2140 | integration (app) | headless run whose conversation database stays locked while the agent answers | the reply cannot be saved | `ok: false`, exit code 4, `error_kind` `conflict`, the JSON `error` says the reply could not be saved, and the JSON `text` still holds the reply | `src/app.rs` `tc_2141_locked_database_fails_the_run_with_exit_4_and_keeps_the_text` |
| TC-2142 | FR-2140 | integration (app) | headless run whose conversation database stays locked while the agent answers | the reply cannot be saved | the transcript keeps the agent reply next to the "Could not save" system note; the run does not crash | `src/app.rs` `tc_2142_failed_save_keeps_the_system_note_and_the_reply` |
| TC-2143 | FR-2140 | integration (app) | headless run whose conversation database fails with an error that is not a busy error | the reply cannot be saved | `ok: false` with exit code 6 (internal error), `error_kind` `internal`, not the conflict code; the reply text is still returned | `src/app.rs` `tc_2143_non_busy_database_error_maps_to_the_internal_exit_code` |
| TC-2144 | FR-2140 | unit (app) | a busy conversation error and a non-busy database error | map both to a headless exit code and error kind | busy → 4 (conflict) with `error_kind` `conflict`, every other database error → 6 (internal) with `error_kind` `internal`; other exit codes carry no error kind | `src/app.rs` `tc_2144_busy_database_error_maps_to_the_conflict_exit_code` |
| TC-2430 | FR-2430 | unit + integration (CLI) | opencode preset/fake JSONL runner emitting a `tool_use` line | run a turn with a progress sink; run `dmuxai run ... --json --progress` | an `Activity` tool item is emitted, its progress line has `kind:"tool"` with the tool name and title/input first line, and stderr has at least one `tool` line besides started/completed | `src/agent/custom.rs` `tc_2430_opencode_tool_lines_emit_tool_progress`; `tests/agent_api_cli.rs` `tc_2430_cli_progress_emits_tool_line_for_jsonl_runner` |
| TC-2431 | FR-2431 | unit | opencode edit tool line with an absolute in-workspace `filePath` and old/new content | parse the tool line and format progress | activity target is the workspace-relative path, progress `kind` is `file_edit`, and the summary contains neither old nor new content | `src/agent/custom.rs` `tc_2431_edit_tool_emits_file_edit_with_relative_path` |
| TC-2432 | FR-2432 | unit | `ProgressTracker` with a shortened heartbeat interval, a started turn, one typed output line, then a terminal event | advance the clock past each interval | one heartbeat per interval with turn age, last-output age and `type=step_start`; none after completed; every running agent is covered | `src/progress.rs` `tc_2432_heartbeat_every_interval_with_last_output_age` |
| TC-2433 | FR-2433 | unit | opencode write tool line with file content and output fields; long multiline title | parse and format progress | summaries contain only the relative path/first safe line, stay within 160 characters, and never include file content, tool output, or secrets | `src/agent/custom.rs` `tc_2433_progress_summaries_never_include_content` |

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
