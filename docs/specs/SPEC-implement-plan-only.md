# SPEC: Implement plan-only and AGY terminal lifecycle

Status: Implemented
Owner: DmuxAI
Created: 2026-10-17
Baseline: `main`
Related: SPEC-permission-hardening, SPEC-agy-idle-timeout, dogfood #24

## 1. Decision summary

Implement mode must apply file edits, not plans. AGY write detection must use parsed tool-call events (not init-line text matches). Terminal adapter events (`Completed`, `Cancelled`, `Error`) must be emitted only after the provider child exits, with a short post-`result` grace period before kill. Plan-only Implement outcomes (no writes + plan-like reply) retry once with a stricter prompt and conversation resume, then fail the turn and graph node. Concurrent sends while a headless turn is still running surface as `Error`, not `Notice`. Cancel always clears app busy/active-turn tracking.

## 2. Scope

### In scope

- AGY adapter streaming, Implement prompt, plan-only retry
- `implement_outcome` write detection for AGY NDJSON
- Codex, Cursor, AGY, custom adapters: already-running concurrent send
- App cancel and turn completion alignment

### Out of scope / non-goals

- Changing Ask/Plan sandbox semantics
- Auto-elevating permissions or new provider protocols

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | Plan-only detection uses text heuristics on the aggregated response; false positives retry once then fail. | DmuxAI | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Plan-only outcome | Implement turn with no observed writes and a plan-like assistant reply | Plan mode success |
| Pending terminal | Parsed `result` held until child exit | Mid-stream `Activity` events |
| Grace period | Time after `result` before killing a lingering child | Idle timeout |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-500 | Must | Given Implement mode, when the user submits a prompt | The AGY prompt requires applying edits directly and forbids plan artifacts and review-before-edit language | Ask/Plan prompts unchanged | TC-500, TC-502 |
| FR-501 | Must | Given AGY Implement NDJSON, when a `step_update` tool call uses a write tool name | `ImplementTurnTracker` marks writes observed | Init/tool-list lines without an active write tool call do not count | TC-501 |
| FR-502 | Must | Given Implement with no writes and plan-like reply on first attempt | Adapter resumes the conversation once with a stricter Implement instruction | Second plan-only attempt fails the turn with a recorded reason | TC-503, TC-504 |
| FR-503 | Must | Given AGY stdout delivers `result`, when the child has not exited | No terminal `Completed`/`Error`/`Cancelled` is emitted until after process exit | After grace seconds post-`result`, child is killed and exactly one terminal event is emitted | TC-505 |
| FR-504 | Must | Given a headless adapter with an active child turn, when a second `Send` arrives | An `Error` (not `Notice`) is emitted for the new request context | App may finish the tracked turn on terminal errors | TC-506 |
| FR-505 | Must | Given the user presses cancel, when an active turn is tracked or the agent is busy | App busy state and `active_turns` are cleared and adapter cancel is invoked | Adapter cancel errors surface only when a turn was tracked | TC-507 |
| FR-506 | Must | Given plan-only failure after retry | Graph node and turn finish as failed with the plan-only reason | Success path unchanged when writes are observed | TC-504 |
| FR-507 | Should | Given Plan mode plan-like reply | User sees the existing plan-review notice; terminal completion still deferred until exit | Implement does not immediately error on first plan-like `result` line | TC-508 |
| FR-508 | Should | Given permission-hardening write checks | Implement turns without writes still fail when the reply is not plan-only | Ask/Plan never require writes | TC-509 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-500 | FR-500 | unit | Implement prompt builder | Inspect `agy_prompt(Implement, …)` | Contains apply-edits and no-plan language | `src/agent/agy.rs` `implement_prompt_forbids_background_commands` |
| TC-501 | FR-501 | unit | AGY tool `step_update` NDJSON | Parse write tools vs init line | Writes from tool names only | `src/agent/implement_outcome.rs` `agy_write_tools_are_detected_from_step_updates`, `init_line_does_not_count_as_write_via_text_scan` |
| TC-502 | FR-500 | unit | Implement prompt | Compare to Ask/Plan | Ask/Plan omit implement-only clauses | `implement_prompt_forbids_background_commands` |
| TC-503 | FR-502 | unit | Plan-only classifier | Text fixtures | Detects plan.md / review patterns | `plan_review_response_detection_covers_implement_plan_only`, `plan_only_outcome_requires_missing_writes_and_plan_like_text` |
| TC-504 | FR-502, FR-506 | integration | Fake AGY script returning plan-only twice | `run_turn` Implement | Second attempt returns error containing `plan-only` | `agy_plan_only_retries_once_then_fails` |
| TC-505 | FR-503 | integration | Fake AGY prints `result` then sleeps | `run_turn` Ask | Completes within grace + kill window | `agy_result_grace_kills_lingering_child_after_result` |
| TC-506 | FR-504 | unit | Normalized `result` handling | `normalize_event` on `result` | No immediate terminal in event list | `result_event_defers_terminal_until_process_exit` |
| TC-507 | FR-505 | unit | App with `active_turns` only | Ctrl+C cancel | Busy cleared; adapter cancel called | `cancel_clears_active_turn_even_when_busy_flag_was_cleared` |
| TC-508 | FR-507 | unit | Plan mode plan-like `result` | `normalize_event` | Notice + deferred `Completed` | `marks_plan_review_responses_as_a_notice` |
| TC-509 | FR-508 | unit | Implement tracker, no writes | `finish(..., require_writes: true)` | Fails with no observed file changes | `implement_without_writes_fails` |

## 6. Data, integration, and safety contracts

- Failure reasons are plain text on the turn; no secrets from stderr beyond provider output.

## 7. Implementation plan

See [PLAN-implement-plan-only.md](../plans/PLAN-implement-plan-only.md).

## 8. Review and test exit criteria

- All TC-500..509 pass under `cargo test --all-targets` and `cargo clippy --all-targets --all-features -- -D warnings` (including Windows target cfg tests where present).
- Review recorded in [REVIEW-implement-plan-only.md](../reviews/REVIEW-implement-plan-only.md).
