# DmuxAI Agent API

DmuxAI exposes the same operations through four surfaces that share one contract.
All commands run against the workspace (repo) they are started in.

| Surface | Entry point | Use it when |
| --- | --- | --- |
| CLI | `dmuxai run`, `todo`, `status`, `events` | A one-shot prompt, a to-do action or a status/event query from a script. |
| HTTP | `dmuxai serve` (view-only) or `dmuxai --web` | You want to drive a dashboard that is already running (another process owns the writable dashboard). |
| MCP | `dmuxai mcp` (stdio) | An agent client (IDE, chat bot) needs DmuxAI as tools. |
| Events | `.dmuxai/events.jsonl` + HTTP/CLI/MCP readers | Following progress and reconstructing state changes. |

Normative behavior lives in [specs/SPEC-agent-api.md](specs/SPEC-agent-api.md);
this page is the user-facing contract.

## 1. Headless CLI

Subcommands are recognized as the first argument and never start the TUI.
`--help`/`--version` print to stdout and exit `0`; unknown subcommands or flags
print usage to stderr and exit `2`.

| Command | Flags | stdout |
| --- | --- | --- |
| `dmuxai run <prompt\|->` | `--agent ID`, `--mode ask\|plan\|implement`, `--approve-plan`, `--allow-sensitive`, `--graph` (no-op), `--json` | Final agent text, or one JSON object with `--json` |
| `dmuxai todo add <title>` | `--body TEXT`, `--json` | The created to-do (TSV or JSON) |
| `dmuxai todo list` | `--status open\|running\|done`, `--json` | To-dos (TSV lines or a JSON array) |
| `dmuxai todo run <id>` | `--agent ID \| --graph`, `--mode M`, `--json` | Same as `run`; the to-do is marked running then done/open |
| `dmuxai status` | `--json` | Workspace status (JSON if `--json`) |
| `dmuxai events` | `--since N`, `--limit N`, `--follow`/`-f`, `--json` | Events (see §4); `--json` prints one object per line |
| `dmuxai mcp` | — | stdio MCP server (see §3) |

Notes:

- `run` accepts either `--agent` or `--graph`, not both. `--graph` is accepted as a
  no-op (legacy scripts); it does not change judge routing. `--` ends flag parsing
  (`dmuxai run -- --literal prompt`).
- Without `--agent`, headless `run` uses the same judge → plan path as the TUI.
  Exit `7` means plan approval (`--approve-plan`) or a sensitive gate. Use
  `--allow-sensitive`, or `--agent` with `--approve-plan` on a one-step run, to
  confirm sensitive gates headlessly. JSON includes `triage` and, when applicable,
  `plan`.
- `dmuxai run -` reads the prompt from stdin:
  `echo "summarize the repo" | dmuxai run - --agent agy --json`.
- With `--json`, stdout carries only the JSON document(s); warnings/diagnostics go
  to stderr.

### `run --json` output

```json
{
  "ok": true,
  "exit_code": 0,
  "conversation_id": "9f1c…",
  "agent": "agy",
  "graph_id": null,
  "text": "The repo is a Rust TUI client…",
  "messages": [
    { "role": "agent", "agent": "agy", "content": "The repo is a Rust TUI client…" }
  ],
  "error": null,
  "todo": null,
  "triage": { "route": "simple", "source": "judge", "reason": "…", "runner": "agy", "model": "…", "elapsed_ms": 42 },
  "plan": null,
  "events": { "first_seq": 12, "last_seq": 14 }
}
```

| Key | Type | Meaning |
| --- | --- | --- |
| `ok` | bool | Turn/graph succeeded |
| `exit_code` | int | Process exit code the CLI returns |
| `conversation_id` | string | Durable conversation id |
| `agent` | string\|null | Runner that answered (null for graph runs) |
| `graph_id` | string\|null | Graph run id (null for single-agent runs) |
| `text` | string | Final agent text |
| `messages` | array | `{ role, agent, content }` per message |
| `error` | string\|null | Failure detail when `ok` is false |
| `todo` | object\|null | To-do after a `todo run` |
| `triage` | object\|null | Judge/fallback/override verdict (`source`: `judge`, `fallback`, or `override`) when a graph run started |
| `plan` | object\|null | `{steps, tasks}` when a plan was produced (including exit 7 awaiting approval) |
| `events` | object | `first_seq`/`last_seq` written for this run; both `null` when the run wrote no events |

Failures with `--json` print `{ "ok": false, "exit_code": <n>, "error": "<message>" }`.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | OK |
| 1 | Turn failed or graph failed |
| 2 | Usage error (unknown flag/subcommand, empty title/prompt) |
| 3 | Not found (to-do id, unknown agent) |
| 4 | Busy / conflict (already running) |
| 5 | Runner not in `[graph] allowed_runners` |
| 6 | Internal error |
| 7 | Plan approval or sensitive gate required (headless); `--approve-plan` for plans; `--allow-sensitive` or `--agent` + `--approve-plan` for sensitive gates |
| 130 | Cancelled (Ctrl+C) |

