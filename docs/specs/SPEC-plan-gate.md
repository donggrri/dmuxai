# SPEC: plan-gate (always-on graph, judge, plan approval)

Status: Draft (v0.2.3 feature 1)
Owner: donggrri
Created: 2026-10-18
Baseline: `787436b` (origin/main)
Related: Dongjin 2026-10-18 22:32 KST (approved design); supersedes SPEC-graph-default-status FR-370 (`[graph] enabled` toggle) and the keyword routing of SPEC-agent-graph-orchestration (`infer_roles`); keeps SPEC-graph-allowed-runners FR-430..431 and the sensitive-step gate (`/graph continue`); SPEC-model-policy; SPEC-dashboard-parity FR-784; SPEC-graph-swimlane

## 1. Decision summary

Today a prompt reaches the graph planner only while `[graph] enabled` is on, and the planner decides between "single agent" and "graph" by matching English keywords (`plan`, `review`, `fix`…). Korean prompts match nothing and almost always run as a single agent, the `orchestrate` node never runs, and nothing is shown to the user before a multi-step graph starts.

Decision:

1. **Graph engineering always runs.** `[graph] enabled` is removed. Every prompt that does not start with `/` goes through the graph path. Old configs still load and get a deprecation notice.
2. **Judge.** The currently selected agent (its runner and model, not a separate judge model) reads the prompt and returns a strict JSON verdict, `simple` or `complex`, with a one-line reason. The reason is shown in the TUI and the dashboard. The English keyword heuristic stays only as a fallback when the judge fails or times out, and the fallback is labelled.
3. **Simple** runs at once as a 1-node graph: no approval, same records, graph view and swimlane as a multi-node run.
4. **Complex.** The selected agent writes a plan (steps, role, runner and model per step, dependencies). DmuxAI validates it against the model policy and allowed runners and shows it in the TUI and the dashboard for **approve** (run the graph), **edit** (feedback, re-plan) or **reject** (stop).
5. **Overrides.** `/plan <prompt>` forces planning, `/direct <prompt>` forces a 1-node run.

## 2. Scope

### In scope

- Always-on graph path for TUI prompts, dashboard prompts, dashboard to-do runs and headless `dmuxai run`.
- Judge turn and plan turn on the selected agent, strict JSON parsing, timeouts, deterministic fallback.
- Plan validation (schema, DAG, runner allow-list, model policy) with one automatic repair round.
- Plan approval panel in the TUI and in the dashboard with identical approve / edit / reject semantics.
- `/plan` and `/direct` slash commands; `/graph approve|edit|reject`.
- Persisting the verdict, the plan revisions and the decision in the graph run record.
- Removing `[graph] enabled` with a migration notice.

### Out of scope / non-goals

- A separate judge-model or planner-model setting. There is **none**; the selected agent judges and plans. Do not add `[graph] judge` or similar.
- Changing the sensitive-step gate (`requires_gate`, `/graph continue`) beyond FR-944.
- Per-node worktree isolation, goal loops, scheduled runs, mid-run interruption, `ask_user` MCP (dropped from v0.2.3).
- diff review sidebar and the INTENT/MISSION world model (v0.2.3 features 2 and 3, separate specs).
- Token or cost tracking (dropped).
- Changing which models the policy allows. This spec only *calls* `model_policy::validate_model_for_runner`; the `grok` runner's model rules belong to SPEC-grok-runner.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-930 | Assumed: a simple (1-node) run does not get the sensitive-step gate, exactly like today's single-agent path. A sensitive prompt that the judge calls simple therefore runs without `/graph continue`. Alternative: gate 1-node runs too. Decision needed from Dongjin. | Dongjin | Implementation start |
| OQ-931 | Assumed timeouts: judge 90 s, plan 240 s (constants, not settings). Slow runners (cursor cold start, AGY) may need more. | donggrri | Review |
| OQ-932 | Assumed: to-do runs with an explicit agent and `dmuxai run --agent` behave like `/direct` (1-node graph on that agent, no judge), because the caller already chose. | Dongjin | Implementation start |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Selected agent | The agent tab active when the prompt is submitted (TUI `selected`, dashboard `selected_agent`), with its current mode and effective model | `[routing] default_agent` |
| Judge turn | An internal turn on the selected agent that returns the triage verdict JSON (§6) | A user-visible chat turn |
| Verdict | `{ "route": "simple" \| "complex", "reason": "<one line>" }` | The plan |
| Triage source | `judge` (valid verdict), `fallback` (judge failed or timed out, heuristic used), `override` (`/plan`, `/direct`, explicit agent) | |
| Plan turn | An internal turn on the selected agent that returns the plan JSON (§6) | The `plan` node role |
| Plan revision | One plan proposal; revision 1 is the first, each edit adds one | |
| Approval gate | The pause between a validated plan and execution | The sensitive-step gate (`/graph continue`, per node) |
| 1-node graph | A `GraphRun` with exactly one executable node plus the triage record node | Today's `ExecutionPlan::Single`, which wrote no run record |

