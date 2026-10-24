# SPEC-graph-defer-queue

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `18f2913`
Related: refactor scan finding #5, PR-8c

## 1. Decision summary

The graph busy-runner deferral queue and its retry/backoff timing in `src/app.rs` are refactored into a pure, App-free module (`src/graph_defer.rs`) as part of PR-8 ("extract a session engine from app.rs").
DmuxAI uses graph deferral queues when an agent runner is busy executing a turn or step, retrying node dispatches with backoff intervals. Previously, `App` owned three loose maps (`graph_deferred_starts: HashMap<Agent, VecDeque<NodeAssignment>>`, `graph_defer_retry_after: HashMap<String, Instant>`, `graph_defer_since: HashMap<String, Instant>`) and embedded the deferral, polling, timeout, and queue maintenance logic directly in methods like `defer_graph_node_start`, `flush_deferred_graph_starts`, `remove_deferred_assignment_for_node`, and `clear_graph_defer_state`.
PR-8c introduces:
1. `GraphDeferQueue` encapsulating the three maps and constants `GRAPH_DEFER_RETRY_INTERVAL` and `GRAPH_DEFER_MAX_WAIT`.
2. Pure, unit-testable methods accepting an explicit `now: Instant`: `clear`, `is_empty`, `has_queued`, `agents`, `remove_node`, `defer`, `mark_waiting_behind_busy`, `poll` returning `DeferPoll`, and `node_started`.
3. Replacement of the three loose fields on `App` with a single `graph_defer: GraphDeferQueue`, preserving all App-level side effects.

## 2. Scope

### In scope

- Pure `GraphDeferQueue` struct owning `starts`, `retry_after`, and `since` maps.
- Constants `GRAPH_DEFER_RETRY_INTERVAL` and `GRAPH_DEFER_MAX_WAIT` moved to `src/graph_defer.rs`.
- `DeferPoll` enum (`Empty`, `NotYet`, `TimedOut(NodeAssignment)`, `Ready(NodeAssignment)`).
- Explicit `now: Instant` parameter on timing methods (`defer`, `mark_waiting_behind_busy`, `poll`).
- `App` field migration from three maps to `graph_defer: GraphDeferQueue`.
- Behavior-preserving integration in `defer_graph_node_start`, `flush_deferred_graph_starts`, `clear_graph_defer_state`, `busy_retry_pending`, `remove_deferred_assignment_for_node`, and `start_graph_node`.
- Unit tests `tc_2040_defer_sets_since_once_and_retry_after`, `tc_2041_poll_not_yet_ready_timed_out`, `tc_2042_mark_waiting_behind_busy_resets_since`, and `tc_2043_remove_node_and_node_started`.

### Out of scope / non-goals