### Examples

```bash
dmuxai status --json
dmuxai run "summarize this repo" --agent agy --json
dmuxai run "ship the feature" --json
dmuxai run "ship the feature" --json --approve-plan
dmuxai run "plan the migration" --graph --json
dmuxai todo add "Write docs" --body "API.md first" --json
dmuxai todo list --status open --json
dmuxai todo run 3f2a1c9e --agent agy --mode implement --json
dmuxai events --since 0 --limit 50 --json
dmuxai events --follow --json
```

`status --json` keys: `workspace`, `version`, `agents` (`{id, label, allowed}`),
`allowed_runners`, `graph_enabled`, `todos` (`{open, running, done}` or `null`),
`last_graph` (`{id, phase, counts}` or `null`), `events` (`{path, last_seq}`),
`dashboard` (`{base_url, port, pid, token_file}` or `null`) and `warnings` (array).

## 2. HTTP API

Start a dashboard, then drive it over HTTP:

```bash
dmuxai serve            # dashboard only, view-only for prompt/run
dmuxai --web            # dashboard only, no TUI; prompt/run enabled, prints the URL
dmuxai serve --port 0   # pick a free port
```

### Auth

Every `/api/*` request needs the token, via either:

- `Authorization: Bearer <token>`, or
- `?token=<token>`.

The token is a per-start random value (printed in the startup URL) unless
`DMUXAI_WEB_TOKEN` is set. It must be 32–128 characters of `[A-Za-z0-9_-]`; an
invalid value is ignored with a warning and a random token is used.

A running dashboard writes its endpoint and token to the discovery file:

```json
{ "base_url": "http://127.0.0.1:7420", "port": 7420, "pid": 12345, "token": "<token>" }
```

Path: `<workspace>/.dmuxai/web.json`. Owner-only (`0600`) on Unix, removed on
clean exit. Read it instead of parsing logs.

### Host and Origin rules (#27)

- For loopback binds the `Host` header must be one of
  `127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>`. A non-loopback bind
  (`--bind ADDR --allow-remote`) disables this allowlist.
- When an `Origin` header is present it must equal `http://<Host>`.
- A missing/disallowed `Host` or a foreign `Origin` is `403`. A missing or wrong
  token is `401`.

### Routes

`todo` bodies are `{title, body?}`; `PATCH` accepts `{title?, body?, status?}`.
`{id}` is a to-do id. All routes below require the token.

| Method | Path | Request | Success | Errors |
| --- | --- | --- | --- | --- |
| GET | `/api/state` | — | `200` dashboard snapshot | 401, 403 |
| GET | `/api/events` | `?since=N` | `200` SSE | 401, 403 |
| GET | `/api/events/log` | `?since=N&limit=M` | `200` `{events, last_seq}` | 400, 401, 403 |
| POST | `/api/prompt` | `{text, agent?, mode?, attachments?}` | `202` `{accepted, conversation_id, agent}` | 400, 401, 403, 404, 409, 415, 504 (prompt not accepted within 10 s) |
| POST | `/api/attachments` | raw image bytes (png, jpeg, gif or webp; ≤ 10 MiB) | `201` `{name, bytes, mime}` | 400, 401, 403, 409, 413, 415 |
| GET | `/api/attachments/{name}` | — | `200` image bytes | 401, 403, 404 |
| POST | `/api/command` | `{command, ...}` (see below) | `200` `{ok, session}` | 400, 401, 403, 404, 409, 415, 504 |
| GET | `/api/todos` | — | `200` `[Todo]` | 401, 403, 503 |
| POST | `/api/todos` | `{title, body?}` | `201` `Todo` | 400, 401, 403, 413, 415, 503 |
| PATCH | `/api/todos/{id}` | `{title?, body?, status?}` | `200` `Todo` | 400, 401, 403, 404, 415, 503 |
| DELETE | `/api/todos/{id}` | — | `204` | 401, 403, 404, 503 |
| POST | `/api/todos/{id}/run` | `{mode: "agent"\|"graph", agent?}` | `202` `Todo` | 400, 401, 403, 404, 409, 500, 503, 504 |

Status meanings:

- `400` invalid body (bad JSON, empty text, invalid status/mode, invalid
  `since`/`limit`); `413` body too large; `415` missing/wrong `Content-Type`.
- `404` unknown to-do or unknown agent, `409` busy agent / already running / a
  `serve` dashboard asked to prompt or run, `503` to-do board unavailable,
  `504` run request timed out (to-do run) or prompt accept timed out (`POST /api/prompt`).

