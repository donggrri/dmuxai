# REVIEW: Task progress visibility

Status: complete<br>
Spec: `../specs/SPEC-task-progress-visibility.md`<br>
Plan: `../plans/PLAN-task-progress-visibility.md`<br>
Reviewed: 2026-08-24

## Summary

The feature adds an in-memory activity record for each active agent and
surfaces it in the existing one-line status area. Accepted requests begin as
`QUEUED`; correlated provider events move them to `RUNNING` phases such as
`starting`, `responding`, `tool`, or `summarizing`. The status includes elapsed
time and the age of the last observed event, while other active agents receive
compact indicators. Terminal events remove only the matching agent's activity.

## Requirement review

| Requirement | Evidence | Result |
| --- | --- | --- |
| FR-060 | `submit_to_agent` inserts a queued activity only after adapter acceptance; failed sends do not insert it. `queued_activity_is_visible_after_request_acceptance` covers both paths. | Met |
| FR-061 | `observe_activity` runs only after correlated output is durably accepted; `AgentActivity::observe` maps existing lifecycle/text/notice/tool events. `running_activity_reflects_observable_phase` and `activity_status_reports_observed_event_age` pass. | Met |
| UX-030 | `draw_status` renders state, phase, elapsed duration, and `last event` age; detail is bounded and no ETA/heartbeat claim is made. | Met |
| UX-031 | `other_activity_summary` lists active non-selected agents. `active_agents_are_visible_and_terminal_activity_is_cleared` verifies discovery with Codex and Cursor. | Met |
| FR-062 | `finish_turn` and all summary terminal/overflow paths clear only the matching activity. The multi-agent terminal cleanup test passes. | Met |
| NFR-020 | Activity data is transient in `src/app.rs`; no SQLite migration or provider event/request change was added. Existing persistence and adapter tests pass. | Met |

## Scope and regression review

- Existing `busy_agents`, turn correlation, cancellation, transcript storage,
  tool chat messages, and provider safety mappings remain authoritative.
- Activity updates occur after `record_turn_outputs` succeeds, so a failed
  persistence write does not present an uncommitted provider event as progress.
- Misowned and delayed output continues to follow the existing ownership
  checks. Terminal cleanup is per-agent, so another agent's active work remains
  visible.
- Summary jobs use the same activity map with a `summarizing` phase and clear
  it on successful, failed, cancelled, and oversized-output paths.
- The worktree already contained unrelated in-progress changes, including
  multi-instance, role-separated rendering, and provider mode validation. They
  were preserved; this review covers only the task-progress additions that
  overlap `src/app.rs` and its tests.

## Test evidence

All commands were run from `C:\path\to\DmuxAI` with the bundled
Rust toolchain path `%USERPROFILE%\.cargo\bin\cargo.exe` where needed.

- `cargo test --bin dmuxai` — passed: 93 passed, 5 ignored provider smoke tests.
- `cargo test --all-targets` — passed: 8 library tests, 93 binary tests, and
  31 conversation integration tests passed; 5 authenticated-provider tests
  remained ignored.
- `cargo fmt --check` — passed.
- `cargo clippy --all-targets --all-features -- -D warnings` — passed.
- `scripts/check-spec.ps1` — passed for the feature spec, baseline spec, and
  the other changed feature specs in the existing worktree.
- `git diff --check` — passed.

Live Codex, Cursor, and AGY smoke tests were not run because they require
installed and authenticated provider CLIs; they remain explicitly ignored as
required by the repository workflow.

## Residual risks

- `last event` is an observation of DmuxAI's event stream, not a provider
  heartbeat. A provider can be internally thinking without emitting an event;
  the UI intentionally does not label that state as stalled or waiting.
- The existing one-line status area clips on narrow terminals. Clipping does
  not affect the transcript, prompt input, or activity state.

## Rollback

Revert the activity model, lifecycle hooks, status rendering, tests, and the
three task artifacts. No database repair, migration rollback, or provider
protocol rollback is required.
