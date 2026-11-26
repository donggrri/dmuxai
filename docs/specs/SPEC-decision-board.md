# SPEC: Decision board (coordinator board)

Status: in progress (series 2/3: dashboard Board view and HTTP API) · Target: v0.4.1
Owner: DmuxAI
Created: 2026-11-26
Baseline: `897c973`
Related: SPEC-graph-plan-approval-269 (pending plans), SPEC-agent-api (CLI/MCP/notify contract), SPEC-web-all-projects (all-projects index), SPEC-web-local-access (dashboard access rules)

## 1. Decision summary

A coordinator bot drives many dmuxai runs in parallel across projects, and the
owner can only review one thing at a time. The decision board collects, in one
user-level place, every piece of work the coordinator is handling, separates
what the coordinator may finish alone (for example debugging) from what needs
the owner's decision (for example a feature change), and lets the owner read
and edit the plan, decide, and discuss it in a side chat. It is modelled on a
"coordinator board": a "My decisions" card list plus a kanban of lanes.

1. Store: one user-level board under `<state base>/board/` (not per repo), so
   items from every workspace of this OS user appear together. Each item
   records its workspace path and repo key.
2. Lanes: `auto` (the coordinator proceeds), `needs_decision`, `approved`,
   `in_progress`, `done`, `rejected`, `unknown`.
3. Classification: the poster gives a `kind` (`debug`, `feature`, `question`,
   `review`, `other`) and an optional difficulty `grade` (`A`..`D`). Editable
   rules choose the initial lane (default: `debug` → `auto`, everything else →
   `needs_decision`). An explicit lane from the poster or the owner wins.
4. Plan: a board plan is a graph pending plan (SPEC-graph-plan-approval-269).
   Attaching or editing a plan writes a pending file and records its hash, so
   execution reuses `run --graph --approve-plan-id` and its re-validation.
5. Decisions: `approve`, `request_changes` (with direction text), `answer`
   (pick an option or free text), `reject`. A decision is bound to the plan
   hash the owner saw.
6. Side chat: each item has a thread. Owner messages and decisions wake a
   bot that is blocked in `board wait`.
7. Bots use the same operations through the CLI (`dmuxai board ...`), MCP tools
   and (series 2) HTTP.

Series:

