# TASKS: plan-gate

Status: In progress
Spec: [SPEC-plan-gate](../specs/SPEC-plan-gate.md)
Plan: [PLAN-plan-gate](../plans/PLAN-plan-gate.md)

Rules: one task at a time, each sized for one opencode free-model session (≤ 30 min). Per task: implement, `cargo fmt --all`, `cargo clippy --locked --all-targets -- -D warnings`, the task's tests, tick it here, commit with the task number (`P07: …`), push with `--force-with-lease`. Review by a different free model on the diff only. Never more than one task open.

Shared gate for every task (in addition to its own check): no test hangs — run app tests as `timeout 300 cargo test --locked --bin dmuxai`.

## Done (before the split into P-tasks)

- [x] T1 — Remove `graph_enabled`, deprecation warning, init template (FR-930, FR-931; TC-932). Commit 85383a3.
- [x] T2 — Judge prompt, verdict parser, Korean-aware heuristic (FR-933..FR-936, NFR-930; TC-935, TC-936, TC-961). Commits 773ab53, 7d21458.
- [x] T3 — Plan document, validation, DAG build, record fields (FR-939, FR-944, FR-946; TC-942, TC-943, TC-953). Commits 773ab53, fe01f39.
- [x] T3b — 1-node fallback plan, DAG plan prompt, parallel scheduling with per-runner wait (FR-949, FR-950, FR-952, FR-953; TC-965..TC-967, TC-969, TC-988). Commits 7d21458, 7c2c9ad.
- [x] T3c-pure — Replan module: compact prompt, marker, budget, `apply_replan` (FR-956..FR-958, FR-961; TC-974, TC-975, TC-978). Commit 0b173bc.

## Remaining (execution order)

- [x] P01 — (7e547c2; composer-2.5 on opencode deepseek WIP; review glm BLOCK → fixed; sonnet-5.5-high re-review APPROVE) Finish the triage state machine (judge turn, timeouts, `/direct`, quiet 1-node runs; WIP commit 1375768): make all app tests pass, fix the `headless_run_reports_agent_error` hang, `/direct` parity row.
  FR-930, FR-932, FR-935, FR-937, FR-948, FR-951, FR-954. TC-930, TC-931, TC-933, TC-934, TC-937..TC-940, TC-964 (simple), TC-968 (simple).
  Files: `src/app.rs`, `src/web/parity.rs`, `tests/graph_orchestration.rs`.
  Done when: `timeout 300 cargo test --locked --bin dmuxai` 0 failed, `cargo test --locked --test graph_orchestration` green.
- [x] P02 — (opencode deepseek-v4.1-flash-free + composer-2.5 fix; review sonnet-5.5-high BLOCK → fixed; 2nd-round finding "replan steps need `task` in the replan prompt" moved to P09) Plan `tasks` schema in the plan JSON, validation, task→step mapping, `PLAN_PROMPT` split rule (pure).
  FR-963, FR-964, FR-975. TC-991, TC-992, TC-993.
  Files: `src/graph/plan_doc.rs`, `src/graph/types.rs` (`PlanTask`, `GraphNode.task`, `GraphRun.tasks` serde default).
  Done when: `cargo test --locked --lib graph::plan_doc` green incl. `plan_tasks_parse_and_validate`, `plan_tasks_invalid_cases`, `tasks_build_dag_with_parallel_branches`.
- [x] P03 — Task status derivation, implicit task for 1-node runs, persistence round-trip (pure + 1-node wiring).
  FR-966, FR-967 (record part), FR-969. TC-995, TC-998 (round-trip part).
  Files: new `src/graph/tasks.rs`, `src/graph/store.rs`, `src/app.rs` (1-node builder only).
  Done when: `task_status_follows_nodes`, `one_node_run_has_implicit_task` green.
