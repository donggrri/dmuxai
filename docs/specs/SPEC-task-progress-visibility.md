# SPEC: Task progress visibility

Status: complete<br>
Owner: DmuxAI<br>
Created: 2026-08-24<br>
Baseline: `main` worktree baseline<br>
Related: baseline `FR-012`, `FR-041`, `BR-001`, `NFR-001`, `NFR-002`

## 1. Decision summary

Users need to distinguish an agent that is still working from one that is
idle or has stopped producing visible text. DmuxAI will expose a transient,
provider-neutral activity summary in the status bar for every active agent:
queue/running state, current observable phase, elapsed time, and time since
the last correlated provider event. The display will use existing lifecycle,
text, notice, and tool events; it will not claim that a provider is waiting or
making progress when DmuxAI has no such evidence.

## 2. Scope

### In scope

- Show `QUEUED` immediately after an agent request is accepted by the adapter.
- Show `RUNNING` after the correlated `Started` event is received.
- Derive a concise phase from existing events: starting, working, responding,
  or tool activity.
- Show elapsed time since the accepted request and age of the last correlated
  activity event for the selected agent.
- Show compact active-state indicators for other agents that are working in
  the same DmuxAI runtime.
- Cover conversation-summary turns with the same activity treatment.
- Remove the transient activity indicator on completion, cancellation, or
  failure while preserving the existing durable transcript and notice.

### Out of scope / non-goals

- Adding provider heartbeats, changing Codex/Cursor/AGY request or event
  protocols, or proving that a provider process is making internal progress.
- Persisting activity timestamps, phase labels, or a full event history in
  SQLite.
- Automatically timing out, retrying, or cancelling a turn because no event
  arrived for a period of time.
- Replacing existing chat tool-progress messages or adding a new command.
- Changing cancellation semantics, agent isolation, sandbox policy, or process
  launch behavior.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | `last event` is an honest and useful signal even when a provider is internally thinking without emitting an event. | DmuxAI | Execute |

OQ-001 is resolved by displaying the age as an observation (`last event`) and
never presenting it as a provider heartbeat or guaranteed progress signal.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Activity record | In-memory UI record for one active `(agent, turn)` request. | A durable turn row or provider session. |
| Queue state | The adapter accepted the request, but DmuxAI has not received `Started`. | Provider execution has begun. |
| Running phase | The most recent observable phase inferred from a correlated event. | A provider-reported internal plan or percentage. |
| Last event age | Monotonic elapsed time since the most recent correlated lifecycle, text, notice, or tool event. | A heartbeat, ETA, or timeout. |

Activity records transition as follows:

