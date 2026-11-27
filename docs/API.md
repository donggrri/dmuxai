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

### Global options

The following flags can be used before or after subcommands, or with the interactive TUI:

- `--config PATH`: Path to a `dmuxai.toml` user config file. Overrides `DMUXAI_CONFIG` and default XDG/HOME config paths. Exits with status `2` if the file does not exist.
- `-C DIR`, `--workspace-dir DIR`: Run in `DIR` instead of the current directory.

| Command | Flags | stdout |
| --- | --- | --- |
| `dmuxai run <prompt\|->` | `--agent ID` (Cursor keyword: `cursor-agent`; `cursor` alias), `--model M`, `--approve-plan`, `--allow-sensitive`, `--graph` (no-op), `--progress`, `--json`, `--trust-workspace-config` | Final agent text, or one JSON object with `--json` |
| `dmuxai todo add <title>` | `--body TEXT`, `--json` | The created to-do (TSV or JSON) |
| `dmuxai todo list` | `--status open\|running\|done`, `--json` | To-dos (TSV lines or a JSON array) |
| `dmuxai todo run <id>` | `--agent ID \| --graph`, `--model M`, `--progress`, `--json`, `--trust-workspace-config` | Same as `run`; the to-do is marked running then done/open |
| `dmuxai trust` | `[--status]`, `[--json]`, `[--revoke]`, `[--yes]` | Trust management for workspace `dmuxai.toml` |
| `dmuxai status` | `--json` | Workspace status (JSON if `--json`) |
| `dmuxai runners` | `[--json]` | Registered runners, admission, installation and model policy |
| `dmuxai models` | `[--agent ID]`, `[--json]` | Available models and runner policy |
| `dmuxai events` | `--since N`, `--limit N`, `--follow`/`-f`, `--json` | Events (see §4); `--json` prints one object per line |
| `dmuxai board` | see [Decision board](#decision-board) | Coordinator items (user-level store) |
| `dmuxai mcp` | — | stdio MCP server (see §3) |

Notes:

- `run` accepts either `--agent` or `--graph`, not both. `--graph` is accepted as a
  no-op (legacy scripts); it does not change judge routing. `--read-only` is an internal
  flag (not shown in `dmuxai --help`): runs the prompt's turn with the read-only turn
  policy. `--mode` was removed: any `--mode` on `run` or `todo run` prints a usage
  error to stderr and exits `2` (see [SPEC-acp-runners](specs/SPEC-acp-runners.md)
  FR-1318); the MCP `run` tool rejects a `mode` argument the same way. `--` ends
  flag parsing (`dmuxai run -- --literal prompt`).
- Without `--agent`, headless `run` uses the same judge → plan path as the TUI.
  Exit `7` means plan approval (`--approve-plan`), a sensitive gate, or an agent
  that stopped at its own plan review without editing files. The plan-review case
  needs an explicit approval request together with a plan statement in the
  reply, so an ordinary answer that only mentions a plan still exits `0`. Use
  `--allow-sensitive`, or `--agent` with `--approve-plan` on a one-step run, to
  confirm sensitive gates headlessly. JSON includes `triage` and, when applicable,
  `plan`.
- `dmuxai trust` manages workspace configuration trust:
  - `dmuxai trust`: Interactive prompt in a TTY asking to trust the workspace configuration, displaying actual restricted key values (showing secret environment variable names only, never values).
  - `dmuxai trust --status [--json]`: Reports trust status (`trusted`, `untrusted`, `not_needed`, or `no_workspace_config`), SHA-256 hash, restricted keys, and detailed key-value definitions in JSON mode.
  - `dmuxai trust --yes`: Non-interactively trusts the current workspace configuration.
  - `dmuxai trust --revoke`: Revokes trust for the workspace configuration regardless of current file content.
  - The trust hash covers only `dmuxai.toml` itself, not files it references (skills, prompt files); those are confined to the workspace while untrusted and are not hashed.
  - Child processes do not inherit the `DMUXAI_TRUST_WORKSPACE_CONFIG` opt-in.
  - `dmuxai trust --status --json` includes a `warning` field when the trust store or its directory was ignored; text output prints it to stderr. Displayed configuration values have control characters escaped.
  - Dashboard saves of workspace `dmuxai.toml` (`POST /api/skills`, `POST /api/agent-prompts`) on a write-capable `dmuxai --web` dashboard refresh an existing trust record to the new hash only when the on-disk file still matches it right before the write. They never create trust; `dmuxai serve` (view-only) and the one-process opt-in never refresh it.
  - Headless `run` and `todo run` accept `--trust-workspace-config` (or `DMUXAI_TRUST_WORKSPACE_CONFIG=1`) to trust workspace settings for a single process without modifying the persistent trust store.
- `--model M` requires `--agent`; it sets the Explicit model source for the run
  (validated with `validate_model_for_runner` / custom `allowed_models`). A disallowed
  model is rejected before any turn starts with exit `5` (usage errors such as a
  missing value exit `2`); the accepted model is echoed in `triage.model`.
- `dmuxai run -` reads the prompt from stdin:
  `echo "summarize the repo" | dmuxai run - --agent agy --json`. For Cursor use
  `--agent cursor-agent` (or the alias `cursor`).
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
  "error_kind": null,
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
| `text` | string | Final agent text (still returned when the reply could not be saved, see exit code 4) |
| `messages` | array | `{ role, agent, content }` per message |
| `error` | string\|null | Failure detail when `ok` is false |
| `error_kind` | string\|null | Stable failure category next to `error`: `provider`, `model_unavailable`, `auth` (sign-in required; no runner fallback), `process_exit`, `timeout`, `idle`, `stalled`, `cancelled`, `policy`, `sensitive_gate` (exit 7), or `conflict` (exit 4) / `internal` (exit 6); `null` when there is no error |
| `todo` | object\|null | To-do after a `todo run` |
| `triage` | object\|null | Judge/fallback/override verdict (`source`: `judge`, `fallback`, or `override`) when a graph run started |
| `plan` | object\|null | `{steps, tasks}` when a plan was produced (including exit 7 awaiting approval) |
| `events` | object | `first_seq`/`last_seq` written for this run; both `null` when the run wrote no events |
| `fallbacks` | array | Optional list of handovers: `{from_runner, from_model, reason, detail, to_runner, to_model}` (`to_*` null when the chain is exhausted); omitted when empty |

Failures with `--json` print `{ "ok": false, "exit_code": <n>, "error": "<message>", "error_kind": "<kind>|null" }`, where `error_kind` is `sensitive_gate` for a headless sensitive gate, `conflict` for exit 4 and `internal` for exit 6.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | OK |
| 1 | Turn failed or graph failed |
| 2 | Usage error (unknown flag/subcommand, empty title/prompt) |
| 3 | Not found (to-do id, unknown agent) |
| 4 | Busy / conflict (already running; the workspace conversation database is locked by another process — while it is opened and while a reply is saved) |
| 5 | Runner not in `[graph] allowed_runners` |
| 6 | Internal error (including a non-busy conversation-database error while saving the agent reply, which exits 6 rather than 4) |
| 7 | Plan approval, sensitive gate, or an agent that stopped at its own plan review (headless); `--approve-plan` for plans and for a headless `--agent` run that must edit files; `--allow-sensitive` or `--agent` + `--approve-plan` for sensitive gates |
| 130 | Cancelled (Ctrl+C) |

A locked workspace conversation database is retried with a bounded backoff
(about 20 s of waiting per turn; after that the database is only probed, so a
lock that is released later still receives the reply). When the reply still
cannot be saved, the run ends with exit code 4 (`error_kind` `conflict`), or
exit 6 (`error_kind` `internal`) for any other database error, `error` says the
reply could not be saved, and `text` still holds the answer the agent produced.
After the retry budget is spent the process closes its session with a short
busy timeout, so shutdown does not wait the default connection timeout again
when the database is still locked.

### Examples

```bash
dmuxai status --json
dmuxai run "summarize this repo" --agent agy --json
dmuxai run "ship the feature" --json
dmuxai run "ship the feature" --json --approve-plan
dmuxai run "ship the feature" --json --progress   # live progress JSON lines on stderr
dmuxai run "plan the migration" --graph --json
dmuxai todo add "Write docs" --body "API.md first" --json
dmuxai todo list --status open --json
dmuxai todo run 3f2a1c9e --agent agy --json --progress
dmuxai events --since 0 --limit 50 --json
dmuxai events --follow --json
```

`status --json` keys: `workspace`, `version`, `agents` (`{id, label, allowed, graph_allowed, run}` where running runs include `started_at_ms` and `last_output_at_ms`),
`allowed_runners`, `graph_enabled`, `todos` (`{open, running, done}` or `null`),
`last_graph` (`{id, phase, counts}` or `null`), `events` (`{path, last_seq}`),
`dashboard` (`{base_url, port, pid, token_file}` or `null`) and `warnings` (array).

In `agents[]`:
- `id`: runner id (`cursor-agent`, `agy`, `codex`, or custom agent name)
- `label`: display name
- `allowed`: headless `run` admission under `[graph] allowed_runners` (true when unset)
- `graph_allowed`: graph admission (built-in presets excluded when unset)
- `run`: last run state (`{state, started_at_ms?, last_output_at_ms?, ended_at_ms?, duration_ms?, node?}`)

### `runners` and `models` (SPEC-cli-runners-models)

`dmuxai runners [--json]` lists registered runners in `status` order with:
- `id`: runner id (`cursor-agent`, `agy`, `codex`, or custom agent name; `cursor-agent` also reports `"aliases": ["cursor"]`)
- `label`: display name
- `kind`: `"builtin"`, `"preset"`, or `"custom"`
- `allowed`: headless admission under `[graph] allowed_runners` (true when unset)
- `graph_allowed`: graph admission (built-in presets excluded when unset)
- `installed`: bool indicating executable lookup without running a process (or null if unknown)
- `default_model`: default model string or null
- `model_policy`: human-readable policy summary

`dmuxai models [--agent ID] [--json]` lists models per runner:
- `id`: runner id
- `allowed`: headless admission
- `source`: `"command"`, `"static"`, or `"none"`
- `models`: array of `{ "id", "label"?, "allowed" }` where `allowed` is policy verdict
- `error` / `error_kind`: present when listing failed (e.g. `error_kind: "auth"` when grok is not signed in)

### `term`: persistent terminal sessions (Linux/WSL, SPEC-terminal-sessions)

Named PTY or serial sessions owned by a per-workspace session host. `term` is
Linux/WSL only: there is no Windows ConPTY build, and there is no native `ssh`
session kind (run `ssh` inside a `shell` session). The host starts when a
`term` command needs one (never from `run` or any other command). `term list` and
`term close` do not start a host: with none running, `list` returns `[]` and
`close --all` succeeds with zero sessions closed (exit 0); `close NAME` exits 3
(`not_found`). The host listens on
`$XDG_STATE_HOME/dmuxai/repos/<repo-key>/term/host.sock` (dir 0700, socket 0600, peer uid
checked; a short path under `$XDG_RUNTIME_DIR` or `/tmp/dmuxai-<uid>/` is used when that path
is too long), and exits after its last session ends plus an idle grace (600 s,
`DMUXAI_TERM_IDLE_SECS`). After `term close --all` (including `--force`) of
sessions that were already running, the `dmuxai term __host` process is expected
to stay alive until that grace elapses; this is intentional (FR-2601), not a leak.

```
dmuxai term open   <name> [--kind shell|cmd] [--cwd DIR] [--env K=V]... [--cols N --rows N] [--json] [-- argv...]
dmuxai term open   <name> --serial DEV --baud N [--parity none|even|odd] [--data 8] [--stop 1]
                   [--flow none|rtscts] [--usbipd-busid ID] [--no-usbipd-attach] [--json]
dmuxai term send   <name> <text> [--no-newline] [--secret] [--json]
dmuxai term send   <name> --keys C-c[,Enter,Up,Tab,Esc,F1,...]
dmuxai term read   <name> [--since SEQ] [--tail N] [--raw] [--json]
dmuxai term wait   <name> (--match REGEX | --idle DUR) --timeout DUR [--since SEQ] [--json]
dmuxai term exec   <name> <cmd> [--timeout DUR] [--json]
dmuxai term list   [--all] [--json]
dmuxai term show   <name> [--json]
dmuxai term resize <name> --cols N --rows N
dmuxai term close  <name> [--force] | --all
dmuxai term reopen <name>
```

- Names match `[a-z0-9][a-z0-9-]{0,31}`; a duplicate open exits 4.
- `--serial DEV --baud N` opens the device with serial2 (no extra tool). `--baud` is
  required. WSL1 maps `COMn` to `/dev/ttySn`. On WSL, if the device node is missing,
  the command exits 3 with `error_kind` `needs_usbipd_attach` and does **not** run
  `usbipd.exe`. JSON includes `busid` when known (`--usbipd-busid` or trusted
  `[terminal.serial] usbipd_busid`) and `commands` to type in an elevated Windows
  PowerShell: `usbipd bind --busid <id>` (first time only) and
  `usbipd attach --wsl --busid <id>`. Without a known busid, the commands use a
  `<busid>` placeholder and include `usbipd list`. On native Linux a missing device
  is `serial_not_found` (also exit 3). `--usbipd-busid` only names the id in that
  guidance. `--no-usbipd-attach` is accepted as a deprecated no-op (warn on stderr).
- `term list` hides `closed` and `exited` sessions; `term list --all` includes them.
- `send` returns `{"seq":N}`; pass it to `wait --since N` so a match only looks at output
  that came after your input (without `--since`, wait starts at the current seq and
  does not search earlier buffer text).
- `read`/`wait`/`exec` return `{text, seq_from, seq_to, dropped_bytes}`, plus `reason`
  (`match`, `idle`, `exited`, `marker`, `timeout`) and `exit_code`. Output is ANSI-stripped
  unless `--raw`. The buffer keeps 1 MiB per session.
- `exec` on a `shell` session appends a nonce exit marker and reports the command's
  `exit_code` (the CLI itself exits 0); the marker is removed from `text`. A timeout exits 1
  with `reason: "timeout"`.
- For `wait`, a bare number for `--idle` is milliseconds (`--idle 500` is 500 ms);
  suffixes `ms`/`s`/`m`/`h` are accepted. A bare number for `--timeout` (and for
  `exec --timeout` / `open --timeout`) is seconds (`8` and `8s` are 8 seconds;
  `500ms` is 500 ms).
- `close` sends SIGHUP to the session's whole process tree (including children that moved to
  their own process group), then SIGKILL after 2 s (`--force`: at once). With no host
  running, `close --all` reports zero closed and does not start one.
