# TASKS: Task sessions

Status: in progress (PR-B UI wiring and PR-C resume/delta complete)
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
- [x] T5 - Resume each agent's own active binding after other agents complete
  turns, build the bounded unseen delta with source agent/model labels, and use
  the existing fresh projection when the delta exceeds 200 turns or 65,536
  UTF-8 bytes or the binding is stale, retired, or missing.
  Requirements: FR-2323, FR-2324, FR-2325. Verify:
  `cargo test -j 2 --test conversation tc_232`.
- [x] T6 - Advance `known_through_seq` only on a saved completed turn, and
  retire plus retry once with a fresh projection when a provider rejects a
  resume session id.
  Requirements: FR-2325, FR-2326. Verify:
  `cargo test -j 2 --test conversation tc_232`,
  `cargo test -j 2 --bin dmuxai tc_232`.
- [x] T7 - Keep delta data inside the existing untrusted envelope, scoped to
  the active task, and update the superseded cross-agent-sharing FR-027 text,
  CHANGELOG, and review record.
  Requirements: FR-2332, FR-027. Verify:
  `cargo test -j 2 --test conversation tc_2332`,
  `cargo test -j 2 --bin dmuxai tc_2332`,
  `bash scripts/check-spec.sh docs/specs/SPEC-task-sessions.md`,
  `bash scripts/check-spec.sh docs/specs/SPEC-automatic-cross-agent-sharing.md`.