| Current state | Event | Result |
| --- | --- | --- |
| absent | request accepted | `QUEUED`, phase `starting` |
| queued | `Started` | `RUNNING`, phase `starting` |
| running | provider session/notice event | `RUNNING`, phase `working` |
| running | text delta | `RUNNING`, phase `responding` |
| running | tool progress | `RUNNING`, phase `tool` with bounded summary |
| queued/running | completed, cancelled, or failed | activity record removed |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-060 | Must | Given an idle agent and a request accepted by its adapter | The status bar immediately shows that agent as `QUEUED` and retains the existing cancel affordance | If adapter submission fails, no activity record is shown and the existing prompt/error recovery remains unchanged | TC-060 |
| FR-061 | Must | Given an active request and a correlated provider event | The selected agent's status shows `RUNNING`, a phase derived from the latest observable event, elapsed time, and `last event` age | Misowned, stale, or uncorrelated output does not update the activity record | TC-061, TC-062 |
| UX-030 | Must | Given the selected agent has an active request | The one-line status area communicates at least state, phase, elapsed time, and time since the last correlated event without claiming an ETA or provider heartbeat | Long tool summaries are bounded and the status line may clip at the terminal edge without affecting chat or input handling | TC-061, TC-062 |
| UX-031 | Must | Given one or more non-selected agents have active requests | The status bar includes compact indicators for those agents so switching the selected agent is not required to discover active work | Idle agents are omitted from the compact active list; each agent remains independently cancellable through existing selection and `Ctrl+C` behavior | TC-063 |
| FR-062 | Must | Given a correlated completion, cancellation, or failure event | The finished agent disappears from the active activity display, and the existing transcript/terminal notice remains visible | A late event that fails existing ownership checks cannot remove or alter another agent's activity | TC-063 |
| NFR-020 | Must | During any activity display | Activity state is transient in memory; no SQLite migration or provider protocol change is introduced, and existing turn persistence/correlation remains authoritative | A renderer failure cannot change turn state or provider permissions | TC-064 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-060 | FR-060 | app unit | A test app has an idle Codex adapter and an input prompt | Submit the prompt while the adapter accepts it | The prompt is sent, the app shows `Codex QUEUED`, and a failed adapter submission shows no queued state and restores input | `cargo test queued_activity_is_visible_after_request_acceptance` |
| TC-061 | FR-061, UX-030 | app rendering/unit | A queued request has a correlated turn ID | Apply `Started`, a text delta, and a tool-progress event | The status changes to `RUNNING`, then exposes `responding`, then bounded tool detail; the activity record's last-event timestamp advances | `cargo test running_activity_reflects_observable_phase` |
| TC-062 | FR-061, UX-030 | app rendering/unit | An active record has a known start and last-event instant | Render using a later deterministic instant | The status reports elapsed time and `last event` age; it does not say `stalled`, `waiting`, or provide a fabricated ETA | `cargo test activity_status_reports_observed_event_age` |
| TC-063 | UX-031, FR-062 | app unit/rendering | Codex and Cursor each have an active request | Render, then complete Codex | Both active agents are discoverable before completion; only Codex disappears afterward and Cursor remains active | `cargo test active_agents_are_visible_and_terminal_activity_is_cleared` |
| TC-064 | NFR-020 | regression/static | Existing conversation persistence and provider adapter tests are available | Run the feature and repository gates | No schema/protocol files change for the feature; existing correlation, cancellation, and provider-safety tests pass | `cargo test --all-targets`; `cargo clippy --all-targets --all-features -- -D warnings` |

The scenarios cover request acceptance, observable phase changes, elapsed/last
event boundary behavior, multi-agent visibility, terminal cleanup, and the
no-persistence/no-protocol-change contract.

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema or canonical transcript change; activity timestamps and phase are in-memory only. | Reverting the feature removes only the status display state; existing SQLite rows remain readable. | TC-064 |
| Provider protocol | No provider request or normalized event shape change; existing correlated events are reused. | Provider adapters and live smoke-test policy remain unchanged. | TC-061, TC-064 |
| Security / permissions | No prompt routing, sandbox, approval, or process-launch change. | Existing mode and provider safety invariants remain authoritative. | TC-064 |
| Platform | Uses the existing Ratatui status row and monotonic `Instant`; no platform-specific behavior change. | Existing terminal rendering and input behavior remain the rollback boundary. | TC-062, TC-064 |

## 7. Implementation plan

See [PLAN-task-progress-visibility.md](../plans/PLAN-task-progress-visibility.md)
and [TASKS-task-progress-visibility.md](../tasks/TASKS-task-progress-visibility.md).

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add in-memory activity model and lifecycle update helpers | `src/app.rs` | FR-060, FR-061, FR-062, NFR-020 | TC-060, TC-061, TC-063 |
| 2 | Render selected detail and non-selected active indicators | `src/app.rs` | UX-030, UX-031 | TC-061, TC-062, TC-063 |
| 3 | Add deterministic state/rendering tests and run repository gates | `src/app.rs`, docs | all | TC-060..TC-064 |

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] The diff has been reviewed against every non-goal and existing dirty
  worktree change is preserved.
- [x] Required Rust checks pass: format, all-target tests, and clippy.
- [x] The specification traceability check passes.
- [x] `../reviews/REVIEW-task-progress-visibility.md` records exact commands,
  results, residual risk, and rollback notes.
