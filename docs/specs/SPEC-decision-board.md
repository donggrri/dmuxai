# SPEC: Decision board (coordinator board)

Status: in progress (series 3/3: Ask planner, board run, notify) · Target: v0.4.1
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
| OQ-3105 | PR 3 scope | Ask planner, `board run`, board notify hooks, and keeping a board copy of each plan so a failed run can be retried with the same hash. "Request digest" and "Send to coordinator" stay out (follow-ups). |
| OQ-3106 | Who may run planner chat / board run from the dashboard? | Only `--web` (it launches agents); view-only `serve` answers 403. CLI/MCP: any local caller; `board run` additionally needs lane `approved` (decided by the owner) or `auto`. |
| OQ-3104 | Who may approve? | Approval is always human-only (Dongjin, 2026-11-27). Only the owner may `decide` (every verdict: approve, reject, request_changes, answer) or `check`, in every lane. The owner is the dashboard (a browser request, FR-3138) or the CLI with stdin and stdout on a TTY, no `DMUXAI_RUN_ID`, and an explicit typed confirmation on the terminal (FR-3138). A non-TTY CLI, MCP, a headless run, a planner turn or `board run` can never act as owner (exit 5). No path moves an item into `approved` or `rejected` except an owner decision. This guards against accidental and agent self-approval; it is not authentication against a hostile local process running as the same OS user. |

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
| FR-3104 | Must | Rules live in `<state base>/board/rules.json`: an ordered list of `{kind?, grade?, title_regex?, lane}` plus `default_lane`. The first matching rule picks the initial lane when an item is created without an explicit lane. Missing file → built-in defaults (OQ-3101). An invalid rules file is reported and the defaults are used. Rules cannot target `approved` or `rejected` (usage error). `dmuxai board rules [--set <file>|-]` prints or replaces the rules (validated; regex size-limited; `--set` requires owner + TTY confirmation). `PUT /api/board/rules` requires browser headers (403 otherwise). |
| FR-3105 | Must | `dmuxai board add --title T [--summary S|-] --kind K [--grade G] [--lane L] [--bot NAME] [--channel C] [--ref URL] [-C DIR] [--recommend TEXT] [--option TEXT]... [--plan-hash H] [--key K] [--json]` creates an item; with `--key` an existing item with the same key and bot is returned instead of a duplicate; a non-owner `--lane auto|approved|rejected` is refused (exit 5), and owner `--lane auto` on CLI requires TTY confirmation — otherwise the rules choose the lane; `-C` (default: current dir) records the canonical workspace path and repo key; prints the item (JSON with `--json`, else the id). |
| FR-3106 | Must | `--plan-hash H` (and `board plan <id> --hash H`) attaches the pending plan `H`: the file is read (16-hex validated; missing → exit 3), its content hash must equal `H` (else exit 1); `plan` stores `hash`, a masked display copy of `prompt`, `tasks`, `steps`, `revision` (+1 per attach/edit) and `history` (previous hashes, max 20). The unmasked prompt stays only in the 0600 pending file. `dmuxai board prompt <id> [--hash H]` (MCP `board_prompt`) prints the unmasked prompt of the current plan (or of a hash in its history) so the bot can pipe it into `run --graph --approve-plan-id`. Attaching or editing a plan clears the previous decision and moves an `approved` item back to `needs_decision` (an `auto` item stays `auto`). |
| FR-3107 | Must | `dmuxai board plan <id> --edit <file|->` replaces the plan steps (and optional tasks) from a JSON document `{steps:[...], tasks:[...]}`: it is parsed with the graph plan parser, validated with `validate_plan_with_limit` against the current graph runners/limits (a pre-check; `run --graph --approve-plan-id` re-validates authoritatively), and on success written as a new pending plan file (atomic, 0600) with the unmasked prompt read from the previous pending file (or `--prompt <file|->` when the item has no plan yet); omitted `tasks` are inherited; settings are loaded from the item's workspace; the item gets the new hash, `revision` +1, and lane `needs_decision` unless the item is in `auto`. Validation errors → exit 1 with the errors, item unchanged. |
| FR-3108 | Must | `dmuxai board decide <id> approve|reject|request_changes|answer [--text T] [--option N] [--plan-hash H] [--by owner|bot:NAME]` records `decision` (`verdict`, `text`, `option`, `plan_hash`, `by`, `at_ms`) and moves the lane: approve → `approved`, reject → `rejected`, request_changes/answer → stay (`needs_decision`) with the decision visible to the bot. When `--plan-hash` is given and differs from the current plan hash → exit 4 (conflict). Approving an item that has a plan requires `--plan-hash` equal to the current hash (a TTY owner may omit it). `request_changes` requires `--text`; `answer` requires `--text` or `--option`. OQ-3104 is enforced: every verdict needs the owner (FR-3138); `--by owner` from a non-TTY CLI or MCP → exit 5; any bot or planner decision, in any lane → exit 5. |
| FR-3109 | Must | `dmuxai board say <id> --text T|- [--from owner|bot:NAME|planner]` appends a thread message (`from`, `text`, `at_ms`). |
| FR-3110 | Must | `dmuxai board move <id> <lane>` and `dmuxai board update <id> [--title] [--summary] [--recommend] [--option]... [--grade] [--kind] [--run-id R] [--if-version N]` change an item; `move` also accepts `--if-version`; mismatch → exit 4. Only the owner may move an item into `auto` (exit 5 otherwise). Nobody may `move` an item into `approved` or `rejected` (exit 5 with a hint to use `board decide`); those lanes are reached only through an owner decision. `planner` may only post messages and edit plans. |
| FR-3111 | Must | `dmuxai board list [--lane L]... [--workspace DIR] [--all] [--json]` lists items newest first (hiding `done`/`rejected` older than 30 days unless `--all`); `dmuxai board show <id> [--json]` prints one item including plan and thread. |
| FR-3112 | Must | `dmuxai board wait <id> [--since-version N] [--timeout SECS] [--for decision|message|any]` blocks until the item's version exceeds N (default: current) with a change of the requested kind made by someone other than the waiter (owner or planner), then prints `{"timed_out":false,"event":{kind,by,version},"item":{...}}`; events already logged before the wait started count when `--since-version` is lower than the current version; on timeout (default 600, max 3600) prints `{"timed_out":true,"version":N}` with exit 0. Events are only counted when `by` is `owner` or `planner`; a missing item → exit 3. `board wait --any [--since-seq S]` waits for the next such event on any item; `board events --since S [--json]` lists events for polling bots. Polling reads only `events.jsonl` growth (no full scan) at ≤ 1 s intervals. |
| FR-3113 | Must | MCP server exposes `board_add`, `board_list`, `board_show`, `board_update`, `board_move`, `board_plan` (attach by hash or edit by steps), `board_say`, `board_wait` (timeout capped at 300 s per call), `board_prompt`, `board_events` with the same validation and exit-code mapping as the CLI (`isError` + code). |
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
| FR-3130 | Must | Plan copies: whenever a plan is attached or edited (FR-3106, FR-3107) the board also writes `<state base>/board/plans/<hash>.json` (0600, same bytes as the pending file). When `board run` finds `graph-pending/<hash>.json` missing it restores it from that copy (re-checking that the copy's sha256 prefix equals the hash), so a failed run can be retried with the same hash. Copies whose items are `done`/`rejected` and older than 30 days may be pruned by `board run`; nothing else deletes them. FR-2546 itself is unchanged. |
| FR-3131 | Must | `[board]` table, read from the user config only (`settings::default_user_config_path_from`, normally `$XDG_CONFIG_HOME/dmuxai/dmuxai.toml`; a `[board]` table in a workspace `dmuxai.toml` is ignored with a config warning): `planner = "AGENT[:MODEL]"` (optional; default: the item workspace's normal planner selection), `notify_command = [argv...]` (optional), `notify_webhook_url` (optional http(s) URL), `notify_webhook_secret_env` (optional env var name), `notify_events` (subset of `needs_decision`, `decision`, `message`, `run_finished`; default all). Unknown keys warn. |
| FR-3132 | Must | `dmuxai board ask <id> --text T|- [--revise] [--agent A] [--model M] [--json]` (MCP `board_ask`, HTTP `POST /api/board/{id}/ask` `{text, revise?}`): first appends T to the thread as the caller (owner on dashboard/TTY, else `bot:NAME`/`--by`). Without `--revise` it runs a read-only single turn `dmuxai -C <workspace> run --read-only --json -` (agent/model from flags, else `[board] planner`) whose prompt holds the item title, summary, current plan steps (masked), the last 20 thread messages and T, and appends the answer (masked, truncated to 8000 chars) as a `planner` message. With `--revise` it runs `dmuxai -C <workspace> run --graph --json -` with the plan's original prompt plus a `Owner feedback:` section (T and the last 20 thread messages); on exit 7 the returned `plan_hash` is attached as a new revision (FR-3106 validation and copy, FR-3130) and a `planner` message names the new revision, hash and step count; on any other exit a `planner` message reports the exit code and no revision is made. An item without a workspace is a usage error; an item without a plan cannot `--revise` (usage). Child processes get the item workspace as cwd, inherit the environment except `DMUXAI_WEB_TOKEN`, and are killed after 900 s (reported as exit 124). The HTTP route returns 202 at once and posts results through the thread (SSE); at most one ask per item runs at a time (second → 409). |
| FR-3133 | Must | `dmuxai board run <id> [--detach] [--json]` (MCP `board_run`, HTTP `POST /api/board/{id}/run` with optional `{plan_hash}`): allowed when the lane is `approved` and the decision's plan hash equals the current plan hash, or when the lane is `auto` and the item has a plan; otherwise conflict (exit 4 / 409). It restores the pending file if needed (FR-3130), moves the item to `in_progress` (as the caller), then runs `dmuxai -C <workspace> run --graph --approve-plan-id <hash> --json -` with the pending file's prompt on stdin. The run id from the child's JSON result is appended to `run_ids` and a `run` thread message; on exit 0 the item moves to `done`, on any other exit to `unknown` with a message carrying the exit code and the child's summary (masked, ≤ 2000 chars). An item already `in_progress` with a live run started by this board is a conflict. `--detach` (and HTTP, which returns 202) returns after the move to `in_progress`; the result is still recorded. **Amended by SPEC-board-approval-hardening FR-3806:** the result moves the item to `done`/`unknown` only while it is still `in_progress` on the plan hash that ran; otherwise only a `run` thread message records the exit code. |
| FR-3134 | Must | The dashboard drawer gets an "Ask planner" area (question box, "Ask" and "Ask & revise plan" buttons, a busy indicator while the ask runs) and a "Run" button shown for runnable items; both are hidden in view-only `serve` (the API also returns 403 there: `--web` only). Results arrive through SSE (FR-3124); new revisions refresh the plan table and diff. |
| FR-3135 | Must | Board notifications: when `[board]` has `notify_command` or `notify_webhook_url`, events `needs_decision` (item created in or moved to `needs_decision`), `decision`, owner `message` and `run_finished` are delivered with the same delivery rules as `[notify]` (FR-770..FR-772: command gets the JSON on stdin with no shell, webhook POST with secret header from the env var, timeouts, log with secrets/query strings masked) to `<state base>/board/notify.log`. Payload: `{schema:"dmuxai.board.event.v1", event, item_id, title, lane, kind, workspace_path, by, plan_hash, at_ms}` (masked text only; never the plan prompt or thread bodies). Delivery happens in the process that made the change, on a background thread, and never blocks or fails the board write. |
| FR-3137 | Must | `dmuxai board pending [--workspace DIR] [--json]` (MCP `board_pending`, read-only; HTTP `GET /api/board/pending`) lists what the human must act on, across all workspaces, oldest first: items in `needs_decision` (reason `decide`), items in `unknown` (reason `failed`) and items in `done` that have no `checked` record (reason `check`), excluding `done`/`unknown` items older than 30 days. Text output: one line per item with id, reason, workspace basename, title (truncated to 80 chars) and age (`5m`, `3h`, `2d`), then a count line; `--json` prints `{items:[{id, reason, lane, workspace_path, repo_key, title, kind, grade, age_ms, updated_ms}], count}`. `dmuxai board check <id> [--text T]` (owner only, FR-3138; HTTP `POST /api/board/{id}/check`) records `checked` (`by`, `at_ms`, `text`) on a `done` or `unknown` item, appends a `checked` event, and removes it from `pending`; any other lane → conflict (exit 4). The dashboard drawer shows "Mark checked" for unchecked `done`/`unknown` items and the My decisions area shows a "To check" group. |
| FR-3138 | Must | Human-only enforcement (OQ-3104), applied in the store so CLI, MCP and HTTP share it: `decide`, `check`, `add --lane auto`, `rules --set`, and `move` into `auto` require actor `owner`; `move` into `approved`/`rejected` is refused for everyone. CLI owner actions additionally ask on the terminal `Type the item id to confirm <verb> <title>:` (read from the TTY, not from piped stdin) and proceed only when the typed text equals the item id; there is no flag that skips it. Headless runs, `board` child runs/asks, detached runs, and `term` sessions opened from a run export `DMUXAI_RUN_ID`, preventing PTY owner self-approval. HTTP owner writes (`decision`, `check`, `lane`, `PUT /api/board/rules`) require browser fetch metadata (`Sec-Fetch-Site: same-origin` and `Sec-Fetch-Mode` present) and the header `X-Dmuxai-Human: 1`, which `board.js` sends only from a click or key handler; other requests → 403. `board run`, `board ask` (both modes), the auto lane and notifications never write a `decision` or `checked` record and never move an item into `approved`/`rejected`. MCP has no decide or check tool; `board_decide` (PR 1) is removed and returns an unknown-tool error. **Amended by SPEC-board-approval-hardening FR-3803:** a non-owner `move` into `in_progress` is also refused (exit 5 / 403) unless the item is in `auto` or approved by the owner for its current plan hash. |
| NFR-3130 | Must | PR 3 adds no work to `run`, the TUI or other routes unless a board ask/run is in progress; with no `[board]` notify target no notifier thread starts. Board list over CLI/HTTP keeps its PR 1/PR 2 budgets. |
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
| TC-3130 | FR-3130 | `tc_3130_plan_copy_written_and_restores_missing_pending` |
| TC-3131 | FR-3131 | `tc_3131_board_config_user_only_and_validation` |
| TC-3132 | FR-3132 | `tc_3132_board_ask_answer_and_revise_with_fake_runner` |
| TC-3133 | FR-3133 | `tc_3133_board_run_lane_checks_records_run_and_moves_lane` |
| TC-3134 | FR-3134 | `tc_3134_drawer_ask_and_run_controls_hidden_in_serve` |
| TC-3135 | FR-3135 | `tc_3135_board_notify_command_payload_masked_and_non_blocking` |
| TC-3137 | FR-3137 | `tc_3137_board_pending_lists_decide_failed_check_and_check_clears` |
| TC-3138 | FR-3138 | `tc_3138_human_only_decisions_cli_mcp_http_and_run_never_approves` |
| TC-3136 | NFR-3130 | `tc_3136_no_notifier_without_board_config` |

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
   in a loop (or `board wait <id>`). On `approve` the board starts the run itself (SPEC-board-approve-autorun; the lane is `in_progress`), so only run `dmuxai board run <id>` when `[board] auto_run_on_approve = false` or the decision reported `auto_run.started = false`; on `request_changes`
   or a message, revise with `dmuxai board ask <id> --revise --text ...` (or `board plan --edit`)
   and reply with `board say`.
5. Lane `auto`: `dmuxai board run <id>` replaces the manual pipe in step 3. A failed run moves the
   item to `unknown`; the owner approves again (`board decide <id> approve` on the dashboard or a TTY)
   or a revised plan is approved, then `board run` retries with the same hash (FR-3130).
6. Owners (and the coordinator, to remind them) see everything waiting on a human with
   `dmuxai board pending`; the coordinator never decides or checks for the owner.
7. Configure `[board] notify_command` / `notify_webhook_url` in the user config so the owner hears
   about new decisions without watching the dashboard.

