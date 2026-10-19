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
2b. **Agent-planned DAG (FR-948..FR-953, 2026-10-19).** Delete the keyword chain from routing: `plan_request`, the fixed-order `infer_roles` graph and the linear `build_edges` go away; `heuristic_roles`/`heuristic_route` only pick simple vs complex. Fallbacks build `fallback_plan` (one step on the selected agent, FR-950). `executor::schedule` starts every ready step up to `max_parallel` and skips a step whose runner is busy with another step of the run (OQ-934). The orchestrate node carries the judge agent, model and timestamps (FR-951). `PLAN_PROMPT` gains the DAG and language rules (FR-952, FR-953).
2c. **Dynamic graph (FR-954..FR-962).** New pure module `src/graph/replan.rs` (`REPLAN_PROMPT`, `build_replan_prompt`, `parse_replan`, `apply_replan(&GraphRun, &ReplanDoc, &PlanContext) -> Result<PlanChange, Vec<PlanError>>`). `GraphLimits` gains `max_replans`, `replan_budget_chars`, `max_nodes`; `GraphRun` gains `turn_index`, `plan_history`. The App sends a `replan-<n>` internal turn only on a trigger (failed step, `DMUXAI_REPLAN:` marker, `/graph replan` or the dashboard Replan button; PendingTriage stage `Replan`) with a compact payload (goal, pending-steps JSON, ≤ 500-char summaries) inside the per-run char budget, holds scheduling while it runs (OQ-936) and applies or gates the change (OQ-935). Snapshot builds `graph.turns` from the store for the conversation and `rev_change` per node; app.js re-renders on the SSE snapshot and adds the Replan button and Revisions list.
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
| Agent plans a needless deep chain or a too-wide fan-out | ≤ 8 steps, `max_parallel`, approval gate shows the DAG before anything runs |
| Two parallel steps on one runner | Scheduler waits for the runner (OQ-934) instead of failing the run |

## Rollback

Revert the squash commit. Configs are unaffected (the removed key was optional), and graph records are per-run JSON files.