- Moving TASKS-file synchronization out of `src/app.rs` (follow-up in PR-8).
- Moving replan triggers into `GraphRunSession` (follow-up in PR-8).
- Moving `dispatch_graph_actions` into `GraphRunSession` (follow-up in PR-8).
- Changing retry interval durations or max wait timeout values.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2040 | `GraphDeferQueue` methods take explicit `Instant` parameters to ensure deterministic unit testing without wall-clock sleeps. | dmuxai | PR-8c implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| GraphDeferQueue | Dedicated queue managing deferred graph node dispatches, retry schedules, and expiration. | Interactive turn queues or internal turn buffers. |
| DeferPoll | Explicit polling outcome for an agent's deferred queue (`Empty`, `NotYet`, `TimedOut`, `Ready`). | Boolean dispatch flags. |
| Waiting behind busy | State where a runner is occupied by an active turn; timeout is paused by resetting `since`. | Rejection or refusal by an idle adapter. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2040 | Must | Given an agent, `NodeAssignment`, and `now: Instant` | `defer(agent, assignment, now)` inserts `now` into `since` only on first deferral, sets `retry_after = now + GRAPH_DEFER_RETRY_INTERVAL`, and pushes `assignment` to the agent queue iff not already queued | Re-deferring an existing node updates `retry_after` without duplicating the queued assignment or overwriting initial `since` | TC-2040 |
| FR-2041 | Must | Given an agent defer queue and `now: Instant` | `poll(agent, now)` returns `Empty` if no nodes queued; `NotYet` if `now < retry_after`; `TimedOut(assignment)` with queue pop and map cleanup if `now - since >= GRAPH_DEFER_MAX_WAIT`; `Ready(assignment)` with queue pop but keeping `since`/`retry_after` when ready | Returns `Empty` when queue is empty or agent not found; strictly respects `now < retry_after` and saturating duration for `MAX_WAIT` | TC-2041 |
| FR-2042 | Must | Given queued assignments for an agent occupied by an active turn and `now: Instant` | `mark_waiting_behind_busy(agent, now)` resets `since` to `now` for all queued nodes of that agent | Only queued nodes belonging to the specified agent are updated; other agents' defer origins remain untouched | TC-2042 |
| FR-2043 | Must | Given queued assignments and node lifecycle events | `remove_node(node_id)` removes the assignment with `node_id` from all agent queues; `node_started(node_id)` cleans up `since` and `retry_after` entries | Idempotent on nonexistent `node_id`; `remove_node` preserves timing maps while `node_started` cleans timing maps | TC-2043 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2040 | FR-2040 | unit | Fresh `GraphDeferQueue` | Defer `step-1` at `t0`, then re-defer at `t1 = t0 + 10s` | Initial `since` equals `t0`; re-defer preserves `since == t0`, updates `retry_after == t1 + RETRY_INTERVAL`, and queue contains exactly 1 entry | `src/graph_defer.rs` `tc_2040_defer_sets_since_once_and_retry_after` |
| TC-2041 | FR-2041 | unit | `GraphDeferQueue` with queued assignment | Call `poll` before `retry_after`, between `retry_after` and `MAX_WAIT`, and after `MAX_WAIT` | Returns `NotYet` initially, `Ready` when interval passes, `Empty` when drained, and `TimedOut` when `now - since >= MAX_WAIT` with timing map cleanup | `src/graph_defer.rs` `tc_2041_poll_not_yet_ready_timed_out` |
| TC-2042 | FR-2042 | unit | `GraphDeferQueue` with assignments for multiple agents | Call `mark_waiting_behind_busy` for one agent at `t1` | `since` timestamps for that agent's nodes are updated to `t1`; other agents' node timestamps remain unchanged at `t0` | `src/graph_defer.rs` `tc_2042_mark_waiting_behind_busy_resets_since` |
| TC-2043 | FR-2043 | unit | `GraphDeferQueue` with queued nodes | Call `remove_node` for a node across queues; call `node_started` | `remove_node` purges the node ID from all queues while keeping timing maps; `node_started` deletes `since` and `retry_after` entries | `src/graph_defer.rs` `tc_2043_remove_node_and_node_started` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| State encapsulation | `GraphDeferQueue` replaces loose `graph_deferred_starts`, `graph_defer_retry_after`, and `graph_defer_since` | Backward compatible; no changes to queue ordering or timing | TC-2040, TC-2041 |
| Polling semantics | `DeferPoll` distinguishes `Empty`, `NotYet`, `TimedOut`, and `Ready` | Backward compatible; preserves exact error message and dispatch on timeout and ready | TC-2041 |
| Busy runner reset | Runner busy check pauses timeout counting via `mark_waiting_behind_busy` | Backward compatible; preserves FR-976 timeout pause behavior | TC-2042 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Create `src/graph_defer.rs` module with `GraphDeferQueue`, `DeferPoll`, constants, and unit tests | `src/graph_defer.rs`, `src/main.rs` | FR-2040, FR-2041, FR-2042, FR-2043 | `tc_2040`, `tc_2041`, `tc_2042`, `tc_2043` |
| 2 | Refactor `App` graph deferral fields and methods in `src/app.rs` | `src/app.rs` | FR-2040, FR-2041, FR-2042, FR-2043 | existing app graph tests |
| 3 | Create specification document | `docs/specs/SPEC-graph-defer-queue.md` | FR-2040, FR-2041, FR-2042, FR-2043 | `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-2040..FR-2043) map to passing test evidence (TC-2040..TC-2043).
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [x] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
