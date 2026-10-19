# REVIEW: plan-gate

Status: Implemented (draft PR, not released; release gates in P22)
Spec: [SPEC-plan-gate](../specs/SPEC-plan-gate.md)
Tasks: [TASKS-plan-gate](../tasks/TASKS-plan-gate.md)

## Spec compliance

| Area | Status | Evidence |
| --- | --- | --- |
| Always-on graph, judge, plan approval (FR-930..FR-953) | Met | P04–P06, `tests/graph_orchestration.rs`, app integration tests |
| Sensitive gates on all run sizes (FR-944) | Met | P07, P20 `run_sensitive_gate_exits_7_headless`, TC-951 |
| Replan, caps, pending-only revisions (FR-954..FR-962) | Met | P09–P14, P11 replan tests, #67 dogfood replan rows below |
| Dashboard / TUI parity (UX-930..UX-936, INT-930) | Met | P12–P18, asset tests TC-956..TC-996 |
| To-do and headless (FR-947, FR-1010) | Met | P19–P20, `tests/cli.rs` TC-955 |
| Non-goals (no judge-model setting, no policy edits) | Met | Spec §2; no `[graph] judge` in tree |

Residual risk: `settings::builtin_opencode_tests::starter_config_parses` and `agent::cursor` `prewarm_exited_process_is_replaced` fail intermittently under load (fix tracked in P22). Automated TC-960 headless Chrome e2e not added; manual #67 browser dogfood covers Approve / Edit / Replan. The live judge path (TC-962) was not exercised because every #67 run used `/plan`. Rollback: the feature lives on the `feat/plan-gate` draft branch only; nothing is released, so rollback is not merging (or reverting the merge); old run records stay readable (new fields are serde-default).

## Task reviews (diff-only, reviewer differs from the author)