Run phases (persisted as `GraphRun.phase`):

| From | Event | To |
| --- | --- | --- |
| (new) | non-slash prompt | `triage` |
| `triage` | verdict simple, or `/direct` | `active` (1-node) |
| `triage` | verdict complex, or `/plan` | `planning` |
| `planning` | valid plan | `awaiting_approval` |
| `planning` | invalid twice, or plan turn failed | `awaiting_approval` with the fallback plan (FR-941) |
| `awaiting_approval` | approve | `active` |
| `awaiting_approval` | edit (feedback) | `planning` (revision + 1) |
| `awaiting_approval` | reject | `rejected` (terminal) |
| `triage`, `planning` | cancel (Ctrl+C / dashboard cancel) | `cancelled` (terminal), prompt restored |
| `active` | existing behavior | `waiting_gate`, `completed`, `failed` |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-930 | Must | Given any settings, when the user submits a prompt that does not start with `/` (TUI Enter, dashboard `POST /api/prompt`) | Then the prompt enters the graph path (phase `triage`). There is no setting that bypasses it. `DmuxSettings` has no `graph_enabled` field | `[graph] allowed_runners = []` keeps today's behavior: a system notice and a direct send to the selected agent, recorded as a 1-node run with source `override` | TC-930, TC-931 |
| FR-931 | Must | Given a config file (user or workspace) with `[graph] enabled = true` or `false`, when settings load | Then the key is ignored, a config warning `[graph] enabled is deprecated: graph engineering is always on; remove the key` is shown once in the TUI notices and in `dmuxai status --json` warnings, and every other `[graph]` key still applies | `dmuxai init` no longer writes the key. A config with only `[graph] enabled` loads without error | TC-932 |
| FR-932 | Must | Given phase `triage`, when the judge turn starts | Then it is sent to the **selected agent** with that agent's effective model, `resume_session = None`, a fresh internal turn id (`judge-<n>`), and the judge prompt (§6) that embeds the user prompt verbatim. The turn runs in the agent's read-only mode (Ask, or Plan where Ask is not offered) and the user's mode is restored after it ends. Its streamed text is buffered, not shown as an agent message | If the selected agent is busy, the prompt is restored to the input and `"<agent> is busy"` is shown, as today. No settings key selects another judge runner or model | TC-933, TC-934 |
| FR-933 | Must | Given the judge text | Then DmuxAI extracts the **last** top-level JSON object in the text (fenced ```json``` or bare), requires `route` ∈ {`simple`,`complex`} and a non-empty `reason`, trims the reason to one line of at most 160 chars, and ignores unknown keys | Missing keys, other `route` values, non-JSON or more than 64 KiB of text are a judge failure (FR-935) | TC-935 |
| FR-934 | Must | Given a Korean, English or mixed prompt | Then the judge prompt instructs the model to judge the task, not the language, and to write `reason` in the prompt's language. The parser accepts any Unicode reason | Unicode is cut on char boundaries, never bytes | TC-935, TC-936 |
| FR-935 | Must | Given the judge turn errors, is cancelled by the adapter, returns an unparsable verdict, or does not complete within 90 s | Then DmuxAI cancels the turn if still running, runs the deterministic heuristic (today's `infer_roles` plus Korean keywords, FR-936), and sets triage source `fallback` with reason `fallback: <timeout\|error\|invalid verdict> (heuristic)` | User cancel (Ctrl+C) is not a fallback: the run becomes `cancelled` and the prompt is restored | TC-937, TC-938 |
| FR-936 | Should | Given the fallback heuristic | Then it keeps today's English keywords and adds Korean equivalents (조사/탐색/찾아, 테스트/검증, 계획/설계, 리뷰/검토, 구현/수정/고쳐/추가/리팩터). It returns simple when at most one executable role is inferred, otherwise complex with a heuristic plan | The heuristic is a pure function with no I/O | TC-936 |
| FR-937 | Must | Given verdict simple (judge or fallback) or `/direct <prompt>` | Then a `GraphRun` is created at once with one executable node `step-1` (role `implement`, runner = selected agent, model = its effective model, job = the prompt verbatim), saved to `.dmuxai/graphs/<id>.json`, published to the dashboard graph view and swimlane like any run, and executed without an approval step | The status line shows `Graph 1/1 · step-1 implement · <runner> <model> · running`. A model rejected by the policy for that runner fails the run before sending, with the policy message | TC-939, TC-940 |
| FR-938 | Must | Given verdict complex or `/plan <prompt>` | Then a plan turn is sent to the selected agent (same mode, session and buffering rules as FR-932, turn id `plan-<n>`) with the plan prompt (§6) listing the allowed runners and, per runner, its allowed model ids. The run is in phase `planning` and the TUI and dashboard show `Planning with <runner> <model>…` | Plan turn error or a timeout of 240 s goes to FR-941 | TC-941 |
| FR-939 | Must | Given plan JSON | Then the plan is valid only if: 1 to 8 steps; ids unique and match `^[a-z0-9][a-z0-9-]{0,31}$`; `role` ∈ {research, explore, test, implement, draft, plan, route, review}; `job` non-empty; every `depends_on` id exists; the graph is acyclic; `runner` is an available agent and allowed by `graph_runner_allowed`; `model` (when given) passes `model_policy::validate_model_for_runner(runner, model)`. An omitted model means the runner's effective model, which must also pass. Each error names the step id and rule | `*-fast` models and any non-`composer-2.5` model on cursor are rejected by the policy call, never by a second copy of the rules | TC-942, TC-943 |
| FR-940 | Must | Given an invalid plan on the first attempt | Then one repair plan turn is sent with the validation errors appended. A valid repaired plan proceeds as normal | The repair is counted inside the same revision | TC-944 |
| FR-941 | Must | Given the repair is still invalid, or the plan turn failed or timed out | Then DmuxAI builds the heuristic plan (FR-936) with runners from `assign_runner` and policy-valid models, marks the revision `source: fallback` with the reason, and still requires approval | If even the heuristic plan has no allowed runner, the run fails with `no allowed runner for plan` | TC-945 |
| FR-942 | Must | Given a validated plan revision | Then phase is `awaiting_approval`, the TUI shows the plan approval panel (UX-931) and the dashboard shows the approval panel (UX-932). No node starts. New non-slash prompts are refused with `A plan is awaiting approval: approve, edit or reject it first` and restored to the input (dashboard: HTTP 409) | Restarting DmuxAI while awaiting approval keeps the record with phase `awaiting_approval`; it is shown as stale and can only be rejected | TC-946, TC-947 |
| FR-943 | Must | Given `awaiting_approval`, when the user decides | Approve (TUI `/graph approve`, dashboard Approve): the plan becomes the run graph (nodes from steps, edges from `depends_on`, limits from `[graph] max_parallel`/`max_retries`) and scheduling starts as today. Edit (`/graph edit <feedback>`, dashboard Edit with text): a new plan turn runs with the previous plan and the feedback (revision + 1, at most 5 revisions, after that edit is refused). Reject (`/graph reject`, dashboard Reject): phase `rejected`, no node runs, `Plan rejected.` is shown | Empty feedback is refused. A decision for a plan id that is not awaiting approval is refused (dashboard 409) | TC-948, TC-949, TC-950 |
| FR-944 | Must | Given a multi-node run | Then the sensitive-step gate works as today: nodes whose step text or the user prompt match the sensitive keywords pause with `Gate: confirm sensitive step … with /graph continue`. A plan step may also set `"sensitive": true`, which sets the gate | 1-node runs get no gate (OQ-930) | TC-951 |
| FR-945 | Must | Given the slash command list | Then `/plan` (`Plan first, then ask for approval`) and `/direct` (`Run on the selected agent without planning`) appear in the TUI slash list, `session.commands` and the dashboard command menu. `/plan` or `/direct` with no text shows its usage line. `/graph` gains `approve`, `edit <feedback>` and `reject` | Unknown `/graph` sub-commands still show the graph status | TC-952 |
| FR-946 | Must | Given a run record | Then `GraphRun` persists `triage {route, reason, source, runner, model, elapsed_ms}` and `approval {state: not_required\|pending\|approved\|rejected, revisions: [{n, source, plan, feedback?, errors?}], decided_by: tui\|dashboard\|cli, decided_at_ms}`. Records written before this change still load (missing fields default) | The judge and plan raw text is not stored beyond 4 KiB per revision | TC-953 |
| FR-947 | Must | Given dashboard to-do runs and headless `dmuxai run` | Then a to-do `mode: graph` run and `dmuxai run <prompt>` go through the judge on the selected (headless: default) agent. A complex plan from a to-do run waits in the dashboard and TUI approval panel; reject sets the to-do back to `open` with note `plan rejected`. Headless `run` prints the plan and exits with code 7 `plan approval required` unless `--approve-plan` is passed. `--agent X` and to-do `mode: agent` act as `/direct` on that agent (OQ-932). `--graph` is accepted as a no-op | Exit code 7 is added to docs/API.md | TC-954, TC-955 |
| UX-930 | Must | Given a finished triage | Then the TUI shows one line above the input, and the dashboard shows one line in the graph header: `Judge (<runner> <model>): simple — <reason>` or `complex — <reason>`. Fallback renders `Judge fallback: <cause> (heuristic) — <simple\|complex>`; overrides render `/plan` or `/direct`. The line uses the theme's muted color, the fallback word uses the warning color, and it is cut with an ellipsis to the width | While the judge runs, the line reads `Judging with <runner> <model>…` with the running spinner | TC-956, TC-957 |
| UX-931 | Must | Given `awaiting_approval` in the TUI | Then a panel above the input lists `rev <n>` and the source, then one row per step `<id> <role> <runner>:<model> ← <deps>` with the job wrapped below, then validation notes (if fallback), then the hint `/graph approve · /graph edit <feedback> · /graph reject`. It scrolls with PgUp/PgDn when taller than half the screen | The panel disappears when the phase leaves `awaiting_approval` | TC-958 |
| UX-932 | Must | Given `awaiting_approval` in the dashboard | Then the graph area shows an approval panel with the same rows, an **Approve** button, an **Edit** button that opens a feedback textarea with **Send feedback**, and a **Reject** button, each with a `data-parity` id. The panel renders the plan as the swimlane preview (pending nodes). Buttons are disabled while a decision is in flight and in view-only `serve` | Decisions go through `POST /api/command {"command":"plan_decision","plan_id","decision":"approve"\|"edit"\|"reject","feedback"?}` and return the new session view | TC-959, TC-960 |
| UX-933 | Must | Given the dashboard prompt box | Then a route selector `Auto · Plan · Direct` (`data-parity` `route`) sends `route: auto\|plan\|direct` with `POST /api/prompt`, equivalent to typing the prompt plainly, with `/plan` or with `/direct` | Missing `route` means `auto` | TC-952, TC-960 |
| INT-930 | Must | Given the parity matrix (SPEC-dashboard-parity FR-784) | Then `/plan`, `/direct`, `/graph approve`, `/graph edit` and `/graph reject` have rows whose controls exist in `session.js`, so `parity_matrix_controls_exist_in_session_js` passes | | TC-960 |
| NFR-930 | Must | Given the judge and plan prompts | Then the user prompt is passed as data inside a delimited block, never interpolated into instructions, and the internal turns never receive dashboard tokens, config secrets or other conversations' transcripts | The prompts are stored as constants in the source and covered by a snapshot test | TC-961 |
| NFR-931 | Must | Given CI | Then every judge/plan test uses a scripted mock adapter or the `tests/fixtures/fake-agent.sh` `judge`/`plan` behaviors; no test needs provider auth. A live smoke test against a real runner is `#[ignore]` | | TC-962 |
| NFR-932 | Should | Given a simple prompt | Then the added latency over today's direct send is one judge turn (bounded by FR-935's 90 s); no second turn is added for simple prompts | | Manual: latency depends on the live provider; recorded in REVIEW-plan-gate |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-930 | FR-930 | unit (app) | `test_app` with a scripted adapter | Submit `hello` | A judge turn (`judge-1`) is sent before any user turn; no code path checks a graph toggle | `src/app.rs` `plan_gate_every_prompt_enters_triage` |
| TC-931 | FR-930 | unit (app) | `allowed_runners = []` | Submit `hello` | Notice shown, prompt sent directly, run record with source `override` | `src/app.rs` `plan_gate_empty_allowed_runners_sends_direct` |
| TC-932 | FR-931 | unit | TOML with `[graph] enabled = false` and `max_parallel = 3` | Load settings | Load succeeds, `max_parallel == 3`, one deprecation warning; `init` template has no `enabled` | `src/settings.rs` `graph_enabled_key_is_deprecated_and_ignored` |
| TC-933 | FR-932 | unit (app) | Selected agent agy, mode Implement, model `gemini-3.8-flash-medium` | Submit a prompt | Judge request goes to agy with `resume_session None`, mode Ask during the turn and Implement afterwards; judge text is not in the transcript | `src/app.rs` `judge_turn_uses_selected_agent_read_only` |
| TC-934 | FR-932 | unit (app) | Selected agent busy | Submit | Prompt restored, busy notice, no run created | `src/app.rs` `judge_refused_when_selected_agent_busy` |
| TC-935 | FR-933, FR-934 | unit | Texts: fenced JSON, bare JSON after prose, two objects, Korean reason, missing `reason`, `route: "maybe"`, 70 KiB | `parse_verdict` | Last object wins; Korean reason kept and cut on char boundary at 160; invalid cases return the documented error | `src/graph/judge.rs` `parse_verdict_cases` |
| TC-936 | FR-934, FR-936 | unit | Prompts `README 오타 고쳐줘`, `조사하고 설계한 다음 구현하고 테스트해줘`, `fix typo`, `research, plan, implement and test X` | `heuristic_route` | simple, complex, simple, complex | `src/graph/planner.rs` `heuristic_route_handles_korean_and_english` |
| TC-937 | FR-935 | unit (app, paused clock) | Scripted adapter never completes | Advance 90 s | Judge turn cancelled, fallback used, reason starts `fallback: timeout` | `src/app.rs` `judge_timeout_falls_back_to_heuristic` |
| TC-938 | FR-935 | unit (app) | Adapter returns `not json` / emits Error / user presses Ctrl+C | Complete the turn | fallback `invalid verdict` / fallback `error` / phase `cancelled` with prompt restored | `src/app.rs` `judge_failure_modes` |
| TC-939 | FR-937 | unit (app) | Judge returns simple | Complete judge | One user turn on the selected agent with the prompt verbatim; record has 1 executable node, phase `active`, then `completed`; snapshot `graph` has 1 node | `src/app.rs` `simple_verdict_runs_one_node_graph` |
| TC-940 | FR-937, FR-945 | unit (app) | `/direct fix it` | Submit | No judge turn; 1-node run with source `override`; `/direct` alone shows usage | `src/app.rs` `direct_override_skips_judge` |
| TC-941 | FR-938, FR-945 | unit (app) | Judge returns complex / `/plan do X` | Complete judge / submit | Plan turn `plan-1` sent to the selected agent; prompt lists allowed runners and models; phase `planning` | `src/app.rs` `complex_verdict_starts_plan_turn` |
| TC-942 | FR-939 | unit | Valid 3-step DAG with deps | `validate_plan` | Ok; nodes and edges built from `depends_on` | `src/graph/plan_doc.rs` `validate_plan_accepts_dag` |
| TC-943 | FR-939 | unit | Plans with: 0 and 9 steps, duplicate id, bad id, unknown role, cycle, unknown dep, runner not allowed, `composer-2.5-fast` on cursor, `gpt-5` on cursor, omitted model on cursor | `validate_plan` | Each returns an error naming the step and rule; model errors come from `validate_model_for_runner` | `src/graph/plan_doc.rs` `validate_plan_rejects_each_rule` |
| TC-944 | FR-940 | unit (app) | First plan invalid, repair valid | Complete both turns | Exactly one repair turn whose prompt contains the errors; phase `awaiting_approval`, revision 1 | `src/app.rs` `invalid_plan_gets_one_repair` |
| TC-945 | FR-941 | unit (app) | Plan invalid twice / plan turn errors | Complete | Heuristic plan with source `fallback`, still `awaiting_approval`; with no allowed runner the run fails | `src/app.rs` `plan_fallback_requires_approval` |
| TC-946 | FR-942 | unit (app) | Run awaiting approval | Submit another prompt | Refused with the message, input restored, no turn sent | `src/app.rs` `awaiting_approval_blocks_new_prompts` |
| TC-947 | FR-942 | unit | Record saved with phase `awaiting_approval` | Reload app | Shown as stale; approve refused; reject allowed | `src/app.rs` `stale_awaiting_approval_can_only_be_rejected` |
| TC-948 | FR-943 | unit (app) | Awaiting approval, 3-step plan | `/graph approve` | Phase `active`, first ready nodes started per `max_parallel`, approval `approved` by `tui` | `src/app.rs` `approve_starts_graph` |
| TC-949 | FR-943 | unit (app) | Awaiting approval | `/graph edit 테스트 단계 빼줘`; then `/graph edit` empty; then 5 more edits | New plan turn with previous plan and feedback, revision 2; empty refused; 6th revision refused | `src/app.rs` `edit_replans_with_feedback` |
| TC-950 | FR-943 | unit (app) | Awaiting approval | `/graph reject`; then `/graph approve` | Phase `rejected`, no node started; later approve refused | `src/app.rs` `reject_stops_run` |
| TC-951 | FR-944 | unit (app) | Approved plan with a step `"sensitive": true`, and a prompt containing `deploy` | Schedule | Node pauses with the existing gate message; `/graph continue` resumes; a 1-node run with `deploy` is not gated | `src/app.rs` `sensitive_gate_kept_for_plans` |
| TC-952 | FR-945, UX-933 | unit | Slash list, `session.commands`, session.js | Inspect | `/plan` and `/direct` present with descriptions; route selector present in `session.js` | `src/app.rs` `slash_list_has_plan_and_direct`; `src/web/assets.rs` `prompt_route_selector_present` |
| TC-953 | FR-946 | unit | Old record JSON without new fields; new record | Load / save round trip | Old loads with defaults; new keeps triage and revisions; raw text capped at 4 KiB | `src/graph/store.rs` `run_record_triage_and_approval_round_trip` |
| TC-954 | FR-947 | integration (router + app double) | To-do with `mode: graph`, scripted complex plan | Run, then reject via `plan_decision` | To-do back to `open` with `plan rejected`; `mode: agent` skips the judge | `src/web/server.rs` `todo_graph_run_waits_for_plan_approval` |
| TC-955 | FR-947 | integration (CLI) | Built binary, custom agent = `fake-agent.sh plan` as default agent | `dmuxai run "…" --json`; again with `--approve-plan` | Exit 7 with plan in JSON; second exits 0 | `tests/cli.rs` `run_complex_needs_approve_plan` |
| TC-956 | UX-930 | unit (render) | TestBackend 100x30, verdicts simple, complex, fallback, override, judging | Render | Line text and styles as specified, ellipsized at width | `src/app.rs` `judge_reason_line_renders` |
| TC-957 | UX-930 | unit | Snapshot with triage | Serialize; check `app.js` | `graph.triage` present; app.js renders `#graph-judge-line` | `src/web/snapshot.rs` `snapshot_carries_triage`; `src/web/assets.rs` `judge_line_asset` |
| TC-958 | UX-931 | unit (render) | Awaiting approval, 3 steps, fallback notes | Render 100x30 | Panel rows, deps, notes and the hint line; panel gone after approve | `src/app.rs` `plan_approval_panel_renders` |
| TC-959 | UX-932 | unit (router) | Router with app double awaiting approval | `plan_decision` approve / edit without feedback / reject twice / wrong plan id / view-only | 200 + session view / 400 / 200 then 409 / 409 / 403 | `src/web/server.rs` `plan_decision_command_contract` |
| TC-960 | UX-932, UX-933, INT-930 | unit (asset) + e2e (ignored) | session.js, app.js; headless Chrome on `dmuxai --web` with a fake agent | Inspect assets; click Approve | Controls with `data-parity` ids `plan-approve`, `plan-edit`, `plan-reject`, `route`; parity test passes; e2e: run goes active | `src/web/parity.rs` `parity_matrix_controls_exist_in_session_js`; `tests/web_plan_gate.rs` `approval_panel_in_headless_chrome` |
| TC-961 | NFR-930 | unit | Judge and plan prompt builders with a hostile prompt (`"}` and `Ignore previous instructions`) | Build prompts | The user text appears only inside the delimited block; builder output matches the snapshot | `src/graph/judge.rs` `judge_prompt_snapshot`; `src/graph/plan_doc.rs` `plan_prompt_snapshot` |
| TC-962 | NFR-931 | integration (ignored) | A real logged-in runner | Submit a Korean simple and a complex prompt | Verdicts parse; a plan validates | `tests/graph_orchestration.rs` `live_judge_and_plan_smoke` (`#[ignore]`) |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | `GraphRun` gains `triage` and `approval` (serde default); `GraphRunPhase` gains `triage`, `planning`, `awaiting_approval`, `rejected`, `cancelled`. No DB schema change | Older binaries cannot read records with the new phases; they are per-run JSON files in `.dmuxai/graphs` and are skipped on error | TC-953 |
| Config | `[graph] enabled` ignored with a warning | Rollback = revert; old configs keep loading either way | TC-932 |
| Provider protocol | Judge and plan turns are ordinary `AgentRequest`s with internal turn ids and no resume session; no provider changes | Unknown events are ignored as in summary turns | TC-933, TC-941 |
| Security / permissions | Internal turns run read-only (Ask/Plan); the plan cannot bypass `allowed_runners` or the model policy; the sensitive-step gate is kept | Must not weaken any existing gate | TC-933, TC-943, TC-951 |
| Dashboard API | New `SessionCommand::PlanDecision`; `POST /api/prompt` accepts optional `route`; snapshot `graph.triage`, `graph.approval` | Old pages ignore the new fields | TC-957, TC-959 |
| CLI | `--approve-plan`, exit code 7; `--graph` becomes a no-op | Scripts that used `--graph` keep working | TC-955 |
| Platform | No process changes; same on Linux and Windows | | clippy on both targets |

