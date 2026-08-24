# TASKS: Task progress visibility

Status: complete<br>
Spec: `../specs/SPEC-task-progress-visibility.md`<br>
Plan: `../plans/PLAN-task-progress-visibility.md`<br>

- [x] T1 - Add the transient per-agent activity model, queue initialization,
  phase/last-event update helpers, and terminal cleanup. Requirements:
  FR-060, FR-061, FR-062, NFR-020. Verify: `cargo test
  queued_activity_is_visible_after_request_acceptance` and `cargo test
  running_activity_reflects_observable_phase`.
- [x] T2 - Render selected-agent state, phase, elapsed time, last-event age,
  and compact indicators for other active agents. Requirements: UX-030,
  UX-031. Verify: `cargo test activity_status_reports_observed_event_age` and
  `cargo test active_agents_are_visible_and_terminal_activity_is_cleared`.
- [x] T3 - Cover failed submission, summary activity cleanup, and existing
  cancellation/correlation behavior without changing provider or persistence
  contracts. Requirements: FR-060, FR-062, NFR-020. Verify: targeted app
  tests and `cargo test --all-targets`.
- [x] T4 - Run the spec traceability gate, formatting, all-target tests,
  clippy, and diff check; record exact evidence in the review. Requirements:
  all feature requirements. Verify: the commands listed in the plan.
