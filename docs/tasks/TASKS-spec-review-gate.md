# TASKS: spec-review-gate

Status: part 1 complete; part 2 open
Owner: DmuxAI
Created: 2026-11-28
Spec: [SPEC-spec-review-gate](../specs/SPEC-spec-review-gate.md)
Plan: [PLAN-spec-review-gate](../plans/PLAN-spec-review-gate.md)

## Part 1 (core, CLI, MCP flag)

- [x] T1 — Settings and restricted-key handling. Requirements: FR-3700.
  Verify: `cargo test --lib tc_3700`.
- [x] T2 — Spec-dir snapshot helpers. Requirements: FR-3702, FR-3703.
  Verify: `cargo test --lib graph::spec_first`.
- [x] T3 — Executor hold, `awaiting_spec_review`, re-run with feedback.
  Requirements: FR-3702, FR-3704, FR-3705. Verify: `cargo test --lib graph::`.
- [x] T4 — Record store, digest, `decide()`, gate controller, journal events,
  graph file field. Requirements: FR-3703, FR-3704, FR-3705, FR-3706, FR-3707,
  FR-3710, NFR-3700, NFR-3701. Verify: `cargo test --lib graph::spec_review`
  (TC-3702 .. TC-3707, TC-3709 read caps, TC-3710, TC-3718).
- [x] T5 — App wiring for headless runs (arm, poll, stop, cancel, JSON field).
  Requirements: FR-3702, FR-3704, FR-3706, FR-3717. Verify:
  `cargo test --test spec_first_headless` (TC-3717, TC-3705, TC-3704, TC-3706).
- [x] T6 — `--no-spec-review` and MCP `no_spec_review`; `--approve-plan` never
  approves a spec. Requirements: FR-3701. Verify:
  `cargo test --bin dmuxai tc_3701` and `cargo test --lib tc_3701`.
- [x] T7 — `dmuxai spec-review` CLI, human-only rule, exit codes, no MCP decide
  tool. Requirements: FR-3708 (CLI, MCP), FR-3709. Verify:
  `cargo test --test spec_review_cli` and
  `cargo test --bin dmuxai tc_3708`, `cargo test --lib tc_3708`.
- [x] T8 — Gates. Verify: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`.

## Part 2 (not started)

- [ ] T9 — Board item `spec_review` field, notify events, board child timeout
  (FR-3711, FR-3712). `src/board/runner.rs`.
- [ ] T10 — Dashboard endpoints and human headers (FR-3708 dashboard part, FR-3713).
- [ ] T11 — Board UI (FR-3714, FR-3715) and TUI panel (FR-3716).