- PR 1 (this file's requirement table): store, rules, plan attach/edit,
  decisions, thread, wait, CLI, MCP.
- PR 2: dashboard Board view (lanes, My decisions cards, item drawer with plan
  editor and side chat), HTTP API, live updates.
- PR 3: "Ask planner" (planner turn from the side chat that may return a
  revised plan), `board run` (execute the approved plan and track lanes),
  notify hooks.

## 2. Scope

### In scope (series)

- Board items, lanes, rules, plan attach/edit/validate, decisions, thread,
  blocking wait, CLI, MCP, HTTP, dashboard Board view, planner chat, approved
  plan execution, notifications.

### Out of scope (v0.4.1)

- Choosing machines/hosts, per-account usage balancing, automatic digests
  that scan every session, a "send task to coordinator" box (follow-ups).
- Other OS users' boards; remote multi-user access.

### Assumptions and open questions

| ID | Question | Default chosen |
|---|---|---|
| OQ-3100 | Store location | `<state base>/board/` (user level). |
| OQ-3101 | Default rules | `debug` → `auto`; other kinds → `needs_decision`; owner may move any card. |
| OQ-3102 | May the view-only `serve` dashboard write decisions/messages? | Yes (they launch no agent); planner chat and run need `--web` (PR 2/3). |
| OQ-3103 | Retention | Items in `done`/`rejected` older than 30 days are hidden from the default list, never deleted automatically. |
| OQ-3104 | Who may approve? | The owner: the dashboard, or the CLI with stdin on a TTY (`--by owner` is the TTY default). A non-TTY CLI or MCP call cannot act as owner (exit 5), so a bot cannot approve its own `needs_decision` item; bots may approve only items in `auto`. Owner on the CLI also needs stdout on a TTY and no `DMUXAI_RUN_ID` (agent subprocess). This is a guard against accidental self-approval, not authentication against a hostile local process. |

## 3. Terms

- State base: `state_path::state_base()`.
- Item id: `b` followed by 12 lowercase hex characters (`^b[0-9a-f]{12}$`).
- Pending plan: `<state base>/graph-pending/<hash>.json` holding `prompt`,
  `tasks`, `steps`; hash = first 16 hex of sha256 of that JSON
  (SPEC-graph-plan-approval-269 FR-2545).
- Version: per-item integer, +1 on every change; used for optimistic
  concurrency and for `wait`.

## 4. Requirements

| ID | Pri | Requirement |
|---|---|---|
| FR-3100 | Must | Items are stored one JSON file per item under `<state base>/board/items/<id>.json` (directories 0700, files 0600, atomic write via temp file + rename), with `id`, `title`, `summary`, `kind`, `grade` (optional), `lane`, `source` (`bot`, optional `channel` label, optional `ref` http(s) URL), `workspace` (`path`, `repo_key`, both optional), `recommendation` (optional), `options` (list of short strings, optional), `plan` (optional), `decision` (optional), `thread`, `run_ids`, `created_ms`, `updated_ms`, `version`. Unknown fields in newer files are ignored when read. |
| FR-3101 | Must | Every change appends one line to `<state base>/board/events.jsonl` with a monotonically increasing `seq`, `item_id`, `kind` (`created`, `updated`, `lane`, `plan`, `decision`, `message`), `version` and `at_ms`; writers serialize through an exclusive lock file so concurrent CLI/MCP/dashboard writers never lose an update. |
| FR-3102 | Must | Item ids, lanes, kinds, grades and option indexes are validated before any filesystem access; an invalid id is a usage error (exit 2) and is never joined to a path. Limits: title 200 chars, summary/recommendation 8000, message 8000, options 8 × 200, thread 500 messages; over-limit input is a usage error. |
| FR-3103 | Must | All stored free text (title, summary, recommendation, options, messages, plan step text) is passed through `run_journal::mask_sensitive_tokens` before it is written. |
| FR-3104 | Must | Rules live in `<state base>/board/rules.json`: an ordered list of `{kind?, grade?, title_regex?, lane}` plus `default_lane`. The first matching rule picks the initial lane when an item is created without an explicit lane. Missing file → built-in defaults (OQ-3101). An invalid rules file is reported and the defaults are used. `dmuxai board rules [--set <file>|-]` prints or replaces the rules (validated; regex size-limited). |
| FR-3105 | Must | `dmuxai board add --title T [--summary S|-] --kind K [--grade G] [--lane L] [--bot NAME] [--channel C] [--ref URL] [-C DIR] [--recommend TEXT] [--option TEXT]... [--plan-hash H] [--key K] [--json]` creates an item; with `--key` an existing item with the same key and bot is returned instead of a duplicate; a non-owner `--lane auto|approved` is refused (exit 5) — the rules choose the lane; `-C` (default: current dir) records the canonical workspace path and repo key; prints the item (JSON with `--json`, else the id). |
| FR-3106 | Must | `--plan-hash H` (and `board plan <id> --hash H`) attaches the pending plan `H`: the file is read (16-hex validated; missing → exit 3), its content hash must equal `H` (else exit 1); `plan` stores `hash`, a masked display copy of `prompt`, `tasks`, `steps`, `revision` (+1 per attach/edit) and `history` (previous hashes, max 20). The unmasked prompt stays only in the 0600 pending file. `dmuxai board prompt <id> [--hash H]` (MCP `board_prompt`) prints the unmasked prompt of the current plan (or of a hash in its history) so the bot can pipe it into `run --graph --approve-plan-id`. Attaching or editing a plan clears the previous decision and moves an `approved` item back to `needs_decision` (an `auto` item stays `auto`). |
| FR-3107 | Must | `dmuxai board plan <id> --edit <file|->` replaces the plan steps (and optional tasks) from a JSON document `{steps:[...], tasks:[...]}`: it is parsed with the graph plan parser, validated with `validate_plan_with_limit` against the current graph runners/limits (a pre-check; `run --graph --approve-plan-id` re-validates authoritatively), and on success written as a new pending plan file (atomic, 0600) with the unmasked prompt read from the previous pending file (or `--prompt <file|->` when the item has no plan yet); omitted `tasks` are inherited; settings are loaded from the item's workspace; the item gets the new hash, `revision` +1, and lane `needs_decision` unless the item is in `auto`. Validation errors → exit 1 with the errors, item unchanged. |
| FR-3108 | Must | `dmuxai board decide <id> approve|reject|request_changes|answer [--text T] [--option N] [--plan-hash H] [--by owner|bot:NAME]` records `decision` (`verdict`, `text`, `option`, `plan_hash`, `by`, `at_ms`) and moves the lane: approve → `approved`, reject → `rejected`, request_changes/answer → stay (`needs_decision`) with the decision visible to the bot. When `--plan-hash` is given and differs from the current plan hash → exit 4 (conflict). Approving an item that has a plan requires `--plan-hash` equal to the current hash (a TTY owner may omit it). `request_changes` requires `--text`; `answer` requires `--text` or `--option`. OQ-3104 is enforced: `--by owner` from a non-TTY CLI or MCP → exit 5; a bot approving a non-`auto` item → exit 5. |
| FR-3109 | Must | `dmuxai board say <id> --text T|- [--from owner|bot:NAME|planner]` appends a thread message (`from`, `text`, `at_ms`). |
| FR-3110 | Must | `dmuxai board move <id> <lane>` and `dmuxai board update <id> [--title] [--summary] [--recommend] [--option]... [--grade] [--kind] [--run-id R] [--if-version N]` change an item; `move` also accepts `--if-version`; mismatch → exit 4. Only the owner may move an item into `auto` or `approved` (exit 5 otherwise); `planner` may only post messages and edit plans. |
| FR-3111 | Must | `dmuxai board list [--lane L]... [--workspace DIR] [--all] [--json]` lists items newest first (hiding `done`/`rejected` older than 30 days unless `--all`); `dmuxai board show <id> [--json]` prints one item including plan and thread. |
| FR-3112 | Must | `dmuxai board wait <id> [--since-version N] [--timeout SECS] [--for decision|message|any]` blocks until the item's version exceeds N (default: current) with a change of the requested kind made by someone other than the waiter (owner or planner), then prints `{"timed_out":false,"event":{kind,by,version},"item":{...}}`; events already logged before the wait started count when `--since-version` is lower than the current version; on timeout (default 600, max 3600) prints `{"timed_out":true,"version":N}` with exit 0. Events are only counted when `by` is `owner` or `planner`; a missing item → exit 3. `board wait --any [--since-seq S]` waits for the next such event on any item; `board events --since S [--json]` lists events for polling bots. Polling reads only `events.jsonl` growth (no full scan) at ≤ 1 s intervals. |
| FR-3113 | Must | MCP server exposes `board_add`, `board_list`, `board_show`, `board_update`, `board_move`, `board_plan` (attach by hash or edit by steps), `board_decide`, `board_say`, `board_wait` (timeout capped at 300 s per call), `board_prompt`, `board_events` with the same validation and exit-code mapping as the CLI (`isError` + code). |
| FR-3114 | Must | `dmuxai board --help` and per-subcommand help describe the commands; `docs/API.md` documents the CLI and MCP tools and the coordinator workflow. |
| FR-3115 | Must | Board operations never read or write agent credential files, never print the dashboard token, and do not store company or private data beyond what the caller sends (docs warn that the repo is public). |
| NFR-3100 | Must | Board code runs only for `board` commands/tools; `run`, the TUI and the dashboard have no added work in PR 1. `board list` with 500 items finishes in under 200 ms on the gate box. |
| FR-3120 | Must | HTTP API (`src/web/board_api.rs`, merged into both `--web` and `serve` routers behind the existing `security_and_auth_middleware`): `GET /api/board?lane=&workspace=&all=` (list), `GET /api/board/{id}`, `POST /api/board/{id}/decision` `{verdict,text?,option?,plan_hash}`, `POST /api/board/{id}/messages` `{text}`, `POST /api/board/{id}/plan` `{steps,tasks?}`, `POST /api/board/{id}/lane` `{lane}`, `GET /api/board/rules`, `PUT /api/board/rules`. Dashboard writes act as `owner` (BoardCaller::Dashboard). Errors map to HTTP: usage 400, not found 404, conflict 409, forbidden 403, failed 422, internal 500, as JSON `{error}`. The id path segment is validated with `valid_item_id` before any store call. Writes are allowed in view-only `serve` too (OQ-3102) because they launch no agent. |
| FR-3121 | Must | `GET /api/board/events?since=SEQ` is an SSE stream of board events (`seq`, `item_id`, `kind`, `version`, `by`) tailing `events.jsonl` (≤ 1 s poll of the file length, only while a client is connected); `since` replays later events. |
| FR-3122 | Must | Dashboard header gets a "Board" toggle (`#board-toggle`, `data-view="board"`) with a badge counting `needs_decision` items. The Board view shows (a) "My decisions": one card per `needs_decision` item with title, workspace basename, kind, grade, source bot, age, recommendation, options as buttons, and Approve / Request changes / Reject buttons; (b) a kanban with columns Auto, Needs decision, Approved, In progress, Done, Rejected, Unknown across all projects; (c) a workspace filter and a "show closed" switch. Cards can be moved between lanes with a lane menu (no drag-and-drop required). |
| FR-3123 | Must | Clicking a card opens a drawer: summary, plan revision and hash, plan steps table (id, role, runner, model, depends_on, job) editable inline with add/remove step and Save (calls `POST /plan`; validation errors shown per step, nothing saved); diff of steps against the previous revision when one exists; decision history; and a side chat panel (thread messages with sender labels and times, a textarea and Send; Ctrl/Cmd+Enter sends). Approve sends the plan hash currently shown so a stale approval gets 409 and a "plan changed, reload" notice. |
| FR-3124 | Must | The Board view updates live from the board SSE stream while it is visible and closes the stream when hidden; no timers run when the view is hidden. |
| FR-3125 | Must | All board text is inserted with `textContent` (no `innerHTML`), the dashboard token never appears in the Board view, and `board.js`/`board.css` are served like the other assets (no-store). |
| FR-3126 | Should | Rules editor in the Board view (list of rules with kind/grade/title regex → lane, default lane) saving through `PUT /api/board/rules`. |
| NFR-3120 | Must | Board API and assets add no work to other routes; listing 500 items over HTTP stays under 300 ms; the board stream costs nothing when no client is connected (#301). |

## 5. Acceptance scenarios and test design

| ID | Covers | Test |
|---|---|---|
| TC-3100 | FR-3100 | `tc_3100_item_roundtrip_atomic_write_and_permissions` |
| TC-3101 | FR-3101 | `tc_3101_events_seq_monotonic_under_concurrent_writers` |
| TC-3102 | FR-3102 | `tc_3102_invalid_ids_and_limits_rejected_before_fs_access` |
| TC-3103 | FR-3103 | `tc_3103_free_text_is_masked_on_write` |
| TC-3104 | FR-3104 | `tc_3104_rules_pick_initial_lane_defaults_and_invalid_file` |
| TC-3105 | FR-3105 | `tc_3105_board_add_cli_records_workspace_and_prints_id` |
| TC-3106 | FR-3106 | `tc_3106_attach_pending_plan_by_hash_missing_is_not_found` |
| TC-3107 | FR-3107 | `tc_3107_plan_edit_validates_writes_new_pending_and_bumps_revision` |
| TC-3108 | FR-3108 | `tc_3108_decide_verbs_lanes_stale_hash_conflict_and_bot_approve_forbidden` |
| TC-3109 | FR-3109 | `tc_3109_say_appends_thread_message` |
| TC-3110 | FR-3110 | `tc_3110_move_update_and_if_version_conflict` |
| TC-3111 | FR-3111 | `tc_3111_list_filters_and_hides_old_closed_items` |
| TC-3112 | FR-3112 | `tc_3112_wait_returns_on_owner_decision_and_times_out` |
| TC-3113 | FR-3113 | `tc_3113_mcp_board_tools_listed_and_mapped` |
| TC-3114 | FR-3114 | `tc_3114_board_help_lists_subcommands` |
| TC-3115 | FR-3115 | `tc_3115_board_never_touches_credentials_or_token` |
| TC-3116 | NFR-3100 | `tc_3116_list_500_items_under_budget` |
| TC-3120 | FR-3120 | `tc_3120_board_http_routes_status_codes_and_owner_actor` |
| TC-3121 | FR-3121 | `tc_3121_board_sse_replays_since_and_streams_new_events` |
| TC-3122 | FR-3122 | `tc_3122_board_view_markup_toggle_lanes_and_decision_cards` |
| TC-3123 | FR-3123 | `tc_3123_board_drawer_plan_edit_chat_and_stale_approve_conflict` |
| TC-3124 | FR-3124 | `tc_3124_board_stream_only_while_visible` |
| TC-3125 | FR-3125 | `tc_3125_board_assets_no_innerhtml_no_token` |
| TC-3126 | FR-3126 | `tc_3126_board_rules_get_put_validation` |
| TC-3127 | NFR-3120 | `tc_3127_board_http_list_500_under_budget` |

## 6. Data and safety contracts

- New user-level files: `board/items/*.json`, `board/events.jsonl`,
  `board/rules.json`, `board/.lock`. No repo file changes; nothing is written
  into a workspace.
- Pending plan files written by plan edit use the existing format, so
  `run --graph --approve-plan-id` re-validates them; the stored prompt must
  match the prompt given to that run (FR-2546).
- Access: CLI/MCP run as the OS user. The dashboard (PR 2) uses the existing
  security middleware and dashboard access rules.

## 7. Coordinator workflow (informative)

1. New work arrives from the manager bot. The coordinator runs
   `dmuxai -C <repo> run --graph --json -` with the task prompt; exit 7 returns
   `plan_hash`.
2. `dmuxai board add -C <repo> --title ... --kind debug|feature --plan-hash <hash> --bot coordinator --json`.
3. Lane `auto`: `dmuxai board prompt <id> | dmuxai -C <repo> run --graph --approve-plan-id <hash> -`,
   then `board move <id> in_progress` / `done`.
4. Lane `needs_decision`: `dmuxai board wait --any --since-seq <S> --timeout 600 --as bot:coordinator`
   in a loop (or `board wait <id>`). On `approve` run the decided `plan_hash` as in step 3; on `request_changes`
   or a message, revise (`board plan --edit` or a new planner run +
   `board plan --hash`) and reply with `board say`.

## 8. Planned requirements for PR 2 and PR 3 (not yet gated)

PR 3 (planner chat, run, notify):
- FR-3135: "Ask planner" (`dmuxai board ask <id> --text T`, dashboard button in `--web`): a read-only planner turn with the configured `[board] planner` runner/model gets the plan, thread and question and answers in the thread; a returned plan JSON becomes a new validated revision.
- FR-3136: `dmuxai board run <id>` executes the decided plan hash via `run --graph --approve-plan-id` in the item's workspace, records the run id, moves `in_progress` → `done`/`unknown` on exit.
- FR-3137: `[board.notify]` command/webhook (same delivery rules as `[notify]`) fires on `needs_decision`, `decision`, owner `message`.
