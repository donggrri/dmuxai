# SPEC: Codex model effort and mode-context transition

Status: approved
Owner: DmuxAI
Created: 2026-08-24
Baseline: `52dde59`
Related: baseline UX-002, UX-003, FR-040, FR-021, INT-001, INV-005

## 1. Decision summary

Model selection must let a user choose the selected Codex model's reasoning
effort without a separate slash-command round trip. In the model picker, Tab
will select the highlighted model and open its effort picker. Changing an idle
agent's mode must retire that agent's native binding so the next prompt starts
a fresh provider thread while DmuxAI injects the completed conversation context.
This makes Implement effective for the next request without discarding context
or weakening Ask/Plan safety.

## 2. Scope

### In scope

- Tab transition from the Codex model picker to the highlighted model's effort picker.
- Persisting the selected model/default effort before the user chooses an effort.
- Invalidating an idle selected agent's provider binding when its mode changes.
- Fresh next-turn context projection after a mode change for Codex, Cursor, and AGY.

### Out of scope / non-goals

- Changing a mode while a turn is active.
- Changing model or effort support for Cursor and AGY.
- Persisting effort or mode as conversation data, or changing the canonical transcript.
- Giving Ask or Plan write capability, or changing `approvalPolicy: "never"`.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | Tab on a non-Codex model picker keeps its existing no-op behavior because no effort exists to select. | DmuxAI | Execute |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Model-to-effort transition | A Tab action that selects the highlighted Codex model then presents only that model's advertised efforts. | `/effort` opened outside model selection |
| Retired binding | A previously active provider session marked stale so it cannot be resumed. | A cancelled or failed turn |

An idle mode change transitions the selected agent binding from `active` to
`stale`; the next successful turn creates and binds a fresh provider session.
The durable conversation and completed context remain unchanged.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| UX-040 | Must | Codex model picker has a highlighted model; user presses Tab. | The highlighted model is selected and its advertised effort picker opens, with its default effort highlighted. | If the model has no advertised efforts, show the existing unavailable picker state; non-Codex Tab remains a no-op. | TC-047 |
| FR-043 | Must | User changes an idle agent from one mode to another. | The active provider binding for that conversation/agent is retired and the next prompt uses a fresh provider session with the existing shared-context projection. | If there is no binding, mode still changes. If retirement fails, retain the previous mode and show an error. | TC-048 |
| BR-042 | Must | The selected agent has an active turn. | Mode change remains rejected and does not retire a binding or alter the active turn. | Existing busy-agent message remains visible. | TC-049 |
| NFR-042 | Must | Mode change or model-to-effort transition occurs. | Only the selected agent's configuration/binding changes; approval and sandbox safety settings stay unchanged. | No cross-agent setting or context leak. | TC-047, TC-048, TC-049 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-047 | UX-040, NFR-042 | UI state | Codex catalog with two models and distinct effort lists. | Highlight the second model and press Tab. | The model is selected, its default effort is active, and its model-scoped effort picker opens. | `cargo test model_picker_tab_selects_codex_model_and_opens_its_effort_picker` |
| TC-048 | FR-043, NFR-042 | UI/conversation integration | Completed Codex turn with an active provider binding. | Switch to Implement, then submit a follow-up. | Follow-up has no resume id, contains the completed context, and mode is Implement. | `cargo test mode_change_reopens_the_agent_with_shared_context` |
| TC-049 | BR-042, NFR-042 | UI state | Selected agent has an active turn. | Open `/mode`. | Picker does not open; binding and mode stay unchanged. | `cargo test busy_agent_cannot_open_mode_picker` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Update only the selected `(conversation, agent)` binding status to `stale`; no schema migration. | Removing the behavior merely permits native-session resume again. | TC-048 |
| Provider protocol | A fresh next turn uses no resume id and receives DmuxAI context projection; model effort is still sent on `turn/start`. | Existing model/effort requests stay compatible. | TC-047, TC-048 |
| Security / permissions | Implement opens a fresh mode-scoped turn with `workspaceWrite`; Ask/Plan remain `readOnly`, and approvals remain `never`. | No capability expansion outside Implement. | TC-048, TC-049 |

## 7. Implementation plan

See [PLAN-codex-model-effort-mode-context.md](PLAN-codex-model-effort-mode-context.md)
and [TASKS-codex-model-effort-mode-context.md](TASKS-codex-model-effort-mode-context.md).

## 8. Review and test exit criteria

- [ ] TC-047 and TC-048 fail against the current behavior, then pass after the fix.
- [ ] Spec validation and all Rust format, test, and clippy gates pass.
- [ ] `REVIEW-codex-model-effort-mode-context.md` records evidence and any unrun live-provider smoke test.