- While a human holds a session (takeover, dashboard in a later PR), non-human input exits 4
  with `session '<name>' is in human takeover; ask the user to release it`.
- Session children get the same environment scrub as runner children (`DMUXAI_WEB_TOKEN`
  etc. removed); `--env` is added afterwards.
- Every input and output event is journaled, masked, to
  `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/terms/<16hex>/{manifest.json,io.jsonl}` (0700/0600).
  `--secret` input, and input right after a `Password:`/`passphrase`/`PIN:` prompt, is written
  as `[REDACTED input]`.
- With `--json`, stdout is one JSON object; errors are
  `{"ok":false,"error":"…","error_kind":"not_found","exit_code":3}` with the exit codes above.
- Not a sandbox: a session is a real shell with your rights. On non-Unix platforms
  `term` exits 2. There is no Windows ConPTY build and no native `ssh` session kind.

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
| GET | `/api/jira/issues[?refresh=1]` | — | `200` `{configured, issues, error, fetched_at}` (read-only Jira search, 60 s cache) | 401 |
| POST | `/api/todos/{id}/run` | `{mode: "agent"\|"graph", agent?}` | `202` `Todo` | 400, 401, 403, 404, 409, 500, 503, 504 |
| GET | `/api/mcp/servers` | — | `200` `{servers, warnings}` | 401, 403, 503 |
| POST | `/api/import` | `{server, query}` | `200` `{imported, updated}` | 400, 401, 403, 404, 502, 503 |

