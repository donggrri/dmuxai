# TASKS: the headless plan-only marker needs an explicit approval request

Status: complete  
Spec: [../specs/SPEC-plan-gate.md](../specs/SPEC-plan-gate.md) (FR-2121)

- [x] T1 - Add `is_plan_approval_request` (plus the `mentions_plan_statement`
  and `asks_for_approval` halves) to `src/agent/implement_outcome.rs` with
  anchored English and Korean regexes: a plan statement plus an explicit
  approval request, never a bare `plan.md`, `review`, `proceed` or `confirm`.
  Requirements: FR-2121. Verify: `cargo test --bin dmuxai -- tc_2145`.
- [x] T2 - Use the new classifier (plus "no observed writes") for the headless
  exit-7 marker in `run_turn` (`src/agent/agy.rs`) and keep the stricter-edit
  retry path for turns that require file edits on the loose detector.
  Requirements: FR-2121. Verify: `cargo test --bin dmuxai -- tc_2123 tc_2124`.
- [x] T3 - Detect a structural plan-artifact signal in the AGY stream
  (`step_update` plan/artifact step or plan/artifact tool step) and use it as
  an additional positive signal, never as a requirement.
  Requirements: FR-2121. Verify: `cargo test --bin dmuxai -- agy_plan_artifact_steps_are_detected_from_step_updates`.
- [x] T4 - Replace the headless plan-only note in `src/app.rs` with the safer
  wording ("re-run with --approve-plan ... the reply is in \"text\"").
  Requirements: FR-2121. Verify: `cargo test --bin dmuxai -- tc_2122`.
- [x] T5 - Separate the two answers of one stricter-edit retry with a blank
  line in the transcript and in the headless output.
  Requirements: FR-2121. Verify: `cargo test --bin dmuxai -- tc_2146`.
- [x] T6 - Update the spec (FR-2121, TC-2123, TC-2145, TC-2146), `docs/API.md`,
  the `dmuxai --help` exit-code text in `src/main.rs`, and the changelog, then
  run the gates.
  Requirements: all. Verify: `python3 scripts/check-spec.py --all`,
  `python3 scripts/check-tc-links.py`, `bash scripts/check-spec.sh docs/specs/SPEC-plan-gate.md`,
  `cargo fmt --all`, `cargo check --all-targets`, and the named tests above.
