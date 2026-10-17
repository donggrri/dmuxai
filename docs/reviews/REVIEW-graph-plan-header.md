# REVIEW: Graph plan header shows effective per-runner model

Status: complete  
Spec: [SPEC-graph-plan-header.md](../specs/SPEC-graph-plan-header.md)  
Plan: [PLAN-graph-plan-header.md](../plans/PLAN-graph-plan-header.md)  
Dogfood: #22

## Spec compliance

- FR-460: `assignment_line_with` renders `<id>:<role>→<runner>:<model>` per
  non-orchestrate node and calls `resolve(&runner, &planned)` per node.
  Verified by `assignment_line_uses_resolver_per_runner` (TC-460), which also
  confirms `assignment_line()` still prints the planned model.
- FR-461: `graph_assignment_line` is set through
  `resolved_assignment_line(&self.model_catalogs, session)` at `start_graph_run`,
  `finish_turn_outcome`, `continue_graph_gate`, and `override_graph_node`
  (`src/app.rs`). Non-Cursor runners with no F2 pick resolve to the effective
  model or `provider default`, never composer-2.5. Verified by
  `graph_assignment_shows_agy_model_not_composer` (TC-461), which additionally
  verifies the F2 pick wins and the system message at run start shows the
  resolved line.
- FR-462: Cursor nodes resolve to composer-2.5 (policy pin), including with a
  disallowed F2 pick. Verified by `graph_assignment_cursor_stays_composer`
  (TC-462) and half of TC-460.

## Interpretation note

FR-461's filler text "shows `default`" is rendered as the existing status-line
wording `provider default` (used by `current_graph_status_line` and the
"Graph node" notice), keeping resolution textually identical across the
assignment line and status line. The normative part — never composer-2.5 on a
non-Cursor runner without F2 — is enforced and asserted.

## Test evidence

Deterministic, run 2026-10-17:

- `cargo test assignment_line_uses_resolver_per_runner` → ok (1 passed)
- `cargo test graph_assignment` → ok (2 passed:
  `graph_assignment_cursor_stays_composer`,
  `graph_assignment_shows_agy_model_not_composer`)
- `cargo fmt --check` → ok
- `cargo test --all-targets` → ok (77 + 145 + 4 + 19 + 10 + 3 + 4 passed,
  5 ignored: live-provider smoke tests stay `#[ignore]`)
- `cargo clippy --all-targets --all-features -- -D warnings` → exit 0

## Blocked items

- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1
  -SpecPath docs/specs/SPEC-graph-plan-header.md` was **not run**: no
  PowerShell (`pwsh`/`powershell`) on this Linux host. PowerShell equivalent
  validation was performed as a POSIX substitute: requirement IDs FR-460..462
  unique Must rows, TC-460..462 unique, every Must mapped in the Acceptance
  table + TC/FR row coverage per row (TC-460↔FR-460/462, TC-461↔FR-461) → pass.
- Note: the spec's headings (`## Requirements`, `## Acceptance`) do not match
  the script's supported section-heading patterns (`2. Functional
  requirements`/`4. Requirements`, `4./5. Acceptance scenarios...`); even under
  PowerShell the script would require the longer headings or a script update.
  Flagged as residual; sibling spec SPEC-graph-allowed-runners shares this shape.

## Residual risks

- Custom-agent runner names resolve through `runner_from_str` → their
  `effective_model_for` has no special pin; nodes planned on a custom runner
  show the planned or F2-picked custom model. This mirrors status-line
  behavior and stays within provider-neutral boundaries.
- Cursor residency: dogfood #22 marked fixed based on deterministic tests;
  visual confirmation remains inside the running app.

## Rollback notes

Revert `src/graph/executor.rs` + `src/app.rs` hunks; the persisted graph run
format is unchanged (`GraphPlan`/`GraphRun` untouched), so in-flight runs stay
loadable.
