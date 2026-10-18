# TASKS: plan-gate

Status: Draft
Spec: [SPEC-plan-gate](../specs/SPEC-plan-gate.md)
Plan: [PLAN-plan-gate](../plans/PLAN-plan-gate.md)

- [ ] T1 — Remove `graph_enabled`; deprecation warning; init template.
  Requirements: FR-930, FR-931. Verify:
  `cargo test --locked --lib settings::graph_enabled_key_is_deprecated_and_ignored`,
  `cargo test --locked --bin dmuxai plan_gate_every_prompt_enters_triage plan_gate_empty_allowed_runners_sends_direct`
- [ ] T2 — Judge prompt, verdict parser, Korean-aware heuristic.
  Requirements: FR-933, FR-934, FR-935, FR-936, NFR-930. Verify:
  `cargo test --locked --lib graph::judge`, `cargo test --locked --lib graph::planner::tests::heuristic_route_handles_korean_and_english`
- [ ] T3 — Plan document, validation against allowed runners and `validate_model_for_runner`, DAG build, record fields.
  Requirements: FR-939, FR-944, FR-946. Verify:
  `cargo test --locked --lib graph::plan_doc`, `cargo test --locked --lib graph::store::tests::run_record_triage_and_approval_round_trip`
- [ ] T4 — App triage state machine: judge turn, timeout/fallback, 1-node run, plan turn, repair, approval, edit, reject, `/plan`, `/direct`, `/graph approve|edit|reject`.
  Requirements: FR-932, FR-935, FR-937, FR-938, FR-940, FR-941, FR-942, FR-943, FR-944, FR-945. Verify:
  `cargo test --locked --bin dmuxai judge_ simple_verdict concurrent_plan_decisions direct_override complex_verdict invalid_plan plan_fallback awaiting_approval stale_awaiting approve_starts edit_replans reject_stops sensitive_gate_kept slash_list_has_plan`
- [ ] T5 — TUI judge line and approval panel.
  Requirements: UX-930, UX-931. Verify:
  `cargo test --locked --bin dmuxai judge_reason_line_renders plan_approval_panel_renders`
- [ ] T6 — Dashboard: snapshot triage/approval, `plan_decision`, route selector, approval panel, parity rows.
  Requirements: UX-930, UX-932, UX-933, INT-930. Verify:
  `cargo test --locked --lib web::`, `node --check src/web/assets/app.js`, `node --check src/web/assets/session.js`,
  `cargo test --locked --test web_plan_gate -- --ignored`
- [ ] T7 — To-do runs and headless `run` (`--approve-plan`, exit 7, `--graph` no-op), docs/API.md.
  Requirements: FR-947. Verify:
  `cargo test --locked --lib web::server::tests::todo_graph_run_waits_for_plan_approval`, `cargo test --locked --test cli run_complex_needs_approve_plan`
- [ ] T8 — Docs: notes in SPEC-graph-default-status and SPEC-agent-graph-orchestration, README, REVIEW-plan-gate, screenshots `/workspace/shots/plan-gate-*.png`.
- [ ] T9 — Gates: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` (Linux and `--target x86_64-pc-windows-gnu`), `cargo test --locked --all-targets`, `cargo test --test spec_index`.
