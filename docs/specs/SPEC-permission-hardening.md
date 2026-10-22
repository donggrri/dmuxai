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

## 3. Terms and state model

- Observable write: a provider stream line that indicates a file edit/write (`provider_line_suggests_write`).
- Outcome: Completed or Failed(reason).

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-300 | Must | Implement turn ends; stderr contains a permission-denial pattern. | Turn is Failed and the stderr reason is stored on the turn. | Unrelated stderr does not fail the turn. | TC-300 |
| FR-301 | Must | Implement turn ends with no observable writes. | Turn is Failed with a 'no observable writes' reason. | A turn with any write is Completed. | TC-301 |
| FR-302 | Must | Ask or Plan turn ends without writes. | Turn is Completed; Plan is never blocked by the write check. | — | TC-302, TC-303 |
| FR-303 | Must | Windows Cursor Implement without opt-in. | **Deferred (Windows dropped 2026-10-22).** Rejected before mode queueing and before process spawn. | With `windows_unsandboxed = true` the guard allows it. | TC-304, TC-305 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-300 | FR-300 | unit | stderr with permission denied | classify | denial detected | `cargo test permission_denial_is_detected_in_stderr` |
| TC-301 | FR-301 | unit | Implement, no write lines | classify | Failed | `cargo test implement_without_writes_fails` |
| TC-302 | FR-302 | unit | Ask, no writes | classify | Completed | `cargo test ask_mode_does_not_require_writes` |
| TC-303 | FR-302, FR-300 | integration | Plan turn and failed Implement turn in app | finish turns | Plan Completed; Implement reason persisted in DB | *proposed*: `plan_mode_never_fails_for_missing_writes`, `implement_failure_reason_is_persisted` |
| TC-304 | FR-303 | unit | Windows target, no opt-in | select Implement / start turn | rejected before queue and spawn | `cargo test windows_cursor_implement_mode_is_rejected_before_queueing`; `windows_cursor_implement_turn_is_rejected_before_process_spawn` |
| TC-305 | FR-303 | unit | settings with `windows_unsandboxed = true` | load + validate | opt-in honored; default false | *proposed*: `windows_unsandboxed_requires_explicit_opt_in` |

## 6. Data, integration, and safety contracts

- Failure reasons are stored as plain text, never secrets beyond provider stderr.

## 7. Implementation plan

See status above; proposed tests are tracked in [TEST-PLAN.md](../TEST-PLAN.md).

## 8. Review and test exit criteria

- TC-300..302, TC-304 pass in CI; proposed TC-303, TC-305 implemented.
