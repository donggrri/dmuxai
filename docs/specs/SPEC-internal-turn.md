# SPEC-internal-turn

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `18f2913`
Related: refactor scan finding #1, PR-8a

## 1. Decision summary

The internal query turn handling in `src/app.rs` is refactored into a dedicated internal turn module (`src/internal_turn.rs`) as the first stage of PR-8 ("extract a session engine from app.rs").
DmuxAI uses internal (hidden, buffered) turns for summary generation, triage judge queries, graph planning/repair, and replanning. Previously, each pending turn struct tracked text and character counts separately (`buffer: String`, `buffer_chars: usize`), duplicated identical output routing logic (`output_belongs_to_*`), duplicated character cap checks and append operations, and had separate turn retirement methods.
PR-8a introduces:
1. `TurnBuffer` encapsulating character-capped text buffering with overflow detection (`would_overflow`) and count tracking (`chars`, `push`).
2. An `InternalTurn` trait and generic `output_belongs` helper standardizing agent output routing across all internal turn kinds.
3. A unified `App::retire_internal_turn` method consolidating turn retirement and adapter cancellation.

## 2. Scope

### In scope

- `TurnBuffer` with `MAX_INTERNAL_TURN_BUFFER_CHARS` (80,000 characters), `would_overflow`, `push`, `as_str`, `chars`, and `set_chars_for_test`.
- `InternalTurn` trait defining `agent`, `conversation_id`, and `turn_id`.
- Generic `output_belongs<T: InternalTurn>(pending: Option<&T>, output: &AgentOutput) -> bool` replacing duplicated routing checks.
- Adoption of `TurnBuffer` in `PendingSummary`, `PendingTriage`, `PendingPlan`, and `PendingReplan`.
- Implementation of `InternalTurn` for the four pending turn structs.
- Unified `App::retire_internal_turn` replacing `retire_internal_judge_turn`, `retire_internal_plan_turn`, `retire_plan_turn_id`, `retire_internal_replan_turn`, and `retire_replan_turn_id`.

### Out of scope / non-goals

- Moving internal turn timeouts (`check_triage_timeout`, `check_plan_timeout`, `check_replan_timeout`) behind the module interface (follow-up in PR-8).
- Moving busy-defer state and logic (`DeferredInternalTurn`, `defer_internal_turn`) behind the module interface (follow-up in PR-8).
- Moving prompt generation/sending and output parsing (`parse_summary_document`, `parse_verdict`, `parse_plan`, `parse_replan`) into `internal_turn.rs` (follow-up in PR-8).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2000 | TurnBuffer character counts count Unicode scalar values (`chars().count()`) to match legacy buffer cap behavior. | dmuxai | PR-8a implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Internal turn | Hidden, buffered agent query turn (summary, triage, plan, replan) whose raw output is not directly streamed to the chat transcript. | User-facing interactive agent turn. |
| TurnBuffer | Buffer structure maintaining accumulated text and Unicode character count bounded by a fixed limit. | Standard unbounded `String`. |
| Retired internal turn | A turn identifier recorded in `retired_internal_turns` to ignore late-arriving responses. | Active or deferred pending turn. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2000 | Must | Given a `TurnBuffer` with current char count and incoming delta | `would_overflow(delta_chars)` returns `true` when `chars.saturating_add(delta_chars) > MAX_INTERNAL_TURN_BUFFER_CHARS` (80,000) and `false` at or below limit; `push(delta)` appends `delta` and increments `chars` by `delta.chars().count()` | Saturating addition prevents integer overflow | TC-2000 |
| FR-2001 | Must | Given an optional reference to an `InternalTurn` and an incoming `AgentOutput` | `output_belongs(pending, output)` returns `true` iff `pending` is `Some`, `output.agent == pending.agent()`, `output.conversation_id == pending.conversation_id()`, and `output.turn_id == pending.turn_id()` | Returns `false` when `pending` is `None`, when agent differs, or when conversation/turn ID is `None` or mismatched | TC-2001 |
| FR-2002 | Must | Given an `App`, agent, turn ID, and `cancel_adapter` flag | `retire_internal_turn(agent, turn_id, cancel_adapter)` inserts `turn_id` into `retired_internal_turns` and calls the agent adapter's `cancel()` method if and only if `cancel_adapter` is `true` | Idempotent on repeated calls; nonexistent turn ID still recorded | TC-2002 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2000 | FR-2000 | unit | Fresh `TurnBuffer` | Check `would_overflow` at MAX and MAX+1; push multibyte unicode strings | `would_overflow(MAX)` is false, `would_overflow(MAX + 1)` is true; multibyte chars increment char count by char count not byte length | `src/internal_turn.rs` `tc_2000_turn_buffer_cap` |
| TC-2001 | FR-2001 | unit | Dummy `InternalTurn` instance and various `AgentOutput` combinations | Call `output_belongs` with matching and non-matching agents, IDs, and `None` pending | Returns `true` only when agent, conversation ID, and turn ID all match; returns `false` on `None` pending, missing IDs, or mismatches | `src/internal_turn.rs` `tc_2001_output_belongs_matches_agent_conversation_turn` |
| TC-2002 | FR-2002 | unit | Test `App` with `RecordingAdapter` | Call `retire_internal_turn` with `cancel_adapter = true` and `cancel_adapter = false` | `retired_internal_turns` contains both turn IDs; adapter cancel is invoked only when `cancel_adapter` is true | `src/app.rs` `tc_2002_retire_internal_turn_records_and_optionally_cancels` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Buffer capacity | `MAX_INTERNAL_TURN_BUFFER_CHARS` remains 80,000; `MAX_SUMMARY_BUFFER_CHARS` aliases it | Backward compatible; no changes to user-visible truncation messages | TC-2000 |
| Turn routing | `InternalTurn` abstracts agent, conversation ID, and turn ID | Backward compatible; identical routing logic for summary, triage, plan, and replan | TC-2001 |
| Retirement | Unified `retire_internal_turn` handles retirement and cancellation flags | Backward compatible; preserves cancel-on-error and ignore-on-complete semantics | TC-2002 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Create `src/internal_turn.rs` module with `TurnBuffer`, `InternalTurn`, `output_belongs`, and unit tests | `src/internal_turn.rs`, `src/main.rs` | FR-2000, FR-2001 | `tc_2000`, `tc_2001` |
| 2 | Refactor `Pending*` structs and `App` retirement in `src/app.rs` | `src/app.rs` | FR-2000, FR-2001, FR-2002 | `tc_2002`, existing tests |
| 3 | Create specification document | `docs/specs/SPEC-internal-turn.md` | FR-2000, FR-2001, FR-2002 | `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-2000..FR-2002) map to passing test evidence (TC-2000..TC-2002).
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [x] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
