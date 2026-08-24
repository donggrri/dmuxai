# SPEC: Codex effort, tool progress, and usage status

Status: approved
Owner: DmuxAI
Created: 2026-08-24
Baseline: `7a3a7219793364d533488d96a905a751845a6e5c`
Related: baseline FR-002, UX-003, UX-010, UX-011, NFR-001, NFR-002, INV-004

## 1. Decision summary

DmuxAI will expose Codex reasoning-effort selection, live Codex tool progress,
and Codex account/context usage through the existing terminal UI.  Effort is a
per-agent, next-turn setting and is constrained to values advertised by the
selected Codex model.  Tool activity is transient, correlated with its owning
turn, and displayed in the chat as it starts and completes.  `/status` loads
the selected Codex runtime's configuration plus available token-activity and
rate-limit data; unavailable provider data is explicitly displayed rather than
invented.

## 2. Scope

### In scope

- `/effort` picker for supported Codex reasoning effort values.
- Forwarding non-text Codex App Server tool items as live, bounded chat
  progress messages.
- `/status` display of Codex model, effort, token activity, and remaining
  rate-limit windows when App Server returns them.
- Unit and UI-state tests for normal, unavailable, busy, and malformed data
  paths.

### Out of scope / non-goals

- Changing Cursor or AGY configuration, protocols, or their native commands.
- Persisting tool inputs, tool outputs, account usage, or credentials in the
  DmuxAI SQLite transcript.
- Enabling Codex approval prompts, full access, or any less restrictive
  sandbox/approval policy.
- Fabricating an exact remaining token count when Codex only reports a
  percentage rate-limit window.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | Current Codex App Server versions return model effort options, tool item lifecycle notifications, and account status methods as documented. Older or partially authenticated CLIs may omit any of them. | DmuxAI | Execute |

OQ-001 does not block implementation: each absent or malformed optional field
has a defined unavailable fallback below.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Effort | A Codex `turn/start` reasoning-budget value selected for later turns. | DmuxAI Ask/Plan/Implement mode |
| Tool progress | A transient rendering of a Codex item lifecycle for an owning turn. | Assistant response text |
| Usage status | Provider-reported token-activity and rate-limit data shown by `/status`. | A guaranteed remaining token total |

Effort is `unselected` until model metadata is loaded, then is one advertised
value. It may change only while Codex is idle and applies to the next turn.
Tool progress is `started` then `completed`, `failed`, or `declined`; late or
misowned events are ignored using the existing active-turn ownership gate.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-040 | Must | Codex is selected and idle; user runs `/effort`. | A picker shows effort values advertised for the selected Codex model; selection is reflected in the status line and sent as `effort` on the next Codex `turn/start`. | If model metadata has not loaded, it is loaded first. If no effort is advertised, show an actionable message and do not send a turn override. | TC-040, TC-041 |
| BR-040 | Must | User selects another agent or Codex has an active turn. | `/effort` never changes a non-Codex agent and rejects a change while Codex is busy. | Existing effort remains unchanged. | TC-041 |
| FR-041 | Must | A scoped Codex App Server `item/started` or `item/completed` notification contains a supported tool item. | Chat shows the tool kind, safe concise action summary, and latest lifecycle state while the owning turn is live. | Unknown/malformed items produce no chat message; full tool output and arguments are not rendered or persisted. | TC-042, TC-043 |
| BR-041 | Must | A tool event is delayed, unscoped, or belongs to another conversation/turn. | It is not displayed in the active conversation. | It cannot alter a terminal or another agent's turn. | TC-043 |
| FR-042 | Must | Codex is selected; user runs `/status`. | Chat shows active model and effort plus provider token activity and each available rate-limit window with used/remaining percentage and reset time. | Missing, malformed, unsupported, or unauthenticated usage fields are shown as unavailable; no provider turn starts or fails. | TC-044, TC-045 |
| NFR-040 | Must | Any optional provider metadata is received. | UI work remains bounded: command/tool summaries are length-limited and account status does not persist sensitive account data. | Provider errors are converted to status text, not a crash or unbounded output. | TC-043, TC-045 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-040 | FR-040 | unit | A Codex request and selected effort. | Build the turn request. | `effort` is sent and safety mode fields are unchanged. | `cargo test codex_turn_request_includes_selected_effort` |
| TC-041 | FR-040, BR-040 | UI state | Idle Codex model catalog with supported efforts; then busy/non-Codex states. | Open/select `/effort`. | Picker selects supported value; busy/non-Codex cases preserve settings and show a message. | `cargo test effort_picker` |
| TC-042 | FR-041 | adapter unit | App Server command, MCP, and file-change item fixtures. | Normalize started/completed notifications. | Concise tool lifecycle events contain type/action/state. | `cargo test normalizes_codex_tool_item_lifecycle` |
| TC-043 | FR-041, BR-041, NFR-040 | UI integration | Active turn plus malformed, oversized, and mismatched tool fixtures. | Apply outputs. | Only owned bounded summaries render; malformed/mismatched items do not render. | `cargo test codex_tool_progress` |
| TC-044 | FR-042 | adapter/UI integration | Successful rate-limit and usage response fixtures. | Run `/status`. | Model/effort, usage, remaining percent, and reset text render without a provider turn. | `cargo test codex_status` |
| TC-045 | FR-042, NFR-040 | adapter/UI unit | Missing/unauthenticated/malformed status fixtures. | Run `/status`. | Unavailable fields are explicit and no fatal agent error occurs. | `cargo test codex_status_reports_unavailable_provider_data` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change; tool and account status are transient UI notices. | Remove the feature without migration or transcript rewrite. | TC-043, TC-044 |
| Provider protocol | Use documented `turn/start.effort`, `item/*`, `account/rateLimits/read`, and `account/usage/read` fields; tolerate omissions. | Older CLIs leave optional information unavailable. | TC-040, TC-042, TC-045 |
| Security / permissions | Keep `approvalPolicy: never` and Ask/Plan read-only; do not log full tool arguments/output or account data. | No sandbox or approval change. | TC-040, TC-043 |
| Platform | Uses existing stdio App Server transport on Windows and Linux. | Existing process-launch behavior is unchanged. | TC-040 |

## 7. Implementation plan

See [PLAN-codex-effort-tool-usage.md](../plans/PLAN-codex-effort-tool-usage.md) and
[TASKS-codex-effort-tool-usage.md](../tasks/TASKS-codex-effort-tool-usage.md).

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] External-provider smoke tests are marked run, skipped, or blocked with a reason.
- [ ] `../reviews/REVIEW-codex-effort-tool-usage.md` records commands, result, residual risk, and rollback notes.