- [x] P03 — (composer-2.5 in v0.2.2 graph run e3a7ceb5; review claude-sonnet-5-5-high APPROVE) see below.
- [x] P03b — (composer-2.5 single turns; review claude-sonnet-5-5-high BLOCK ×2 → fixed, last two fixes by hand: keep a running node's runner mapping, timeout counts only adapter refusals; follow-up tests for those two still to add in P04) Graph robustness from the v0.2.2 dogfood run e3a7ceb5: a node is never sent to a runner whose adapter is still busy (start refusal keeps it ready, FR-976), and a node failure marks the run failed in the record, snapshot and graph panel (FR-977).
  FR-976, FR-977, FR-949. TC-1001, TC-1002.
  Files: `src/app.rs`, `src/graph/executor.rs`, `src/web/snapshot.rs`.
  Done when: `next_node_waits_for_adapter_release`, `node_failure_marks_run_failed_everywhere` green.
- [x] P04 — (composer-2.5 single turns via dmuxai cursor runner; review claude-sonnet-5-5-high BLOCK ×2 → fixed (round-2 cancel-scope and retire fixes by hand) → APPROVE) Real plan turn: `begin_planning`, validate, one repair turn, fallback revision, `awaiting_approval`.
  FR-938, FR-939 (app), FR-940, FR-941, FR-950 (app). TC-941, TC-944, TC-945.
  Also (from P03b review): tests `defer_keeps_running_node_mapping` (two ready cursor nodes, `max_parallel` 2) and `defer_timeout_ignores_wait_behind_real_turn`.
  Files: `src/app.rs`.
  Done when: `complex_verdict_starts_plan_turn`, `invalid_plan_gets_one_repair`, `plan_fallback_requires_approval` green.
- [x] P05 — (composer-2.5 single turn via dmuxai cursor runner; review claude-sonnet-5-5-high APPROVE, nit fixed by hand: stale flag scoped to the restored plan id; open nits for P06: reject on stale run, trim before slash check, `//` escape guard, multiple stale awaiting records, startup stale notice) `/plan` override, awaiting-approval blocks new prompts, stale records after restart.
  FR-942, FR-945. TC-946, TC-947, TC-952 (app part).
  Files: `src/app.rs`, `src/web/parity.rs`.
  Done when: `awaiting_approval_blocks_new_prompts`, `stale_records_after_restart`, `slash_list_has_plan_and_direct` green.
- [x] P06 — (composer-2.5 two turns via dmuxai cursor runner; review claude-sonnet-5-5-high BLOCK → fixed → APPROVE, two nits fixed by hand: run-id guard and replan counter kept on edit; dashboard 409 wiring is P15) `decide_plan` approve / edit / reject, `/graph approve|edit|reject`, first decision wins.
  FR-943, FR-951 (complex). TC-948, TC-949, TC-950, TC-963, TC-964/TC-968 (complex parts).
  Files: `src/app.rs`.
  Done when: `approve_starts_graph`, `edit_replans_with_feedback`, `reject_stops_run`, `concurrent_plan_decisions_first_wins` green.
- [ ] P07 — Sensitive gates on planned and 1-node runs.
  FR-944. TC-951.
  Files: `src/app.rs`, `src/graph/plan_doc.rs`.
  Done when: `sensitive_gate_kept_for_plans` green.
- [ ] P08 — Task edits at the gate: rename, delete, merge, move (local revision), split (edit turn); `/graph task …`.
  FR-965. TC-994.
  Files: `src/graph/tasks.rs`, `src/app.rs`.
  Done when: `task_edits_rebuild_plan`, `task_split_uses_edit_turn` green.
- [ ] P09 — Replan review fixes: `replans_used`/`replan_chars_used` counters, reject deps on failed/skipped nodes, `revision_needs_approval_cases`, drop non-executable deps; replans revise only pending tasks.
  FR-957, FR-958, FR-968. TC-974, TC-975, TC-999.
  Also: `REPLAN_PROMPT` asks for `task` on every new/changed step and a replan step without `task` inherits the task of the existing node with the same id (from P02 review); cut every model-provided id in PlanError text.
  Files: `src/graph/replan.rs`, `src/graph/types.rs`.
  Done when: `cargo test --locked --lib graph::replan` green incl. `replan_revises_only_pending_tasks`.
- [ ] P10 — Replan triggers: `NODE_REPLAN_HINT` on multi-node prompts, marker strip, failure hold, `/graph replan [note]`.
  FR-956, FR-957. TC-973.
  Files: `src/app.rs`.
  Done when: `replan_only_on_trigger` green.
- [ ] P11 — Replan send/apply: budget check, hold scheduling, apply or reject, sensitive revision approval, `plan_history` + event log, TUI notice; every turn recorded.
  FR-954, FR-959, FR-960, UX-935. TC-976, TC-979 (TUI), TC-989.
  Files: `src/app.rs`.
  Done when: `sensitive_replan_needs_approval`, `revision_notice_renders`, `every_turn_is_recorded_as_graph_run` green.
- [ ] P12 — TUI judge line (shared text helpers).
  UX-930. TC-956.
  Files: new `src/graph/triage_view.rs`, `src/app.rs` (render).
  Done when: `judge_reason_line_renders` green.
- [ ] P13 — TUI approval panel with steps grouped by task, `tasks d/t` counter.
  UX-931, UX-937, FR-975. TC-958, TC-997.
  Files: `src/graph/triage_view.rs`, `src/app.rs`.
  Done when: `plan_approval_panel_renders`, `task_counter_and_grouped_panel_render` green.
- [ ] P14 — Snapshot: `triage`, `approval`, `plan_history`, `rev_change`, `turns` + `follow_up`, `tasks` with derived status.
  FR-955, FR-962, UX-936 (data). TC-957, TC-977 (snapshot), TC-990, TC-996 (snapshot).
  Files: `src/web/snapshot.rs`.
  Done when: `cargo test --locked --lib web::snapshot` green incl. `snapshot_carries_task_checklist`.
- [ ] P15 — Dashboard commands: `plan_decision`, `plan_replan`, `plan_task_edit`, `POST /api/prompt` `route`.
  UX-932, UX-933 (server), FR-965 (dashboard). TC-959.
  Files: `src/web/commands.rs`, `src/web/server.rs`, `src/app.rs`.
  Done when: `plan_decision_command_contract` + route test green.
- [ ] P16 — Dashboard assets: route selector, judge line, approval panel (tasks + nodes, task actions), revisions list, `added`/`changed` badges, parity rows.
  UX-932..UX-934, INT-930. TC-952 (asset), TC-957 (asset), TC-960 (asset), TC-979 (asset).
  Files: `src/web/assets/app.js`, `src/web/assets/session.js`, CSS, `src/web/assets.rs`, `src/web/parity.rs`.
  Done when: `cargo test --locked --lib web::` green, `node --check` on both JS files.
- [ ] P17 — Dashboard task checklist next to the swimlane (live status, click highlights nodes).
  UX-936. TC-996 (asset).
  Files: `src/web/assets/app.js`, CSS, `src/web/assets.rs`.
  Done when: `task_checklist_asset` green, `node --check`.
- [ ] P18 — `[graph] tasks_file` setting and markdown writer, documented path.
  FR-967. TC-998.
  Files: `src/settings.rs`, `src/init.rs`, `src/graph/tasks.rs`, `src/app.rs`, `docs/API.md`.
  Done when: `tasks_file_setting_and_persistence` green.
- [ ] P19 — To-do runs through triage; explicit agent = `/direct` (OQ-932, resolved yes).
  FR-947. TC-954.
  Files: `src/web/server.rs`, `src/app.rs`.
  Done when: `todo_graph_run_waits_for_plan_approval` green.
- [ ] P20 — Headless `dmuxai run`: `--approve-plan`, exit 7, `--graph` no-op, `triage.source` in JSON, sensitive gate exit 7; fake-agent `judge`/`plan` behaviours; docs/API.md.
  FR-947, FR-944 (headless). TC-955.
  Files: `src/cli.rs`, `src/app.rs`, `tests/cli.rs`, `tests/fixtures/fake-agent.sh`, `docs/API.md`.
  Done when: `cargo test --locked --test cli` green incl. `run_complex_needs_approve_plan`.
- [ ] P21 — Docs and real-run screenshots (light theme, `/workspace/shots/v023-plangate-*.png`): agent-planned DAG with a parallel branch, approval gate with tasks, replan that added a node, task checklist next to the swimlane, judge line; notes in SPEC-graph-default-status, SPEC-agent-graph-orchestration, README; REVIEW-plan-gate with measured planner/replan call counts and sizes.
  TC-960 (e2e), TC-979 (manual), TC-996 (screenshot).
  Done when: screenshots exist and REVIEW doc filled.
- [ ] P22 — Final gates: `cargo fmt --check`, clippy Linux and `--target x86_64-pc-windows-gnu`, `cargo test --locked --all-targets`, `cargo test --test spec_index`, CI green on the draft PR.
