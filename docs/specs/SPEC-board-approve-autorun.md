# SPEC: Wake the bot and auto-run on owner approval

Status: approved · Target: v0.4.2
Owner: DmuxAI
Created: 2026-11-27
Baseline: `77d4fbc` (v0.4.1)
Related: SPEC-decision-board (FR-3110 wait, FR-3133 board run, FR-3135 notify, FR-3138 human-only)

## 1. Problem

After the owner approves a board item, nothing happens until the coordinator
bot polls and then calls `board run`. The owner expects approval to mean
"go": the bot that posted the item should hear about it at once, and an
approved plan should start running without another click.

## 2. Decisions

| ID | Topic | Decision |
| --- | --- | --- |
| OQ-3230 | Who starts the run | The process that recorded the owner's approve decision (dashboard `--web` server or TTY `board decide`) starts a detached `board run` right after the decision is saved. No other path auto-runs. |
| OQ-3231 | Opt-out | `[board] auto_run_on_approve = false` turns auto-run off (default `true`). |
| OQ-3232 | Bot wake-up | Reuse the existing decision event (events log + `[board]` notify); the notify payload gains `bot` and `verdict` so a hook can route the wake-up to the posting bot. No new channel. |

## 3. Requirements

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-3230 | Must | Every owner decision appends the `decision` event before the call returns, so a `board wait <id> --for decision` (CLI/MCP `board_wait`) blocked on that item returns within one poll interval (≤ 1 s) with the decision; `board wait --any` likewise. |
| FR-3231 | Must | The board notify payload (FR-3135, schema `dmuxai.board.event.v1`) adds optional `bot` (the item's `--bot` value, masked) and, for `decision` events, `verdict` (`approve`/`reject`/`request_changes`/`answer`) and, when an auto-run was started, `auto_run: true`. A `decision` by the owner is delivered to `notify_command`/`notify_webhook_url` when configured, as today; no thread bodies or plan prompt are included. |
| FR-3232 | Must | After an owner `approve` decision is saved on an item that has a plan, and the decision's `plan_hash` equals the item's current plan hash, and `[board] auto_run_on_approve` is not `false`, the deciding process starts `board run` detached for that item (`run_item_prepare` with the approved hash as `expected_plan_hash`, then `launch_detached_board_run`), acting as `owner`. The item moves to `in_progress` as in FR-3133. |
| FR-3233 | Must | Auto-run never writes a `decision` or `checked` record and never moves an item into `approved`/`rejected` (FR-3138). It is triggered only from the owner approve paths: HTTP `POST /api/board/{id}/decision` with verdict approve (after the human-browser checks) and TTY `board decide <id> approve` (after the typed id confirmation). MCP, `board say/ask/plan/update/move`, rules, notify hooks and child runs never trigger it. |
| FR-3234 | Must | Auto-run keeps every FR-3133 check: if the plan changed after the decision, a run/ask is already active, the item has no workspace, or the lane is not `approved` with a matching hash, it is not started and a `run` thread message `auto-run skipped: <reason>` is added (masked); the decision itself still succeeds (HTTP 200 / exit 0). |
| FR-3235 | Must | View-only `serve` (no agent launch) never auto-runs: the decision is saved and a `run` thread message `auto-run skipped: view-only dashboard` is added. |
| FR-3236 | Must | The HTTP decision response and `board decide --json` include `auto_run: {started: bool, reason?: string}`; the dashboard shows "Approved — run started" or the skip reason. |
| FR-3237 | Should | README board section and the coordinator guidance in SPEC-decision-board §7 tell bots: after `board wait` returns an approve decision, check the item lane; if it is `in_progress` the board already started the run — do not call `board run` again (it would get a conflict). |
| NFR-3230 | Must | Auto-run adds no work to non-approve decisions and starts no thread when `auto_run_on_approve = false`. The decision write is not delayed by the run (detached spawn only). |

## 4. Tests

| ID | Covers | Test |
| --- | --- | --- |
| TC-3230 | FR-3230 | `tc_3230_owner_decision_wakes_blocked_wait_within_one_second` |
| TC-3231 | FR-3231 | `tc_3231_decision_notify_payload_has_bot_verdict_and_auto_run` |
| TC-3232 | FR-3232, FR-3236 | `tc_3232_owner_approve_with_matching_hash_starts_detached_run` |
| TC-3233 | FR-3233 | `tc_3233_auto_run_only_from_owner_approve_paths` |
| TC-3234 | FR-3234 | `tc_3234_auto_run_skipped_when_plan_changed_or_busy` |
| TC-3235 | FR-3235, NFR-3230 | `tc_3235_view_only_and_opt_out_never_auto_run` |
| TC-3237 | FR-3237 | `tc_3237_readme_documents_auto_run_on_approve` |