`GET /api/mcp/servers` lists configured `[[mcp.servers]]` entries for the dashboard. Each entry returns `name`, `kind` (`"generic"` | `"jira"` | `"confluence"`), `transport` (`"stdio"` | `"http"`), `enabled`, and optional `warnings`. Secret headers/env values and the legacy `runners` field are not returned. DmuxAI does not pass MCP servers to agents; `[[mcp.servers]]` is only used by the dashboard (the server list and Jira/Confluence import). The `runners` key in `[[mcp.servers]]` and `{mcp_config}` in custom agent specs are deprecated and ignored with a warning.

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
| `share` | `target` (`codex`/`cursor-agent`/`agy`; `cursor` alias) | `/share` |
| `status`, `toggle_details`, `quit` | — | `/status`, `/details`, `/quit` |

Busy agents refuse mode/model/effort changes (409); runners outside
`allowed_runners` are refused (403). With `dmuxai --web`, `/api/state` and SSE
snapshots also carry `session` (conversation, per-agent pickers, transcript
tail, recent conversations, slash commands).

Each snapshot includes a top-level `turn` object (FR-1650) for polling clients:

| Field | Type | Meaning |
| --- | --- | --- |
| `state` | string | `idle`, `running`, `awaiting_approval`, `needs_input`, `done`, or `failed` |
| `conversation_id` | string | Active conversation |
| `graph_id` | string\|null | Graph run id when a graph turn is in progress or awaiting approval |
| `since_ms` | int | Unix ms when this turn state began (0 when idle) |

