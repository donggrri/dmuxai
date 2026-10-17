# TASKS: Graph plan header shows effective per-runner model

Status: complete  
Spec: [SPEC-graph-plan-header.md](../specs/SPEC-graph-plan-header.md)

- [x] T1 - Add `GraphRunSession::assignment_line_with(resolve)` in
  `src/graph/executor.rs` rendering `<id>:<role>→<runner>:<model>` per node with
  `resolve(&runner, &planned_model)`; keep `assignment_line()` showing the
  planned model.
  Requirements: FR-460. Test: `cargo test assignment_line_uses_resolver_per_runner`
  (`assignment_line_uses_resolver_per_runner`, TC-460).
- [x] T2 - Use the resolved assignment line everywhere `src/app.rs` sets
  `graph_assignment_line` (`start_graph_run`, `finish_turn_outcome`,
  `continue_graph_gate`, `override_graph_node`) via
  `resolved_assignment_line` + `effective_model_for_catalogs`.
  Requirements: FR-461. Test: `cargo test graph_assignment_shows_agy_model_not_composer`
  (TC-461) and `cargo test graph_assignment_cursor_stays_composer` (TC-462/FR-462).
- [x] T3 - Update `docs/TEST-PLAN.md` (SPEC index row + dogfood matrix rows
  TC-460..462), mark dogfood #22 fixed in `docs/dogfood/README.md`, and write
  `docs/reviews/REVIEW-graph-plan-header.md`.
  Requirements: all. Verify: manual inspection, spec traceability check, and
  `cargo test --all-targets`.
