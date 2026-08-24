# PLAN: Task progress visibility

Status: complete<br>
Spec: `../specs/SPEC-task-progress-visibility.md`<br>
Baseline: current `main` worktree baseline

## Goal

Expose honest, transient activity information for every active agent without
changing provider protocols, durable conversation state, or cancellation.

## Ordered changes

1. Add an in-memory activity record keyed by agent. Store queue/running phase,
   start instant, last correlated-event instant, and a bounded display detail.
   (FR-060, FR-061, FR-062, NFR-020)
2. Create/update records when a request is accepted and when existing lifecycle,
   text, notice, or tool events are applied. Cover summary requests using the
   same record and remove records on terminal outcomes. (FR-060, FR-061,
   FR-062)
3. Replace the selected-agent `BUSY/IDLE` detail with state, phase, elapsed
   time, and last-event age; append compact active indicators for other agents.
   Keep the existing model/mode/cancel affordances where the terminal width
   permits. (UX-030, UX-031)
4. Add deterministic app tests for queued visibility, phase transitions,
   event-age formatting, multi-agent indicators, terminal cleanup, and failed
   submission behavior. (TC-060..TC-064)
5. Run the feature spec check, narrow tests, format, all-target tests, clippy,
   and diff checks; record exact results in the review artifact.

## Risks and mitigations

- A silent provider may be mistaken for progress. Label the value `last event`
  and show its age; do not call it a heartbeat, ETA, or stalled state.
- Existing app tests may construct `App` directly. Keep the new field initialized
  in the shared runtime constructor and avoid changing test adapter contracts.
- Status content can exceed a narrow terminal. Keep tool details bounded and
  use the existing one-line clipping behavior; never alter chat/input layout.
- Summary turns do not have a normal `active_turns` entry. Update their activity
  through the existing `pending_summary` routing and clear it on every terminal
  path.

## Rollback

Revert the activity field, lifecycle helpers, status-row rendering, tests, and
feature documents. No database migration, provider protocol rollback, or data
repair is required.

## Verification order

1. `cargo test queued_activity_is_visible_after_request_acceptance`
2. `cargo test running_activity_reflects_observable_phase`
3. `cargo test activity_status_reports_observed_event_age`
4. `cargo test active_agents_are_visible_and_terminal_activity_is_cleared`
5. `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-task-progress-visibility.md`
6. `cargo fmt --check`
7. `cargo test --all-targets`
8. `cargo clippy --all-targets --all-features -- -D warnings`
9. `git diff --check`