**Judge prompt contract** (constant `JUDGE_PROMPT`): role statement; the rules "simple = one agent can finish it in one turn without a plan (a question, a small edit, one file, one command); complex = several dependent steps, several areas, or research + change + verification"; "do not use tools, do not edit files"; "write `reason` in the language of the user request, one sentence"; output `{"route":"simple|complex","reason":"…"}` and nothing else; then the user request between `<<<USER_REQUEST` and `USER_REQUEST>>>`.

**Plan prompt contract** (constant `PLAN_PROMPT`): output `{"steps":[{"id","role","job","runner","model"?,"depends_on":[],"sensitive"?}]}`; allowed roles; allowed runners with their allowed models (from the policy and catalogs); limits (≤ 8 steps); for a repair, the errors; for an edit, the previous plan JSON and the feedback; the user request in the same delimited block.

## 7. Implementation plan

See [../plans/PLAN-plan-gate.md](../plans/PLAN-plan-gate.md) and [../tasks/TASKS-plan-gate.md](../tasks/TASKS-plan-gate.md).

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Remove `graph_enabled`, deprecation warning | `src/settings.rs`, `src/init.rs`, `src/app.rs` | FR-930, FR-931 | TC-930..932 |
| 2 | Verdict parser, prompts, heuristic with Korean | `src/graph/judge.rs`, `src/graph/planner.rs` | FR-933..936, NFR-930 | TC-935, TC-936, TC-961 |
| 3 | Plan document, validation, DAG build | `src/graph/plan_doc.rs`, `src/graph/types.rs` | FR-939, FR-944, FR-946 | TC-942, TC-943, TC-953 |
| 4 | Triage/plan internal turns, timeouts, approval state in App | `src/app.rs` | FR-932, FR-935, FR-937, FR-938, FR-940..943, FR-945 | TC-933, TC-934, TC-937..941, TC-944..951 |
| 5 | TUI judge line and approval panel | `src/app.rs` | UX-930, UX-931 | TC-956, TC-958 |
| 6 | Dashboard snapshot, `plan_decision`, route selector, panel | `src/web/*` | UX-930, UX-932, UX-933, INT-930 | TC-957, TC-959, TC-960 |
| 7 | To-do and headless paths | `src/app.rs`, `src/cli.rs`, `docs/API.md` | FR-947 | TC-954, TC-955 |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals (no judge-model setting, no policy rule changes).
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (Linux and `x86_64-pc-windows-gnu`), `cargo test --all-targets` pass.
- [ ] TC-960 (e2e) and TC-962 (live) are marked run, skipped, or blocked with a reason.
- [ ] `../reviews/REVIEW-plan-gate.md` records commands, results, residual risk and rollback notes.
- [ ] Light-theme screenshots: judge line, dashboard approval panel, TUI approval panel.