| Task | Author | Reviewer | Verdict |
| --- | --- | --- | --- |
| P01 | opencode deepseek-v4.1-flash-free (WIP) + composer-2.5 | glm-5.3-flash-free: BLOCK (judge overflow leak, busy teardown); claude-sonnet-5-5-high re-review | APPROVE after fixes |
| P02 | opencode deepseek-v4.1-flash-free + composer-2.5 fix | claude-sonnet-5-5-high | BLOCK (replan lost tasks) → fixed; follow-up moved to P09 |
| P03 | composer-2.5 (v0.2.2 graph run) | claude-sonnet-5-5-high | APPROVE |
| P03b | composer-2.5 (+2 hand fixes) | claude-sonnet-5-5-high | BLOCK (deferred slot overwrite, lost mapping, text-matched unbounded retries, duplicate dispatch) → fixed; 2nd BLOCK (mapping removal, timeout behind real turn) → fixed by hand, tests in P04 |
| P04 | composer-2.5 | claude-sonnet-5-5-high | APPROVE |
| P05 | composer-2.5 | claude-sonnet-5-5-high | APPROVE |
| P06 | composer-2.5 | claude-sonnet-5-5-high | APPROVE |
| P07 | composer-2.5 (#67 graph 89caf6df) | claude-sonnet-5-5-high | APPROVE |
| P07b | composer-2.5 (/direct) | claude-sonnet-5-5-high | APPROVE |
| P08 | composer-2.5 (#67 graph 71450cd7) | claude-sonnet-5-5-high | APPROVE |
| P09 | composer-2.5 (#67 graph 3db42ed5) | claude-sonnet-5-5-high | APPROVE |
| P10 | composer-2.5 (#67 graph 0606ec52) | claude-sonnet-5-5-high | APPROVE |
| P11 | composer-2.5 (#67 graph 4b1d8cab) | claude-sonnet-5-5-high | APPROVE after 3 BLOCK rounds (sensitive replan approval, adapter busy on replan → FR-979) |
| P12 | composer-2.5 (#67 graph a5247340) | claude-sonnet-5-5-high | APPROVE after 2 BLOCK rounds (revision counter, markdown marker) |
| P13 | composer-2.5 (#67 graph e0ae8527) | claude-sonnet-5-5-high | APPROVE after 2 BLOCK rounds (grouped panel, planner_io) |
| P14 | composer-2.5 (#67 graph 42781e37) | claude-sonnet-5-5-high | APPROVE |
| P14b | composer-2.5 (/direct 387c8376 + hand fixes) | claude-sonnet-5-5-high | APPROVE |
| P15 | composer-2.5 (#67 graph 7f96ced1 + hand fix) | claude-sonnet-5-5-high | APPROVE (hand fix: failure-sensitive replan approve cleared `graph_failure_replan`) |
| P16 | composer-2.5 (#67 graph c4ab8ea0) | claude-sonnet-5-5-high | BLOCK (panel focus / rebuild) → P16b |
| P16b | composer-2.5 (/direct b28ed165) | claude-sonnet-5-5-high | BLOCK round 2 (row reorder, view-only key, button extraClass) → P16c |
| P16c | composer-2.5 (/direct 15cc6fca) | claude-sonnet-5-5-high | BLOCK → hand fix → APPROVE (button sync scope; checklist layout) |
| P17 | composer-2.5 (#67 graph f0a750c9) | claude-sonnet-5-5-high | BLOCK (expand class, focus, layout) → fixed with P16b |
| P18 | composer-2.5 (#67 graph bfa6a6ba, rev 2) | claude-sonnet-5-5-high | BLOCK → hand fix → APPROVE (tasks_file dedupe); clippy follow-up b60323f |
| P19 | composer-2.5 (#67 graph c2208f02) | claude-sonnet-5-5-high | BLOCK → hand fix → APPROVE (todo_id leak on plain prompt; judge/plan cancel reopen to-do; FR-1010) |
| P20 | composer-2.5 (#67 graph 3a9ebe75) | claude-sonnet-5-5-high | APPROVE (hand fix: sensitive replan back to awaiting_approval exits 7) |
| P21 | composer-2.5 | — | Docs only (README, SPEC status notes, this REVIEW); no implementation diff |
| P22 | — | — | Pending (`TASKS-plan-gate`: final fmt/clippy Windows, full `--all-targets`, CI on draft PR) |

Implement steps P04–P20: cursor **composer-2.5**. Reviews: cursor-agent **claude-sonnet-5-5-high** on the diff only.

## Dogfood findings (v0.2.2 binary driving #67)

- **Run e3a7ceb5 (P03), node dispatched to a busy runner:** step-1 (research, cursor) finished and step-2 (test, cursor) was sent at once; the cursor adapter was still winding down and refused with `Cursor is already running a turn`, twice (retries), so the run failed. Covered by FR-976 / TC-1001 (P03b), extending the FR-949 per-runner wait to the adapter's own busy state.
- **Same run, stale graph state after a node failure:** the header and chat showed `Graph failed: graph node step-2 failed` while the graph panel kept the run active with step-1 running and step-2 pending. Covered by FR-977 / TC-1002 (P03b).
- **v0.2.2 node prompts are cut to 120 chars** (`job_for_role` summarize), so dogfood task briefs live in a gitignored `.dmuxai/tasks/*.md` file named in the prompt. The agent-planned DAG of #67 writes full jobs (FR-952).
- The v0.2.2 keyword planner orders roles research → test → plan → review → implement, so a review node can never follow implement; reviews of #67 tasks therefore run outside the graph on the diff.
- opencode free models were blocked for this network from about 20:00 KST (`abusive traffic from this IP`), so from P02's fix on, composer-2.5 implements and claude-sonnet-5-5-high reviews.

## Dogfood findings (#67 binary driving #67)

- Every planned #67 task P04–P20 used **`/plan`** and fix-ups used `/direct` (judge skipped; `planner_calls.judge` = 0). Plan, repair, edit and replan turns ran live on the real Cursor runner; the live judge verdict did not (TC-962 partial).
- **P11 replan rev 1:** adapter busy, 0 response chars → FR-979 internal turns wait for adapter release.
- **P12 replan rev 1:** rejected (unknown task id `task-3b`).
- **P13 replan rev 2:** applied (+1 node `planner-io-review`).
- **P15:** invalid plan then repair round succeeded.
- **P18:** plan ok but wrong scope; dashboard Edit → rev 2; replan rev 3 rejected (task already done; FR-968 correct).
- **P18 dogfood:** the verify step's `cargo test` saw `DMUXAI_WEB_TOKEN` in child env (its serve test failed and a token prefix reached the agent's local transcript) → FR-1010 strip in P19; `.dmuxai/web.json` still supplies the token to local agents by design (FR-773).
- **P20:** replan after `DMUXAI_REPLAN:` marker rejected (step size must be small or medium); headless `--approve-plan` does not bypass sensitive `waiting_gate` (exit 7).
- **Dashboard (TC-960 manual):** Approve click → `decided_by: dashboard`; Edit + Send feedback → rev 2 (P18); Replan button queued (P13 applied, P12/P18 rejections captured in screenshots).

## Planner and replan calls (measured)

All implement steps: cursor composer-2.5. Every planned #67 task used `/plan` and the fix-up runs (P07b, P14b, P16b, P16c) used `/direct`, so judge calls are 0 in these runs. Plan input is the fixed 1963-char `PLAN_PROMPT` + user prompt; replan prompts stay ~4.1–4.6k chars. P04–P13 plan/repair sizes predate FR-1008 (`planner_io`); only replan sizes exist for P11–P13.

| Task | Run | Call | Prompt chars | Response chars | Result |
| --- | --- | --- | --- | --- | --- |
| P11 | 4b1d8cab | replan rev 1 | 4606 | 0 | rejected (adapter busy, fixed by FR-979) |
| P12 | a5247340 | replan rev 1 | 4424 | 1266 | rejected (unknown task id task-3b) |
| P13 | e0ae8527 | replan rev 2 | 4491 | 2315 | applied (+1 node planner-io-review) |
| P14 | 42781e37 | plan | 1963 | 3346 | ok |
| P15 | 7f96ced1 | plan | 1963 | 2803 | invalid |
| P15 | 7f96ced1 | repair | 2201 | 2652 | ok |
| P16 | c4ab8ea0 | plan | 1963 | 3619 | ok |
| P17 | f0a750c9 | plan | 1963 | 2457 | ok |
| P18 | bfa6a6ba | plan | 1963 | 2242 | ok (wrong scope) |
| P18 | bfa6a6ba | edit (dashboard feedback) | 4511 | 2079 | ok, rev 2 |
| P18 | bfa6a6ba | replan rev 3 (dashboard button) | 4386 | 1327 | rejected (task already done; FR-968 correct) |
| P19 | c2208f02 | plan | 1963 | 3553 | ok |
| P20 | 3a9ebe75 | plan | 1963 | 3462 | ok |
| P20 | 3a9ebe75 | replan rev 2 (agent DMUXAI_REPLAN marker) | 4135 | 3358 | rejected (step size must be small or medium) |

**Totals P11–P20:** 14 calls, prompt 42,495 chars, response 34,479 chars (~10.6k and ~8.6k tokens if ~4 chars/token — rough estimate only).

## Screenshots

Light theme, not in the repo (screenshots from the #67 dogfood runs):

`v023-plangate-approval-dashboard.png`, `v023-plangate-approval-panel.png`, `v023-plangate-approval-edit-feedback.png`, `v023-plangate-approval-rev2-dashboard.png`, `v023-plangate-approval-P13.png`, `v023-plangate-approval-P19.png`, `v023-plangate-approval-P20.png` (DAG with a parallel branch), `v023-plangate-tasks.png`, `v023-plangate-tasks-highlight.png`, `v023-plangate-replan-applied-P13.png` (replan that added a node), `v023-plangate-replan-031955.png`, `v023-plangate-replan-050318.png`, `v023-plangate-replan-P12-rejected.png`, `v023-plangate-replan-dashboard.png`, `v023-run-P03.png` .. `v023-run-P20.png`.

## Local gates

GitHub Actions is unavailable for this repository; local gates stand in for CI.

Latest recorded pass on the P20 tree (`CARGO_TARGET_DIR` isolated target dir):

| Gate | Result |
| --- | --- |
| `cargo fmt --check` | pass |
| `cargo clippy --locked --all-targets -- -D warnings` | pass |
| `cargo test --locked --test cli` | pass (7 tests, incl. TC-955) |
| `cargo test --locked --lib` | pass (353 passed, 1 ignored) |
| `cargo test --locked --bin dmuxai` | pass (325 passed, 6 ignored) |
| `cargo test --locked --test spec_index` | pass (3/3) |
| `check-spec` BASELINE + SPEC-plan-gate | pass |

P22 still requires Windows clippy (`x86_64-pc-windows-gnu`) and full `cargo test --locked --all-targets` before release.

## Open nits / follow-ups

- Initial plan marks every node `added` in the swimlane (rev 1 counts as the latest applied change) — fixed in P22.
- TUI screenshots (judge line, TUI approval panel) not taken as PNGs.
- Checklist titles are cut at 280 px (no wrap/ellipsis tooltip) — dogfood P19 screenshot.
- Temp tasks file is left behind when write/flush (not rename) fails.
- Asset tests are string checks; Move-down and CSS-rule anchoring not pinned.
- TC-954 server test uses an app double; the real reject→open path is covered by the app test.
- MCP servers strip `DMUXAI_WEB_TOKEN` before user env, custom agents after (a user-set value passes only for MCP).
- Unit tests that run in the repo workspace leave fake judge records under `.dmuxai/graphs` (gitignored).
- `.dmuxai/web.json` still gives local agents the dashboard token by design (FR-773); FR-1010 only removes the env copy.
- P20 docs nit: todo run `--graph` still selects graph mode (document in API).
- P20: cancel record reuses kind Initial and the previous rev; fake-agent `pick_runner` takes the first bullet.