`awaiting_approval` means a plan or sensitive replan revision waits for
`/graph approve` (dashboard or `POST /api/command`). `needs_input` means a
sensitive step gate (`waiting_gate`) needs `/graph continue`. While
`awaiting_approval`, the planning runner's `agents[].run.state` is also
`awaiting_approval`, not `done`.

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
| `run` | `prompt`, `agent`, `model`, `approve_plan`, `allow_sensitive`, `graph`, `todo_id` | Run outcome JSON (same keys as `run --json`), plus `approval_required: true` when the child exits `7`. The fields forward to the same child flags as the CLI: `model` → `--model` (requires `agent`, conflicts with `graph`), `approve_plan` → `--approve-plan`, `allow_sensitive` → `--allow-sensitive`. `agent` and `graph` are mutually exclusive. The deprecated `mode` argument is rejected with a removed-mode tool error; the MCP server never passes `--mode` or `--read-only`. |
| `status` | — | Status JSON (see §1) |
| `events_since` | `since` (≥0), `limit` (1–1000, default 200) | `{events, last_seq}` |
| `models` | `agent` (optional) | Available models and runner policy JSON (same as `dmuxai models --json`) |
| `board_add` | `title`, `kind` (required); `summary`, `grade`, `lane`, `plan_hash`, `workspace`, `by`, … | Created board item JSON (`exit_code` in errors) |
| `board_list` | `lane[]`, `all`, `workspace` | Item array |
| `board_show` | `id` | One item |
| `board_update` | `id`, optional fields, `if_version` | Updated item |
| `board_move` | `id`, `lane` | Updated item |
| `board_plan` | `id`, `hash` or `steps` (+ optional `tasks`) | Item with plan revision |
| `board_pending` | `workspace` (optional) | `{items, count}` waiting for human action (`needs_decision`, `unknown`, unchecked `done`) |
| `board_say` | `id`, `text`, `from` | Item with thread |
| `board_wait` | `id` or global wait, `since_version`, `since_seq`, `timeout_secs` (≤300), `wait_for` | Item or `{timed_out:true,version}` |
| `board_prompt` | `id`, optional `hash` | `{prompt}` unmasked text from `graph-pending/` |
| `board_ask` | `id`, `text` (required); `revise`, `agent`, `model`, `by` | Item with planner answer or revised plan (blocks up to 900 s) |
| `board_run` | `id` (required); `by` | `in_progress` item (detached; records run id and moves to `done`/`unknown` on exit) |

