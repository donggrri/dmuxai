# PLAN: plan-gate

Status: Draft
Spec: [SPEC-plan-gate](../specs/SPEC-plan-gate.md)
Tasks: [TASKS-plan-gate](../tasks/TASKS-plan-gate.md)

## Order

1. **Settings (FR-930, FR-931).** Delete `DmuxSettings::graph_enabled` and every read of it (`app.rs` submit path, headless `req.graph`/`req.agent` toggles, tests). Parse `[graph] enabled` into a deprecation warning. Remove it from the `init` template. Rewrite TC-370 and TC-375 (SPEC-graph-default-status) to point at the new behavior.
2. **Pure graph pieces (FR-933..FR-936, FR-939, FR-944, FR-946, NFR-930).**
   - `src/graph/judge.rs`: `JUDGE_PROMPT`, `build_judge_prompt`, `parse_verdict`, `Verdict`, `TriageSource`.
   - `src/graph/plan_doc.rs`: `PLAN_PROMPT`, `build_plan_prompt` (initial / repair / edit), `parse_plan`, `validate_plan(&PlanDoc, &PlanContext) -> Result<GraphPlan, Vec<PlanError>>`; `PlanContext` carries the available runners, `graph_runner_allowed`, effective model per runner and a model-policy callback (`validate_model_for_runner`).
   - `src/graph/planner.rs`: rename `infer_roles` to `heuristic_roles`, add Korean keywords, add `heuristic_route` and `heuristic_plan`; `plan_request` stays as the fallback entry point; add `one_node_plan`.
   - `src/graph/types.rs`: new phases, `Triage`, `Approval`, `PlanRevision` (serde default).
3. **App state machine (FR-932, FR-935, FR-937, FR-938, FR-940..FR-943, FR-945).** Add `PendingTriage { run_id, stage: Judge|Plan{revision, repair}, agent, turn_id, buffer, started_at, restore_mode, prompt }`, mirroring `PendingSummary`: route `AgentOutput` for `judge-*`/`plan-*` turn ids into it, buffer `TextDelta`, finish on `Completed`, fall back on `Error`/timeout (checked on the existing tick), cancel on Ctrl+C. Mode switch to Ask/Plan before send and restore after. Add `/plan`, `/direct`, `/graph approve|edit|reject` to `SLASH_COMMANDS` (array length changes) and the dispatcher. Replace `ExecutionPlan::Single` handling with `one_node_plan` + `start_graph_run`.
4. **TUI rendering (UX-930, UX-931).** Judge line in the graph-status area; approval panel above the input with its own height budget (≤ 50%).
5. **Dashboard (UX-930, UX-932, UX-933, INT-930).** `GraphView.triage`, `GraphView.approval`; `SessionCommand::PlanDecision`; `route` on `POST /api/prompt`; `app.js` judge line + approval panel; `session.js` route selector, `/plan`, `/direct`, `/graph approve|edit|reject` entries; parity rows.
6. **To-do and headless (FR-947).** `run_todo` Graph -> triage; Agent -> direct. `cli.rs` `--approve-plan`, exit 7; `docs/API.md`.
7. **Docs.** SPEC-graph-default-status and SPEC-agent-graph-orchestration notes; README graph section; REVIEW-plan-gate.

## Coordination

- `feat/grok-runner` edits `model_policy.rs`, `app.rs` and `settings.rs`. Implementation starts after it merges and this branch rebases on main. This plan calls `validate_model_for_runner` and never edits its rules, so the grok rules flow in unchanged.
- PR #65 (`feat/graph-swimlane`) changes the graph view; the dashboard approval panel reuses whatever swimlane renderer is on main at rebase time.

## Risks

| Risk | Mitigation |
| --- | --- |
| Every prompt pays one judge turn | Simple prompts add exactly one turn (NFR-932); `/direct` skips it |
| Runner ignores "JSON only" and uses tools | Read-only mode for internal turns; last-JSON-object parsing; fallback |
| Cursor/AGY switch mode by restarting a process | Restore mode after the turn; test TC-933 checks the restore |
| `app.rs` is 13k lines and shared with other branches | Keep new logic in `src/graph/*`; app only wires state |
| Old graph records with new phases | serde defaults; unknown phases skipped |

## Rollback

Revert the squash commit. Configs are unaffected (the removed key was optional), and graph records are per-run JSON files.
