# PLAN: Graph plan header shows effective per-runner model

Status: complete  
Spec: [SPEC-graph-plan-header.md](../specs/SPEC-graph-plan-header.md)

1. Add a resolver-based assignment line in `src/graph/executor.rs`:
   - `GraphRunSession::assignment_line_with<F: FnMut(&str, &str) -> String>(resolve)` renders
     `<id>:<role>→<runner>:<model>` per node (orchestrate excluded), calling
     `resolve(&runner, &planned_model)` for every rendered node.
   - Keep `assignment_line()` as the planned-model default (`resolve` returns `planned`).
   - Add unit test `assignment_line_uses_resolver_per_runner` (TC-460, FR-460, FR-462).

2. Wire the resolved line into the app (`src/app.rs`) everywhere
   `graph_assignment_line` is set:
   - Reusable `effective_model_for_catalogs(&HashMap<Agent, ModelCatalog>, agent, planned)`
     extracted from `effective_model_for` so model resolution can run while a
     graph session borrows the run.
   - Free helper `resolved_assignment_line(&HashMap<Agent, ModelCatalog>, &GraphRunSession)`
     uses `assignment_line_with` with the same resolution as the status line:
     F2 pick wins on every runner; Cursor without it stays pinned to
     composer-2.5; non-Cursor without it shows `provider default`.
   - Use the resolved line at `start_graph_run`, `finish_turn_outcome`,
     `continue_graph_gate`, and `override_graph_node`.
   - Add app tests `graph_assignment_shows_agy_model_not_composer` (TC-461,
     FR-461) and `graph_assignment_cursor_stays_composer` (TC-462, FR-462).

3. Update documentation and traceability:
   - Add SPEC row and TC-460..462 rows to `docs/TEST-PLAN.md`.
   - Mark dogfood #22 as fixed in `docs/dogfood/README.md`.
   - Complete `docs/reviews/REVIEW-graph-plan-header.md`.

## Risks and mitigations

- **Risk:** Borrow-conflict where `graph_session.as_mut()` is live while the
  resolver reads `App` state. **Mitigation:** resolver helper takes the model
  catalogs by shared reference (`&self.model_catalogs` is field-disjoint from
  `self.graph_session`).
- **Risk:** `provider default` vs literal `default` wording in FR-461.  
  **Mitigation:** follow the established status-line wording (`provider
  default`), recorded in the review; composer-2.5 must never appear for a
  non-Cursor node without F2.
- **Risk:** Unknown runner strings in plans. **Mitigation:** resolver falls
  back to the planned model when `runner_from_str` cannot classify the runner,
  mirroring `current_graph_status_line`.

## Rollback

Code and documentation change only; no persisted format changes. Reverting
`src/graph/executor.rs` and `src/app.rs` restores printing `node.model_id`
without data migration.