`POST /api/command` runs a picker or lifecycle action the way the matching TUI
command does (`dmuxai --web` only; SPEC-dashboard-parity FR-781). `agent`
defaults to the selected agent:

| `command` | Fields | TUI equivalent |
| --- | --- | --- |
| `select_agent` | `agent` | `/agent` |
| `set_mode` | `agent?`, `mode` (ignored; one notice) | removed `/mode` (SPEC-acp-runners) |
| `load_models` | `agent?` | opening F2 |
| `select_model` | `agent?`, `model` | F2 |
| `set_effort` | `agent?`, `effort` (Codex) | `/effort` |
| `cancel` | `agent?` | Ctrl+C |
| `new_conversation` | `title?` | `/new` |
| `load_conversation` | `id` | `/resume`, F3 |
| `share` | `target` (`codex`/`cursor`/`agy`) | `/share` |
| `status`, `toggle_details`, `quit` | — | `/status`, `/details`, `/quit` |

Busy agents refuse mode/model/effort changes (409); runners outside
`allowed_runners` are refused (403). With `dmuxai --web`, `/api/state` and SSE
snapshots also carry `session` (conversation, per-agent pickers, transcript
tail, recent conversations, slash commands).

`POST /api/prompt` submits the text exactly like pressing Enter in the TUI
(slash commands included). `attachments` lists up to 10 names returned by
`POST /api/attachments`; the images join the next turn like `/image` or a
clipboard paste in the TUI (Cursor and AGY get `@<absolute path>` tokens, Codex
an "Attached images" line). Uploads need `dmuxai --web` (`403` on a view-only
`serve`); the type is checked by magic bytes, the server always picks the file
name (`<workspace>/.dmuxai/attachments/web-<uuid>.<ext>`), and only names of that
shape are served back. `GET /api/events/log` returns events with
`seq > since` (default `0`), oldest first; `limit` is capped at `1000`.

`GET /api/events` is a Server-Sent Events stream:

- `event: snapshot` frames carry the full dashboard snapshot (as today).
- `event: state-change` frames carry one event record (see §4).
- `?since=N` replays events after `N` as `state-change` frames before live ones.
- The server sends a keep-alive comment every 15 s.

`serve` is view-only: `POST /api/prompt` and `POST /api/todos/{id}/run` return
`409` until DmuxAI runs with `--web`.

### curl examples

```bash
# discover the running dashboard (fields: base_url, port, pid, token)
cat .dmuxai/web.json
TOKEN="<token>"; URL="http://127.0.0.1:7420"

curl -s -H "Authorization: Bearer $TOKEN" "$URL/api/state"
curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -X POST "$URL/api/prompt" -d '{"text":"summarize the repo","agent":"agy"}'
curl -s "$URL/api/events/log?since=0&limit=100&token=$TOKEN"
curl -N "$URL/api/events?since=0&token=$TOKEN"        # SSE: snapshot + state-change
curl -s "$URL/api/todos?token=$TOKEN"
curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -X POST "$URL/api/todos" -d '{"title":"Write docs"}'
```

## 3. MCP server

`dmuxai mcp` speaks MCP over stdio: newline-delimited JSON-RPC 2.0 on
stdin/stdout. Only protocol messages are written to stdout; diagnostics go to
stderr. Supported protocol versions (newest first): `2025-06-18`, `2025-03-26`,
`2024-11-05`.

Tools (`tools/list`):

| Tool | Arguments | Returns |
| --- | --- | --- |
| `todo_add` | `title` (required), `body` | Created to-do |
| `todo_list` | `status` (`open`\|`running`\|`done`) | Array of to-dos |
| `run` | `prompt`, `agent`, `mode` (`ask`\|`plan`\|`implement`), `graph`, `todo_id` | Run outcome JSON (same keys as `run --json`) |
| `status` | — | Status JSON (see §1) |
| `events_since` | `since` (≥0), `limit` (1–1000, default 200) | `{events, last_seq}` |

`run` spawns a child `dmuxai run --json` (or `dmuxai todo run <id> --json`) and
blocks until it finishes, so the result is final. Tool results use
`content: [{type:"text", text:"<json>"}]` with `isError: true` on failure;
unknown tools return JSON-RPC `-32602`.

Client configuration (generic):

```json
{ "command": "dmuxai", "args": ["mcp"], "cwd": "<repo>" }
```

Most clients wrap it, for example:

```json
{ "mcpServers": { "dmuxai": { "command": "dmuxai", "args": ["mcp"], "cwd": "<repo>" } } }
```

## 4. Events