`run` spawns a child `dmuxai run --json` (or `dmuxai todo run <id> --json`), adding
the forwarded flags, and blocks until it finishes, so the result is final. Tool
results use `content: [{type:"text", text:"<json>"}]` with `isError: true` on
failure; unknown tools return JSON-RPC `-32602`. A child exit code `7` (plan
approval, sensitive gate, or a plan-only agent reply) is always an `isError: true`
result whose `structuredContent` keeps `exit_code: 7`, the CLI error note and
`plan`/`text`, and adds `approval_required: true`.

Client configuration (generic):

```json
{ "command": "dmuxai", "args": ["mcp"], "cwd": "<repo>" }
```

Most clients wrap it, for example:

```json
{ "mcpServers": { "dmuxai": { "command": "dmuxai", "args": ["mcp"], "cwd": "<repo>" } } }
```

## Decision board

The coordinator board is **user-level** state under `<state base>/board/`, not per
repo. Items from every workspace appear together. Bots use the same operations as
the owner through `dmuxai board …` or MCP `board_*` tools.

Typical loop (see [SPEC-decision-board](specs/SPEC-decision-board.md) §7):

1. `dmuxai -C <repo> run --graph --json -` — on exit `7`, take `plan_hash`.
2. `dmuxai board add -C <repo> --title … --kind feature --plan-hash <hash> --bot coordinator --json`.
3. Lane `auto`: `dmuxai board run <id>` (replaces the manual `board prompt` pipe).
4. Lane `needs_decision`: `dmuxai board wait <id> --for any` (or `board wait --any`); on `approve`, run `dmuxai board run <id>`; on `request_changes` or messages, revise with `dmuxai board ask <id> --revise --text …` (or `board plan --edit`) and reply with `board say`. A failed run moves to `unknown` and can be retried with the same hash from `<state base>/board/plans/<hash>.json`.
5. Bots poll with `dmuxai board events --since S`; plan prompts stay unmasked only under `graph-pending/<hash>.json` (and board plan copies `<state base>/board/plans/<hash>.json`).

