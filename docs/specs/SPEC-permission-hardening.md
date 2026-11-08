# SPEC: Permission hardening and Implement outcome

Status: Implemented (PR #2); some TCs proposed  
Owner: DmuxAI  
Created: 2026-10-17  
Baseline: `73e3f4d`  
Related: MISSION §4, SPEC-cursor-windows-implement-guard, SPEC-codex-sandbox-policy

## 1. Decision summary

Implement turns must never silently succeed when writes are blocked: a permission denial in stderr or an Implement turn with no observable writes fails the turn and records the reason. Ask/Plan stay read-only and are never failed for missing writes. The Windows-only unsandboxed Cursor opt-in guard (`[cursor] windows_unsandboxed = true` or env) is deferred with Windows support (see [SPEC-cursor-windows-implement-guard.md](SPEC-cursor-windows-implement-guard.md)).

## 2. Scope

### In scope

- `src/agent/implement_outcome.rs` outcome classification
- Failure reason persistence on the turn

### Out of scope / non-goals

- Automatic retry with elevated permissions

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| Q-1 | Proposed TCs (marked *proposed*) have no code yet and must be implemented before this spec is Approved. | DmuxAI | Release gate |

### Code-vs-spec mismatches

| ID | Spec says | Code does (file:function) | Proposed resolution |
| --- | --- | --- | --- |
| OQ-1849 | FR-301 states that an Implement turn ending with no observable writes fails | `src/agent/cursor.rs:1083` and `src/agent/agy.rs:680` pass `require_writes = false` in `implement_tracker.finish()`; ordinary implement turns succeed without writes | Clarify via FR-1861 that `require_writes` is true only for graph implement steps with `require_file_edits = true` |

## 3. Terms and state model

- Observable write: a provider stream line that indicates a file edit/write (`provider_line_suggests_write`).
- Outcome: Completed or Failed(reason).

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-300 | Must | Implement turn ends; stderr contains a permission-denial pattern. | Turn is Failed and the stderr reason is stored on the turn. | Unrelated stderr does not fail the turn. | TC-300 |
| FR-301 | Must | Implement turn ends with no observable writes (see OQ-1849). | Turn is Failed with a 'no observable writes' reason when require_writes is active. | A turn with any write is Completed. | TC-301 |
| FR-302 | Must | Ask or Plan turn ends without writes. | Turn is Completed; Plan is never blocked by the write check. | — | TC-302, TC-303 |
| FR-303 | Retired (Windows dropped, #115) | Windows Cursor Implement without opt-in. | **Deferred (Windows dropped 2026-10-22).** Rejected before mode queueing and before process spawn. | With `windows_unsandboxed = true` the guard allows it. | TC-304, TC-305 |
| FR-1860 | Must | Permission denial diagnostics are detected across runners (`src/agent/implement_outcome.rs`, `src/agent/agy.rs`, `src/agent/acp/client.rs`). | In stderr, patterns like `permission denied`, `approval required`, `read-only`, `operation not permitted` trigger turn failure. In AGY, `result.denied_actions` surfaces `"AGY denied permission for: <actions>..."`. In ACP, unauthorized tool requests emit `PERMISSION_DENIED_NOTICE` and end turn as `Cancelled`. | Clean output without denial indicators completes normally. | TC-1860 |
| FR-1861 | Must | Scope of observable write enforcement (`src/agent/implement_outcome.rs` `ImplementTurnTracker::finish`, `src/agent/cursor.rs`, `src/agent/agy.rs`). | Missing observable writes fail the turn only when `require_writes` is `true` (e.g. graph implement steps requiring file edits). Interactive user implement turns pass `require_writes = false` to allow non-editing answers. | Conversational implement turns without file edits do not fail. | TC-1861 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-300 | FR-300 | unit | stderr with permission denied | classify | denial detected | `src/agent/implement_outcome.rs` `tc_300_permission_denial_is_detected_in_stderr` |
| TC-301 | FR-301 | unit | Implement, no write lines | classify | Failed | `src/agent/implement_outcome.rs` `tc_301_implement_without_writes_fails` |
| TC-302 | FR-302 | unit | Ask, no writes | classify | Completed | `src/agent/implement_outcome.rs` `tc_302_read_only_does_not_require_writes` |
| TC-303 | FR-302, FR-300 | integration | Plan turn and failed Implement turn in app | finish turns | Plan Completed; Implement reason persisted in DB | `tests/conversation.rs` `tc_303_turn_completion_and_failure_reason_persisted` |
| TC-1860 | FR-1860 | unit | stderr permission denied and agy denied_actions | classify and stream child | denial error recognized | `src/agent/implement_outcome.rs` `tc_1860_permission_denial_patterns_detected_in_stderr` |
| TC-1861 | FR-1861 | unit | Implement turn tracker with require_writes false vs true | `ImplementTurnTracker::finish` | missing writes ok when require_writes false, fails when true | `src/agent/implement_outcome.rs` `tc_1861_implement_turn_tracker_require_writes_scope` |

## 6. Data, integration, and safety contracts

- Failure reasons are stored as plain text, never secrets beyond provider stderr.

## 7. Implementation plan

See status above; proposed tests are tracked in [TEST-PLAN.md](../TEST-PLAN.md).

## 8. Review and test exit criteria

- TC-300..303 pass in CI; TC-304 and TC-305 are retired (Windows dropped, #115).

### Retired test cases

TC-304 and TC-305 are retired. The Windows Cursor Implement guard and the `windows_unsandboxed` opt-in were removed with execution modes (#80, `924d2b9`) and Windows support (#115, `7c943d1`). Neither `windows_unsandboxed` nor the guard test exists in `src/` or `tests/` any more.