Every agent turn, graph node, graph phase and to-do transition appends one JSON
line to `<workspace>/.dmuxai/events.jsonl`. Deleting the file is safe; `seq`
restarts at 1.

### Schema (version `v` = 1)

```json
{"v":1,"seq":12,"ts":"2026-10-17T11:22:33.456Z","ts_ms":1792236153456,
 "workspace":"/repo","kind":"node","task":"graph-plan-id","todo":null,
 "node":"implement-1","runner":"agy","model":"gemini-3.8-flash-high",
 "from_state":"ready","to_state":"running","message":null}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `v` | int | Schema version (`1`) |
| `seq` | int | Per-workspace, strictly increasing, starts at 1 |
| `ts` | string | RFC 3339 UTC with milliseconds |
| `ts_ms` | int | Epoch milliseconds |
| `workspace` | string | Workspace label |
| `kind` | string | `agent`, `node`, `graph` or `todo` |
| `task` | string\|null | Graph run id |
| `todo` | string\|null | To-do id |
| `node` | string\|null | Graph node id |
| `runner` | string\|null | Runner id |
| `model` | string\|null | Model id |
| `from_state` | string\|null | Previous state |
| `to_state` | string | New state (never null) |
| `message` | string\|null | Detail (e.g. stall or failure text) |

Readers must ignore unknown fields; malformed lines are skipped.

### `kind` / `to_state` vocabulary

| `kind` | `to_state` values emitted |
| --- | --- |
| `agent` | `pending`, `started`, `running`, `completed`, `error`, `cancelled`, `stalled` |
| `node` | `pending`, `ready`, `running`, `waiting_gate`, `completed`, `failed`, `skipped` |
| `graph` | `active`, `waiting_gate`, `completed`, `failed` |
| `todo` | `started` (→ running), `completed` (→ done), `failed` (running → open with an error), `open` (any other change to open) |

Ordering guarantees:

- `seq` is strictly increasing within a workspace and shared across processes:
  each append takes `<workspace>/.dmuxai/events.lock` (created exclusively,
  treated as stale after 5 s, retried up to 2 s) and re-reads the last valid
  `seq` under the lock.
- One transition = exactly one line; unchanged state writes nothing.
- A write failure never blocks or crashes a runner.

### Catch-up patterns

| Reader | How |
| --- | --- |
| CLI | `dmuxai events --since N --limit M --json` (add `--follow` to keep tailing) |
| HTTP | `GET /api/events/log?since=N&limit=M` |
| HTTP SSE | `GET /api/events?since=N` (replays then streams `state-change` frames) |
| MCP | `events_since` with `{ since, limit }` |

## 5. Notifications

Configured under `[notify]` in `dmuxai.toml`. Delivery is best effort and never
blocks a runner.

| Key | Default | Meaning |
| --- | --- | --- |
| `webhook_url` | — | HTTP(S) URL; each event is POSTed as JSON (redirects are not followed) |
| `webhook_secret_env` | — | Env var name holding the secret value (secrets never live in `dmuxai.toml`) |
| `secret_header` | `X-Dmuxai-Secret` | Header that carries the secret |
| `timeout_secs` | `5` | Per-delivery timeout (webhook and command) |
| `command` | — | argv array run once per event; event JSON on stdin, cwd = workspace, no shell unless configured |
| `events` | all | Only deliver events whose `to_state` is listed (the log keeps everything) |
| `stall_after_secs` | `600` | Busy-runner stall threshold; `0` disables the detector |

Delivery guarantees:

- One bounded queue of 256 events, delivered in order, off the runner/UI thread.
- An event that does not fit is dropped and the drop is logged.
- Failures and timeouts are appended to `<workspace>/.dmuxai/notify.log`; the
  secret and any URL query string are never logged.

```toml
[notify]
webhook_url = "https://example.invalid/hooks/dmuxai"
webhook_secret_env = "DMUXAI_WEBHOOK_SECRET"
secret_header = "X-Dmuxai-Secret"
timeout_secs = 5
events = ["completed", "failed", "stalled"]
stall_after_secs = 600
command = ["tee", "-a", "events.log"]
```

## 6. Configuration (`dmuxai.toml`)

User config (`$XDG_CONFIG_HOME/dmuxai/dmuxai.toml` or `~/.config/dmuxai/dmuxai.toml`)
loads first; workspace `dmuxai.toml` overrides per key. See `dmuxai init` for a
commented starter file.

### `[graph]`

| Key | Default | Meaning |
| --- | --- | --- |
| `tasks_file` | `false` | When `true`, DmuxAI writes `<workspace>/.dmuxai/tasks/TASKS-<run id>.md` (UTF-8 markdown checklist) at plan approval and after each task status change. Write failures become a system note and never fail the run. |

```toml
[graph]
tasks_file = true
```