Subcommands: `add` (`--key` for idempotency), `list`, `show`, `update`, `move`
(`--if-version`), `plan`, `prompt`, `decide`, `check` (`--text`), `pending`
(`--workspace`), `say`, `wait`, `events`, `rules`, `ask` (`--text`, `--revise`,
`--agent`, `--model`, `--by`), `run` (`--detach`, `--by`).
Approval is **always human-only**: `decide` (every verdict, in every lane),
`check`, `move` into `auto`, and `rules --set` require `owner` (dashboard browser
request, or CLI with stdin and stdout on a TTY, `DMUXAI_RUN_ID` unset, and typed
item-id confirmation on `/dev/tty`). Nobody may `move` an item into `approved` or
`rejected` (use `board decide`). MCP callers default to `by=bot:mcp`; `by=owner`
over MCP or non-TTY CLI is rejected (exit `5`), and `board_decide` is not
exposed over MCP. Bots may run only `auto` items or items approved by `owner`;
`board run` and `board ask` never write `decision` or `checked`.

User-only `[board]` configuration (`$XDG_CONFIG_HOME/dmuxai/dmuxai.toml`; ignored
with a warning in workspace `dmuxai.toml`):

```toml
[board]
planner = "codex:gpt-5.4"
notify_command = ["notify-send", "dmuxai-board"]
notify_webhook_url = "https://hooks.example.test/board"
notify_webhook_secret_env = "DMUXAI_BOARD_SECRET"
notify_events = ["needs_decision", "decision", "message", "run_finished"]
```

Dashboard HTTP (user-level, same Bearer token; `serve` and `--web`):

| Method | Path | Body / query | Success | Errors |
| --- | --- | --- | --- | --- |
| GET | `/api/board` | `?lane=`, `?workspace=`, `?all=` | `200` `{items, launch_allowed}` | 400, 401, 403 |
| GET | `/api/board/pending` | `?workspace=` | `200` `{items, count}` | 400, 401, 403 |
| GET | `/api/board/{id}` | — | `200` item (`previous_steps` when present) | 400, 401, 403, 404 |
| POST | `/api/board/{id}/decision` | `{verdict,text?,option?,plan_hash?}` (human headers required) | `200` item | 400, 401, 403, 404, 409, 422 |
| POST | `/api/board/{id}/check` | `{text?}` (human headers required) | `200` item | 400, 401, 403, 404, 409 |
| POST | `/api/board/{id}/messages` | `{text}` | `200` item | 400, 401, 403, 404 |
| POST | `/api/board/{id}/plan` | `{steps, tasks?}` | `200` item | 400, 401, 403, 404, 422 |
| POST | `/api/board/{id}/lane` | `{lane}` (human headers required) | `200` item | 400, 401, 403, 404 |
| GET | `/api/board/rules` | — | `200` rules JSON | 401, 403 |
| PUT | `/api/board/rules` | rules JSON | `200` rules JSON | 400, 401, 403 |
| GET | `/api/board/events` | `?since=SEQ` (SSE) | `event: board` | 400, 401, 403 |
| POST | `/api/board/{id}/ask` | `{text,revise?,agent?,model?}` | `202` item (`--web` only) | 400, 401, 403, 404, 409 |
| POST | `/api/board/{id}/run` | `{plan_hash?}` | `202` item (`--web` only) | 400, 401, 403, 404, 409 |

Dashboard writes act as `owner`. Owner writes (`POST /decision`, `POST /check`,
`POST /lane`) require `Sec-Fetch-Site: same-origin`, `Sec-Fetch-Mode`, and
`X-Dmuxai-Human: 1` (else `403`). Invalid item ids return `400` before any
store access.

Exit codes match the headless agent API: usage `2`, not found `3`, conflict `4`,
forbidden `5`, validation `1`, internal `6`.

### Memory MCP server

`dmuxai mcp-memory` is a stdio MCP server exposing Obsidian memory tools (`memory_get_context`, `memory_search`, `memory_propose`). DmuxAI does not pass MCP servers to agents; every agent configures MCP tools directly in its own native configuration.

Client configuration (generic):

