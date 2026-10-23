# SPEC: Dashboard follow-ups while a reply is streaming

Status: implemented (review pending)
Owner: Dongjin Shin
Created: 2026-10-21
Baseline: `9b3a5b9` (fix/chat-duplicate-send, PR #82)
Related: SPEC-dashboard-parity FR-782/FR-783, PR #82 prompt ids

## 1. Decision summary

While an agent run is active, the dashboard used to refuse a new prompt as
busy. Following Cursor's agent UI, the composer now stays usable during a run:
a prompt sent then is queued as a follow-up, shown in the chat as a queued
user message, and started automatically, in order, when the run ends. Queued
follow-ups can be removed or edited before they start. Stop keeps the queue.

## 2. Scope

### In scope

- Writable dashboard (`dmuxai --web`): queue, show, remove, edit, auto-start.
- Placeholder "Send follow-up" and a Stop button next to Send during a run.

### Out of scope / non-goals

- The TUI does not show or edit the queue (it keeps working as before).
- Queued follow-ups are not persisted across a dmuxai restart.
- Slash commands are never queued; they still run at once (FR-783).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1230 | Stop keeps the queue (no question asked). Decided by the request: the user already said what to do next by queueing it; remove items with x. | Dongjin | Done |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Run active | A turn, triage judge, planner, summary, graph run or deferred turn is in progress | A plan waiting for approval (not active, but it blocks the queue) |
| Follow-up | A dashboard prompt sent while a run is active | A TUI prompt |

Follow-up states: `queued` -> `started` (terminal) or `queued` -> `removed`
(terminal). Editing keeps it `queued`.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1230 | Must | Writable dashboard, run active, user sends a prompt | 202 with `queued: true` and a `prompt_id`; no busy refusal; nothing reaches the agent | Queue full (20): 409 busy | TC-1230, TC-1236 |
| FR-1231 | Must | Follow-up queued | The chat shows it once as a user message marked `queued`, after the working reply | It never enters the conversation or agent context until it starts | TC-1230, TC-1236 |
| FR-1232 | Must | Several follow-ups queued | They start one at a time in the order sent, each after the previous run ended | Follow-ups of another conversation wait for that conversation | TC-1230, TC-1234 |
| FR-1233 | Must | Follow-up still queued | x removes it; Edit changes its text (Save/Cancel) | Already started or removed: error, no change. Empty text or a slash command: refused | TC-1231, TC-1236 |
| FR-1234 | Must | User presses Stop or Cancel during a run | The run is cancelled; the queue is kept and the next follow-up starts after the cancelled run ended | | TC-1232, TC-1236 |
| FR-1235 | Must | View-only dashboard | Cannot queue (busy, as before) and cannot remove or edit (403) | | TC-1233, TC-1235 |
| UX-1236 | Must | Run active on a writable dashboard | Placeholder "Send follow-up"; Stop button next to Send (same cancel path) | Idle: original placeholder, no Stop | TC-1236 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1230 | FR-1230, FR-1231, FR-1232 | unit | Direct turn running | Send two follow-ups, finish turns | Queued in order, not sent or in messages; start one by one | `followup_during_run_is_queued_and_runs_in_order` |
| TC-1231 | FR-1233 | unit | Three queued | Remove one, edit one | Edited text runs, removed never does; late edit refused | `queued_followup_can_be_removed_or_edited_before_it_starts` |
| TC-1232 | FR-1234 | unit | One queued | Cancel the run | Queue kept, next starts after the cancel | `stop_keeps_the_followup_queue` |
| TC-1233 | FR-1235 | unit | Run active | Submit without queue permission | Busy, nothing queued | `followup_without_queue_permission_is_refused_as_busy`, `dashboard_slash_commands_via_prompt` |
| TC-1234 | FR-1232 | unit | Queued item of another conversation | Run ends | It does not start | `queued_followup_stays_with_its_conversation` |
| TC-1235 | FR-1235 | integration | Router with and without `web_attached` | POST prompt, queue_remove, queue_edit | allow_queue follows web_attached; 403 view-only | `view_only_dashboard_cannot_queue_followups` |
| TC-1236 | FR-1230, FR-1231, FR-1232, FR-1233, FR-1234, UX-1236 | headless browser | Real server and assets, fake app | Send during run, x, Edit, Stop | Badges, order, remove, edit, Stop keeps queue | `cargo test --test web_followup_queue -- --ignored` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change; the queue lives in memory | Revert the PR | — |
| Web API | `POST /api/prompt` reply gains `queued` (only when true); session view gains `run_active` and message `queued`; commands `queue_remove {id}`, `queue_edit {id,text}` | Additive; old clients ignore the fields | TC-1235 |
| Security / permissions | Queue commands need a writable dashboard | Same gate as plan decisions | TC-1235 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Queue, flush after each run (run loop and tick), remove/edit | `src/app.rs` | FR-1230..1235 | TC-1230..1234 |
| 2 | API fields, commands, write gate | `src/web/{commands,server,snapshot,hub}.rs` | FR-1230, FR-1235 | TC-1235 |
| 3 | Composer placeholder and Stop; queued messages with Edit and x | `src/web/assets/session.{js,css}` | FR-1231, FR-1233, UX-1236 | TC-1236 |

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] Format, all-targets test and clippy pass.
- [x] Live check with a slow fake `cursor-agent` (recorded in the PR).
