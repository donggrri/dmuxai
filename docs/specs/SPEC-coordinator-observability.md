# SPEC: Coordinator observability for a manager bot

Status: implemented (approved Dongjin 2026-11-28)
Owner: Dongjin
Created: 2026-11-28
Baseline: `068b920`
Related: SPEC-decision-board FR-3101 FR-3112 FR-3113 FR-3135, SPEC-board-approve-autorun FR-3231, SPEC-headless-runs-dashboard FR-2800..FR-2817, SPEC-agent-api, SPEC-spec-review-gate (#372)

## 1. Decision summary

A manager bot (Grok Bot) drives dmuxai as the coordinator and must learn, without
a browser, when work starts, when a human decision is waiting, and what a run is
doing. Board notifications only cover `needs_decision`, `decision`, owner
`message` and `run_finished`, and run details are only on the dashboard HTTP
API. This spec adds board events and notifications for `run_started`,
`awaiting_approval` and `spec_review_requested`, and `dmuxai runs list|show`
CLI plus MCP `runs_list`/`runs_show` that read the same run journal as
`GET /api/runs`.

## 2. Scope

### In scope

- New board event kinds and notify event names; payload fields.
- A public hook the spec review gate calls to raise `spec_review_requested` for the item whose run paused.
- `runs list|show` CLI and MCP tools; docs.

### Out of scope / non-goals

- Implementing the spec review gate itself (SPEC-spec-review-gate, separate PR).
- Push channels other than the existing command/webhook hooks.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3850 | `spec_review_requested` is emitted only once the spec review gate exists; until then the hook is present and tested with a fake caller | Dongjin | Answered 2026-11-28 |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| board event | One line in `<state base>/board/events.jsonl` (FR-3101) | Workspace `events.jsonl` |
| awaiting approval | A board item whose plan waits for the owner (created in or moved to `needs_decision` with a plan, or a new plan revision attached while in `needs_decision`) | A headless run manifest status `awaiting_approval` (reported by `runs`) |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3850 | Must | Given a board run moves an item to `in_progress` and starts the child | Then a board event `run_started` (`item_id`, `version`, `by`, `plan_hash`) is logged and, when allowed by `[board] notify_events`, delivered as notify event `run_started` | — | TC-3850 |
| FR-3851 | Must | Given an item with a plan enters `needs_decision` or gets a new plan revision while in `needs_decision` | Then a board event and notify event `awaiting_approval` is emitted with `plan_hash` and `plan_revision` (once per revision) | Items without a plan emit only the existing `needs_decision` | TC-3851 |
| FR-3852 | Must | Given a caller invokes the hook `board::notify_spec_review_requested(workspace, run_id, review_id, spec_paths)` | Then the board item whose session link (or run_ids) holds `run_id` gets a `spec_review` thread message and a board/notify event `spec_review_requested` with `review_id` and up to 20 workspace-relative `spec_paths`; with no matching item the board is not changed. The board sets `DMUXAI_BOARD_ITEM_ID=<item id>` on the child of `board run` (also auto-run and `--exec-prepared`), which agent children of that run do not inherit; when the hook's process has it set to a valid item id whose item is `in_progress`, that item is used, otherwise `run_ids` are matched | Never blocks the caller; errors are logged to `<state base>/board/notify.log` | TC-3852 |
| FR-3853 | Must | Given `[board] notify_events` | Then it accepts the new names `run_started`, `awaiting_approval`, `spec_review_requested`; the default (unset) delivers all events as before plus the new ones | Unknown names stay a config warning | TC-3853 |
| FR-3854 | Must | Given `board wait --any` / MCP `board_wait` / `board events` | Then the new event kinds are returned like the existing ones (`wait_for any` wakes on them; `decision`/`message` filters do not) | — | TC-3854 |
| FR-3855 | Must | Given `dmuxai runs list [--status S]... [--limit N] [--json]` (MCP `runs_list`) | Then it lists the workspace's run manifests newest first with the same fields as `GET /api/runs` (FR-2806: run_id, origin, status, runner/model, created_at_ms, elapsed_ms, exit code, error_kind, graph_id, stale, stalled); text output one line per run | Default limit 50, max 200; missing runs dir → empty list; invalid status → exit 2 | TC-3855 |
| FR-3856 | Must | Given `dmuxai runs show <run_id> [--progress N] [--json]` (MCP `runs_show`) | Then it prints the manifest, the graph view when present, `approval {plan_hash, hint}` when awaiting approval, and the last N (default 50, max 200) progress lines, as FR-2807 | Unknown id → exit 3; invalid id → exit 2; strings masked; read-only | TC-3856 |
| FR-3857 | Must | Given docs/API.md | Then §1 lists `runs`, §3 lists `runs_list`/`runs_show`, the Decision board section lists the new event kinds, and a short "Manager bot loop" subsection shows how a bot polls `board_pending`/`board_wait --any` and `runs_show` | — | TC-3857 |
| NFR-3850 | Must | Board writes and `run` | New events add one appended line per transition; no thread started when no notify target is set | — | TC-3850 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3850 | FR-3850, NFR-3850 | unit | Approved item, fake child, notify command capturing stdin | `board run` | `run_started` event and notify payload | `src/board/runner.rs` `tc_3850_run_started_event` |
| TC-3851 | FR-3851 | unit | Item with plan | add in needs_decision; attach new revision; add without plan | `awaiting_approval` once per revision; none without plan | `src/board/store.rs` `tc_3851_awaiting_approval_event` |
| TC-3852 | FR-3852 | unit | Item with run id R; `in_progress` item E without R; item N in `needs_decision`; fake board child | Call hook with R; with unknown run; with `DMUXAI_BOARD_ITEM_ID` = E, N, invalid, empty; `board run`; spec review gate (SPEC-spec-review-gate) enters `awaiting_spec_review`, is polled, gets changes requested and approved | Thread message + event; unknown → no change; env E → E, env N/invalid/empty → R's item; run child sees the item id, agent and ask children do not; the gate calls the hook once per pending round with review id, spec paths and board item id, one `spec_review_requested` event and notification, an unknown item does not fail the run | `src/board/notify.rs` `tc_3852_spec_review_requested_hook`, `tc_3852_spec_review_hook_uses_board_item_env`; `src/board/runner.rs` `tc_3852_board_run_child_gets_board_item_id_env`; `src/process/mod.rs` `tc_3852_board_item_env_not_inherited_by_agent_children`; `src/graph/spec_review.rs` `tc_3852_spec_review_wait_point_calls_hook_once_per_pending_round`, `tc_3852_spec_review_wait_point_raises_single_board_event` |
| TC-3853 | FR-3853 | unit | Settings parsing | New names; unknown name | Accepted; warning | `src/settings.rs` `tc_3853_notify_events_new_names` |
| TC-3854 | FR-3854 | unit | Waiter with `any` and `decision` | Emit run_started | any wakes; decision does not | `src/board/store.rs` `tc_3854_wait_any_sees_new_kinds` |
| TC-3855 | FR-3855 | cli + mcp | Temp workspace with 3 manifests | `runs list --json`, `--status failed`, MCP | Order, fields, filter; empty dir → [] | `src/cli.rs` `tc_3855_runs_list`, `src/mcp_server.rs` `tc_3855_mcp_runs_list` |
| TC-3856 | FR-3856 | cli + mcp | Manifest awaiting approval with progress lines | `runs show --json`, MCP | approval hint, progress tail; unknown → 3 | `src/cli.rs` `tc_3856_runs_show`, `src/mcp_server.rs` `tc_3856_mcp_runs_show` |
| TC-3857 | FR-3857 | doc check | docs/API.md | grep | Sections present | `tests/spec_index.rs` `tc_3857_api_docs_runs_and_events` |