```json
{
  "mcpServers": {
    "dmuxai-memory": {
      "command": "dmuxai",
      "args": ["mcp-memory", "--workspace", "<repo>"]
    }
  }
}
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
| `kind` | string | `agent`, `node`, `graph`, `todo` or `fallback` |
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
| `activity` | `tool`, `file_edit`, `output` (runner activity and progress heartbeats) |
| `node` | `pending`, `ready`, `running`, `waiting_gate`, `completed`, `failed`, `skipped` |
| `graph` | `active`, `waiting_gate`, `completed`, `failed` |
| `todo` | `started` (→ running), `completed` (→ done), `failed` (running → open with an error), `open` (any other change to open) |
| `fallback` | `<runner>` (handing the turn to that runner) or `exhausted` (chain exhausted) |

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
blocks a runner. In a workspace `dmuxai.toml`, executable notification settings
(`command`, `webhook_url`, `webhook_secret_env`, and `secret_header`) require
workspace configuration trust (see [specs/SPEC-workspace-config-trust.md](specs/SPEC-workspace-config-trust.md)). Untrusted
workspace configurations have these keys stripped.

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

User config loads first; workspace `dmuxai.toml` overrides per key. In a workspace `dmuxai.toml`, settings that run commands (`[[agents.custom]]`, `[notify] command`, external `[[skills.sources]]`), read secret environment variables (`[notify] webhook_secret_env`), or make network connections (`[notify] webhook_url`, git skill sources, `[todo] jira_base_url`) require workspace configuration trust via `dmuxai trust` (see [specs/SPEC-workspace-config-trust.md](specs/SPEC-workspace-config-trust.md)). Untrusted workspace settings are stripped with a warning.

The user config file is located using the following precedence:
1. `--config PATH` CLI flag
2. `DMUXAI_CONFIG` environment variable
3. `$XDG_CONFIG_HOME/dmuxai/dmuxai.toml`
4. `~/.config/dmuxai/dmuxai.toml` (via `HOME`)

An explicit config path passed via `--config` or `DMUXAI_CONFIG` that does not exist exits with status `2` (`config file not found: <path>`). Specifying a config file directly avoids changing `XDG_CONFIG_HOME`, keeping credential files intact. See `dmuxai init` for a commented starter file.

### `[graph]`

| Key | Default | Meaning |
| --- | --- | --- |
| `tasks_file` | `false` | When `true`, DmuxAI writes `<workspace>/.dmuxai/tasks/TASKS-<run id>.md` (UTF-8 markdown checklist) at plan approval and after each task status change. Write failures become a system note and never fail the run. |

```toml
[graph]
tasks_file = true
```

### `[runners.default]` / `[runners.<id>]` / `[runners.<id>.models."<model>"]`

Per-runner and per-model system prompts. User config sets defaults; workspace `dmuxai.toml` overrides per key. In a workspace `dmuxai.toml`, `system_prompt_file` paths that resolve outside the workspace root require workspace configuration trust; otherwise they are stripped when untrusted.

| Key | Default | Meaning |
| --- | --- | --- |
| `system_prompt` | — | Inline prompt text (mutually exclusive with `system_prompt_file`). |
| `system_prompt_file` | — | Path to a prompt file, relative to the config file that sets it unless absolute. |
| `prompt_inject` | `session` | `session` (inject on fresh session or hash change) or `always` (every turn). |
| `prompt_roles` | `["chat", "worker"]` | Roles that may receive the prompt; `judge`/`triage`/`planner`/`replan`/`summary`/`interview` never do. |
| `builtin_prompt` | `true` | Whether the built-in prompt may be used for this runner (or for runners without their own value under `[runners.default]`). `false` disables built-ins; a runner value beats the default value. |
| `models` | — | Per-model prompt tables keyed by model id (`[runners.<id>.models."<model>"]`). Not allowed under `[runners.default]` (warns and is ignored). |
| `model` | — | Per-runner model profile (`[runners.<id>] model = "..."`); validated with `validate_model_for_runner` (built-in) / `allowed_models` (custom). Invalid values are dropped with a config warning. `[runners.default] model` is ignored with a warning. |
| `fallback` | — | Ordered chain of `"<runner>"` or `"<runner>:<model>"` targets tried when the runner fails for provider reasons or stalls; empty array disables the default chain for that runner; missing/empty means old behaviour. Invalid targets are dropped with a config warning. |
| `stall_timeout_secs` | `240` | No-progress seconds that trigger a stall when the runner has a non-empty chain; `0` disables. |
| `stall_api_retries` | `3` | Consecutive `API error (attempt N)` retry lines that trigger a stall; `0` disables. |

Model policy (SPEC-open-model-lists): `cursor-agent` (`cursor`) and `agy` accept any non-fast model id their CLI reports (`dmuxai models --agent cursor-agent|agy`). Ids containing `fast` are blocked for every runner unless the **user** config sets:

```toml
[models]
allow_fast = true   # user config only; a workspace dmuxai.toml [models] table is ignored
```


`[runners.<id>.models."<model>"]` accepts `system_prompt`, `system_prompt_file`, and `prompt_inject` with the same validation, 32 KiB cap, relative-path rules, and warnings as the runner table. Model ids match case-insensitively after trimming. An explicitly empty/whitespace prompt at the model or runner level means "no prompt" and stops resolution; it does not fall through to built-ins or `[runners.default]`.

Resolution order for a turn's prompt (first match wins; built-ins only on write turns):

1. `[runners.<runner>.models."<model>"]`
2. `[runners.<runner>]`
3. Built-in by model (`gemini-*` → AGY/Gemini Flash, `composer-2.5` → Cursor, runner `cursor-agent` + `grok-4.7*` → Cursor Grok)
4. Built-in by runner (`agy` → AGY/Gemini Flash, `cursor-agent` → Cursor, `grok` → Grok Build)
5. `[runners.default]`
6. None

Built-in source labels are `builtin:model:gemini`, `builtin:model:composer-2.5`, `builtin:model:cursor:grok-4.7`, `builtin:runner:agy`, `builtin:runner:cursor`, and `builtin:runner:grok`. `prompt_inject` resolves model entry > runner table > default > `session`; built-ins use the runner/default value. Switching model changes the prompt hash and re-injects on the `session` cadence.

```toml
[runners.default]
builtin_prompt = true
system_prompt = "Follow these rules."
prompt_inject = "session"
prompt_roles = ["chat", "worker"]

