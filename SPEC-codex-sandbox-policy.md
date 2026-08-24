# SPEC: Codex sandbox-policy object compatibility

Status: approved
Owner: DmuxAI
Created: 2026-08-24
Baseline: `91cd730`
Related: baseline INT-001, INV-005

## 1. Decision summary

DmuxAI currently sends a string for Codex App Server `turn/start.sandboxPolicy`.
Current App Server versions require an internally tagged sandbox-policy object,
which makes summary and ordinary turns fail before the agent starts. DmuxAI will
send the documented object form while retaining its existing read-only Ask/Plan
and workspace-write Implement permissions.

## 2. Scope

### In scope

- Correct the Codex `turn/start` sandbox-policy JSON shape.
- Add deterministic request-construction regression coverage for every mode.

### Out of scope / non-goals

- Changing approval policy, user-visible modes, or Cursor and AGY protocols.
- Relaxing read-only or workspace-write permissions.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | None; the provider error and current official App Server schema identify the required object shape. | DmuxAI | N/A |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Sandbox-policy object | `turn/start.sandboxPolicy` value with a `type` discriminator. | The legacy string value or thread-level `sandbox` |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| INT-040 | Must | DmuxAI starts a Codex Ask, Plan, or Implement turn. | `sandboxPolicy` is an object whose `type` is `readOnly` for Ask/Plan and `workspaceWrite` for Implement. | The legacy string form is never sent. | TC-046 |
| NFR-041 | Must | A sandbox-policy request is constructed. | Existing `approvalPolicy: never`, model, effort, and mode-specific prompt behavior remain intact. | No mode gains broader permissions. | TC-046 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-046 | INT-040, NFR-041 | unit | One request per Codex mode. | Build `turn/start` JSON. | The discriminator object is present and legacy string shape is absent; approval/model/effort values are preserved. | `cargo test codex_execution_modes_set_per_turn_sandbox_and_approval_policy` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change. | Code-only revert. | TC-046 |
| Provider protocol | Send `sandboxPolicy: { "type": "readOnly" | "workspaceWrite" }`. | Matches current App Server tagged-enum schema. | TC-046 |
| Security / permissions | Preserve `approvalPolicy: "never"`; Ask/Plan remain read-only. | No capability expansion. | TC-046 |

## 7. Implementation plan

See [PLAN-codex-sandbox-policy.md](PLAN-codex-sandbox-policy.md) and
[TASKS-codex-sandbox-policy.md](TASKS-codex-sandbox-policy.md).

## 8. Review and test exit criteria

- [ ] TC-046 passes after failing against the legacy string shape.
- [ ] Rust format, all-target tests, and clippy pass.
- [ ] `REVIEW-codex-sandbox-policy.md` records evidence and no live-provider test is claimed.
