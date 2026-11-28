# TASKS: spec-review-gate

Status: part 1 and part 2 complete
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

## Part 2 (board, dashboard, TUI, #374)

- [x] T9 — Board item `spec_review` field, thread messages, `spec_review`
  notify event, `DMUXAI_BOARD_ITEM_ID` for the child, board child timeout
  suspended while pending. Requirements: FR-3711, FR-3712. Verify:
  `cargo test --lib tc_3711` and `cargo test --lib tc_3712`
  (`src/board/runner.rs`).
- [x] T10 — `POST /api/board/{id}/spec-review` with human headers and digest;
  `spec_review` in item and pending payloads. Requirements: FR-3708 (dashboard
  part), FR-3713. Verify: `cargo test --lib tc_3708` and
  `cargo test --lib tc_3713` (`src/web/board_api.rs`).
- [x] T11 — Board UI `Plan review` / `Spec review` badges, My decisions entry,
  spec review section and plan review title (FR-3714, FR-3715); TUI spec review
  panel with `a` / `c` / `x` / Esc (FR-3716). Verify:
  `cargo test --lib tc_3715`, `cargo test --bin dmuxai tc_3716`,
  `cargo test --test web_board_e2e -- --ignored tc_3714 tc_3715`.
- [x] T12 — #374: `board plan --edit`, dashboard Save plan and
  `board add --plan-hash` store the spec-first expanded plan (FR-3715 with
  FR-3503); inserted steps marked `added by spec-first`; executed graph records
  `plan_hash`. Verify: `cargo test --lib tc_3715`,
  `cargo test --bin dmuxai tc_3715`,
  `cargo test --test web_board_e2e -- --ignored tc_3715`.
- [x] T13 — Gates and cleanup: `tests/spec_review_part2.rs` deleted;
  `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-targets`, `bash scripts/check-spec.sh docs/specs/SPEC-spec-review-gate.md`,
  `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py`.
