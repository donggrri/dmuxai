# TASKS: Board intake from a prompt (planner auto plan)

Spec: [SPEC-board-intake](../specs/SPEC-board-intake.md) ·
Plan: [PLAN-board-intake](../plans/PLAN-board-intake.md)

- [x] T1 — Fallback plan saved to `graph-pending/<hash>.json`; exit-7 JSON has
  `plan_hash`, `plan_source`, masked `planner_error` (≤ 500). FR-3860, NFR-3860.
  `cargo test --test headless_fallback_plan tc_3860`
- [x] T2 — `run --graph --plan-only` (+ MCP `plan_only`): always plans, never
  executes, exit 7; flag conflicts exit 2. FR-3866.
  `cargo test --test headless_fallback_plan tc_3866`
- [x] T3 — `planning` status on items, `set_planning`, key-idempotent create.
  FR-3861, FR-3863. `cargo test --lib tc_3863`
- [x] T4 — Intake runner: validation before create, plan lock, plan-only child,
  attach via `attach_plan`, failure masking, detached `board intake`. FR-3861,
  FR-3864. `cargo test --lib tc_3861` · `cargo test --lib tc_3864`
- [x] T5 — Fallback warning in `board show`, `board pending` and the modal.
  FR-3862. `cargo test --lib tc_3862`
- [x] T6 — CLI `board add --prompt P|- [--detach]`, MCP `board_add.prompt`,
  HTTP `POST /api/board`. FR-3861. `cargo test --lib tc_3861` ·
  `cargo test --bin dmuxai`
- [x] T7 — Dashboard New task form, `Planning…` card, hidden in `serve`.
  FR-3865. `cargo test --lib tc_3865` ·
  `cargo test --test web_board_e2e -- --ignored tc_3865`
- [x] T8 — Docs: API.md (CLI, MCP, HTTP, decision board loop), README board
  section, CHANGELOG, REVIEW; spec status `implemented`;
  `bash scripts/check-spec.sh docs/specs/SPEC-board-intake.md`,
  `python3 scripts/check-spec.py --all`, `cargo test --test spec_index`.
