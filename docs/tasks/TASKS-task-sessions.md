# TASKS: Task sessions

Status: in progress (PR-B UI wiring complete)
Spec: [SPEC-task-sessions.md](../specs/SPEC-task-sessions.md)
Plan: [PLAN-task-sessions.md](../plans/PLAN-task-sessions.md)

- [x] T1 - Store the active task's agent, model and effort whenever the TUI
  picker (`/agent`, model picker, effort picker) or a dashboard session command
  changes them, and show a notice if the write fails.
  Requirements: FR-2322. Verify: `cargo test -j 2 --bin dmuxai tc_2322`.
- [x] T2 - Restore saved settings on conversation select (F3,
  `/conversations`, `/resume`, dashboard `load_conversation`) after runner
  availability and model-policy validation, falling back to the global default
  with one notice for an unavailable agent or model; NULL settings keep the
  previous behavior.
  Requirements: FR-2322. Verify: `cargo test -j 2 --bin dmuxai tc_2322`.
- [x] T3 - Expose `settings: {agent, model, effort}` (or `null`) on dashboard
  conversation entries, including other workspace groups.
  Requirements: FR-2322. Verify: `cargo test -j 2 --lib session_conversation_fields_roundtrip`.
- [x] T4 - Add the CHANGELOG "Unreleased" entry and run the focused gates.
  Requirements: FR-2322. Verify: `cargo fmt --check`,
  `cargo clippy -j 2 --all-targets -- -D warnings`,
  `cargo test -j 2 --test conversation tc_232`.