[runners.codex]
system_prompt_file = "prompts/codex.md"

[runners.cursor-agent]
builtin_prompt = false

[runners.cursor-agent.models."composer-2.5"]
system_prompt_file = "prompts/composer.md"
prompt_inject = "session"
```

Precedence for a turn's effective model: explicit (`--model` / explicit request) >
selected (F2 picker / `ModelCatalog.selected`) > `[runners.<id>] model` >
custom agent `default_model` > runner default (Cursor → `composer-2.5`) >
provider default (no `--model`).

## 7. Environment variables and child process policy

### Environment variables

| Variable | Meaning |
| --- | --- |
| `DMUXAI_CONFIG` | Path to the user configuration file (`dmuxai.toml`). Overridden by `--config PATH`. |
| `DMUXAI_WORKSPACE` | Canonical workspace directory (defaults to current directory or `-C`). |
| `DMUXAI_STATE_DIR` | Custom root directory for DmuxAI state databases and conversations. |
| `DMUXAI_TRUST_WORKSPACE_CONFIG` | Opt in to trust workspace configuration for the current process (`1`, `true`, or `yes`). Does not write to persistent trust store. |
| `DMUXAI_WEB_TOKEN` | Bearer token for the web dashboard (32–128 characters of `[A-Za-z0-9_-]`). |
| `DMUXAI_MCP_CONFIG` | Legacy MCP configuration path (deprecated; scrubbed from child environments). |
| `DMUXAI_*_BIN` | Explicit executable paths for runner CLIs (`DMUXAI_CURSOR_BIN`, `DMUXAI_AGY_BIN`, `DMUXAI_CODEX_BIN`, `DMUXAI_OPENCODE_BIN`, `DMUXAI_GROK_BIN`). |
| `DMUXAI_FORCE_COLOR` | Force color output even when `NO_COLOR` is set. |
| `DMUXAI_REDUCED_MOTION` | Disable motion animations in the TUI. |

### Child process environment policy

Every child process spawned by DmuxAI (including agents, runner CLIs, custom commands, MCP servers, and notify webhooks/commands) has DmuxAI-internal variables stripped from its environment to prevent unintended privilege escalation or leaking parent settings:

- **Removed from child environments:**
  - `DMUXAI_WEB_TOKEN`: Secret dashboard API token (never leaked to agent processes).
  - `DMUXAI_MCP_CONFIG`: Legacy MCP configuration path (always scrubbed; DmuxAI no longer propagates MCP configs to runners).
  - `DMUXAI_CONFIG`: Config file path (child tools or nested DmuxAI instances use standard workspace/default configuration isolation).
- **Preserved in child environments:**
  - `XDG_*` (such as `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`) and `HOME` are **always kept untouched**, preserving child access to git, GitHub CLI (`gh`), ssh keys, and system tool configurations.
  - Other `DMUXAI_*` variables (`DMUXAI_STATE_DIR`, `DMUXAI_WORKSPACE`, runner binary paths, `DMUXAI_REDUCED_MOTION`, `DMUXAI_FORCE_COLOR`) are kept so child processes or subcommands can resolve workspace state and user preferences.

