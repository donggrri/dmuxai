# SPEC: spec-first workflow (spec → implement → test on every write run)

Status: approved (Dongjin 2026-11-27)
Owner: Dongjin
Created: 2026-11-27
Baseline: `70e9df7`
Related: #368, SPEC-plan-gate FR-937 FR-953 FR-1613, SPEC-stage-skills FR-1500 FR-1514, #301 (overhead), SPEC-spec-review-gate (human spec review after the spec step, FR-3700..FR-3717)

## 1. Decision summary

AGENTS.md, README.md, MISSION.md and docs/WORKFLOW.md say every behavior change goes Spec → Plan → Execute → Review → Test, but the runtime never enforced it: there was no spec role, the planner was free to drop tests, and simple, trivial, fallback and headless `--agent` runs were one implement turn. From now on every **write** run first writes or updates a spec, then implements, then tests. The default is `full`: even a simple or `--agent` run becomes a three-step graph `spec-1 → step-1 → test-1` on the selected agent. Missing spec or test steps in an agent plan are inserted by dmuxai, never by an extra planner turn. Read-only turns, questions, greetings, board ask and read-only fan-out are exempt.

## 2. Scope

### In scope

- `[workflow]` settings and the `--no-spec-first` override.
- A new `spec` node role and `spec` stage.
- Plan invariants with auto-insert on every path that builds or changes a plan.
- Three-step graph for simple, trivial-write, fallback, `/direct` and headless/MCP `--agent` write runs.
- Spec gate (diff check) and test gate (`test_command` or a `DMUXAI_TESTS:` marker), recorded in the run.

### Out of scope / non-goals

- Write fan-out (`--agents … --write`) legs: unchanged in this version (each leg is its own worktree); tracked as a follow-up.
- Sandboxing the agent's file access. The spec step is checked after the fact (FR-3508), not prevented from writing.
- Judging spec quality. dmuxai checks that a spec file changed, not what it says. A **human** reviews it: with `spec_first = "full"` the run pauses after the spec step for a spec review on the board, the TUI or `dmuxai spec-review` (SPEC-spec-review-gate, on by default; opt out with `[workflow] spec_review = "off"` or `--no-spec-review`).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3500 | Default `spec_first = "full"`, auto-insert, this repo `test_command = "scripts/gate.sh"` | Dongjin | Answered 2026-11-27 |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| write step | A plan step with role `implement` or `draft` | `test`, which also has write policy but is not a write step here |
| write run | A run whose turn policy is not read-only and that is not exempt (FR-3507) | A run that happens to touch no files |
| spec step | A step with role `spec` | A `plan` step, which plans but writes no spec |
| covered | A write step is covered when a spec step is an ancestor of it through `depends_on` | Direct dependency only |
| tested | A plan is tested when one test step has every write step as an ancestor | Any test step anywhere |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3500 | Must | Given a user config or a trusted workspace `dmuxai.toml` with `[workflow]` | Then `spec_first` accepts `"full"` (default), `"lite"` and `"off"`; `require_tests` (bool, default `true`); `spec_dir` (relative path, default `"docs/specs"`); `test_command` (string, default empty); `test_timeout_secs` (default 1800). A workspace value overrides the user value key by key | An unknown `spec_first` value, an absolute or `..` `spec_dir`, or a wrong type is a config warning and the default is used. `test_command` from an untrusted workspace config is ignored with a warning (it runs a shell command) | TC-3500 |
| FR-3501 | Must | Given any plan, stage map or role list | Then `spec` is a node role: `parse_role("spec")` works, it displays as `spec`, its turn policy is write, its default model is the implement default, and `spec` is a known stage for `[stages.spec] skills` | Old plans without `spec` still load | TC-3501 |
| FR-3502 | Must | Given `spec_first` is not `off` | Then `PLAN_PROMPT` and `REPLAN_PROMPT` list `spec` among the allowed roles and state the rule: a plan that changes code starts with a `spec` step that writes or updates the spec under `<spec_dir>`, every implement/draft step depends on it, and a `test` step depends on every implement/draft step. Role order is otherwise free (FR-953) | With `off` the prompts are the old text (snapshot unchanged) | TC-3502 |
| FR-3503 | Must | Given a validated plan with at least one write step, from the planner, a plan edit, a fallback, `--approve-plan-id` or a dynamic-node add, and `spec_first` is not `off` | Then dmuxai makes it covered and (with `require_tests`) tested without another agent turn: if a write step is not covered it inserts `spec-1` (role `spec`, runner and model of the first write step, task of the first write step, job `Write or update the spec under <spec_dir> for: <user prompt>`) and adds it to `depends_on` of every uncovered root write step; if the plan is not tested it inserts `test-1` (role `test`, same runner/model/task, job `Run the tests for: <user prompt>`) depending on every write step that has no write descendant. Ids use the next free `spec-N` / `test-N`. The result is deterministic and idempotent, and it runs before the plan hash, so an exit-7 hash still matches on `--approve-plan-id` | A plan with no write step is unchanged. Inserted steps may take the plan up to `max_nodes + 2`. The approval panel and dashboard show inserted steps with the note `added by spec-first` | TC-3503 |
| FR-3504 | Must | Given a replan revision of a running plan | Then FR-3503 is applied to the merged plan: completed or running spec/test steps count as present, and a revision that removes the last pending test step gets `test-N` back | Immutable steps are never changed | TC-3504 |
| FR-3505 | Must | Given `spec_first = "full"` and a write run on the simple route (judge simple, trivial short prompt that the heuristic calls simple, heuristic fallback simple), the 1-node fallback plan, or `/direct` | Then the run is a graph of three steps on the selected agent and its effective model: `spec-1` (spec) → `step-1` (implement, job = the prompt verbatim) → `test-1` (test), executed without an approval step like FR-937. The status line reads `Graph 1/3 · spec-1 spec · …` | Model policy is checked once before any record is created, as in FR-937 | TC-3505 |
| FR-3506 | Must | Given headless `dmuxai run --agent <runner>` (or MCP `run` with `agent`) without `--graph`, on a write run | Then dmuxai runs the FR-3505 graph (or the FR-3511 lite node) instead of a plain agent turn. `--approve-plan` still confirms sensitive gates; JSON output has `graph_id`, `ok` is true only when all three steps completed, and `text` is the last step's reply | `--read-only` and exempt prompts keep the plain turn | TC-3506 |
| FR-3507 | Must | Given a read-only turn (`--read-only`, ask mode, board ask, MCP read-only), a question (FR-1640), a trivial prompt (FR-1613), a read-only fan-out, `spec_first = "off"`, or `--no-spec-first` | Then no spec or test step is added and the run behaves as before this spec | A trivial prompt the heuristic calls complex still goes to the judge (FR-1613) and then follows FR-3503 | TC-3507 |
| FR-3508 | Must | Given a `spec` step starts in a git workspace | Then dmuxai records the worktree state; when the step reports success it compares the state: at least one changed path must be under `<spec_dir>` and every changed path must be under `<spec_dir>` (ignoring `.dmuxai/`). Otherwise the step fails with `spec step did not write a spec under <spec_dir>` or `spec step changed files outside <spec_dir>: <paths>` and goes to the normal retry/replan path. `spec_dir` is created if missing before the step runs | In a non-git workspace the check is skipped and the run records `spec gate skipped (not a git workspace)` | TC-3508 |
| FR-3509 | Must | Given a `test` step reports success and `test_command` is set | Then dmuxai runs `sh -c <test_command>` in the workspace with the run's environment, bounded by `test_timeout_secs`, and records `test_gate` on the node in `.dmuxai/graphs/<id>.json` and in the run journal: `source: "command"`, `command`, `exit`, `duration_ms` and the last 8 KB of output with secrets masked. A non-zero exit or timeout fails the step with `test_command failed (exit N)` / `test_command timed out after Ns` | The progress stream shows `test gate: running <command>` with heartbeats; cancel kills the command's process group | TC-3509 |
| FR-3510 | Must | Given a `test` step reports success, `test_command` is empty and `require_tests = true` | Then the reply must contain a line `DMUXAI_TESTS: {"command": "...", "exit": 0, ...}`; dmuxai records it as `test_gate` with `source: "marker"`. A missing marker, unparsable JSON or a non-zero `exit` fails the step with `test step did not report a passing test run (DMUXAI_TESTS)` | With `require_tests = false` the test step is not gated | TC-3510 |
| FR-3511 | Must | Given `spec_first = "lite"` and a write run that FR-3505 or FR-3506 would cover | Then the run is one `implement` node whose prompt starts with the spec-first directive (write or update the spec under `<spec_dir>` first, then implement, then run the tests), and after it succeeds the FR-3508 check (at least one change under `<spec_dir>`, other changes allowed) and the FR-3509/FR-3510 test gate run on that node | Planner plans still follow FR-3503 in `lite` | TC-3511 |
| FR-3512 | Must | Given a spec or test step is sent | Then the spec step prompt says to write or update a spec under `<spec_dir>` with FR and TC rows and to change no file outside it; implement steps that depend on a spec step are told to follow that spec; the test step prompt says either that dmuxai will run `<test_command>` afterwards or that the reply must end with the `DMUXAI_TESTS:` line | Prompts keep the existing DATA-block rules | TC-3512 |
| FR-3513 | Must | Given `dmuxai run` or `dmuxai todo run` | Then `--no-spec-first` sets `spec_first = "off"` for that run only; MCP `run` accepts `no_spec_first: true` with the same effect; `dmuxai run --help` lists the flag | The flag never changes config | TC-3513 |
| FR-3514 | Should | Given `[stages.spec] skills` or `[stages.test] skills` | Then those skills are injected into the spec and test steps of FR-3505 graphs like other multi-step nodes (FR-1514), and preflight checks them before the first step | — | TC-3514 |
| NFR-3500 | Must | Given an 8-step plan | Then FR-3503 runs in under 1 ms and adds no judge or planner call; dmuxai's own overhead per agent turn stays within 10 % of a direct agent run (#301), measured with a fake runner. Agent time of the added steps is by design and reported separately | — | TC-3515 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3500 | FR-3500 | unit | TOML fixtures | Parse `[workflow]` defaults, each value, invalid values, untrusted workspace `test_command` | Defaults full/true/docs/specs/""/1800; warnings for invalid; untrusted command ignored | `src/settings.rs` `tc_3500_*` |
| TC-3501 | FR-3501 | unit | — | `parse_role`, `Display`, turn policy, `KNOWN_STAGES` | `spec` round-trips, write policy, known stage | `src/graph/types.rs` / `src/stage_skills.rs` `tc_3501_*` |
| TC-3502 | FR-3502 | unit | `spec_first` full and off | Build plan and replan prompts | Full contains `spec` role and the rule; off equals the old snapshot | `src/graph/plan_doc.rs` `tc_3502_*` |
| TC-3503 | FR-3503 | unit | Plans: research only; implement only; implement+test; spec+implement; ids `spec-1` taken; two parallel implement roots | Apply spec-first | Expected nodes and edges; second application is a no-op; read-only plan unchanged; hash stable | `src/graph/spec_first.rs` `tc_3503_*` |
| TC-3504 | FR-3504 | unit | Running plan with completed spec, pending test | Revision drops the test step | `test-N` re-inserted; immutable steps untouched | `src/graph/replan.rs` `tc_3504_*` |
| TC-3505 | FR-3505 | unit | App with fake runner, spec_first full | Simple verdict, trivial write prompt routed simple, fallback simple, `/direct` | Graph has `spec-1 → step-1 → test-1` on the selected agent, no approval | `src/app.rs` `tc_3505_*` |
| TC-3506 | FR-3506 | integration | Fake runner binary, git repo | `dmuxai run --agent <fake> --approve-plan --json` with a write prompt | Three turns in order, `graph_id` set, `ok` true, `text` is the test step reply | `tests/spec_first_headless.rs` `tc_3506_*` |
| TC-3507 | FR-3507 | unit | Each exempt case | Submit | One plain turn or 1-node run; no spec/test steps | `src/app.rs` `tc_3507_*` |
| TC-3508 | FR-3508 | unit | Temp git repo | Spec step changes only `docs/specs/x.md`; changes `src/a.rs`; changes nothing; non-git dir | ok; fail outside; fail none; skipped note | `src/graph/spec_first.rs` `tc_3508_*` |
| TC-3509 | FR-3509 | unit | Temp workspace, `test_command` `exit 0` / `exit 3` / sleep past timeout | Test step succeeds | ok recorded; fail exit 3; timeout fail; `test_gate` in graph file, output masked and capped | `src/graph/spec_first.rs` `tc_3509_*` |
| TC-3510 | FR-3510 | unit | Empty `test_command` | Replies with passing marker, failing marker, no marker; `require_tests=false` | ok; fail; fail; not gated | `src/graph/spec_first.rs` `tc_3510_*` |
| TC-3511 | FR-3511 | unit | spec_first lite | Simple write run | One node with directive prefix; spec-change check and test gate applied | `src/app.rs` `tc_3511_*` |
| TC-3512 | FR-3512 | unit | — | Build node prompts for spec, implement-after-spec, test (command / marker) | Expected sentences present | `src/graph/spec_first.rs` `tc_3512_*` |
| TC-3513 | FR-3513 | unit | — | Parse `run --no-spec-first`, `todo run --no-spec-first`, MCP `no_spec_first` | Flag set; help lists it | `src/cli.rs` / `src/mcp_server.rs` `tc_3513_*` |
| TC-3514 | FR-3514 | unit | `[stages.spec] skills = ["x"]` | FR-3505 graph | Spec step prompt contains the skill block | `src/app.rs` `tc_3514_*` |
| TC-3515 | NFR-3500 | unit | 8-step plan; fake runner | Time 10 000 applications; compare dmuxai overhead with and without spec-first per turn | < 1 ms per application; overhead ratio ≤ 1.10 | `src/graph/spec_first.rs` `tc_3515_*` |

## 6. Decisions (Dongjin 2026-11-27)

- Spec first in the dmuxai workflow (t788u).
- `full` is the default; simple and `--agent` runs become `spec → implement → test`.
- Missing spec/test steps are auto-inserted.
- 2026-11-28 (t793u): the spec and the plan both need a human check shown on the board; see SPEC-spec-review-gate.
- dmuxai's own orchestration config sets `test_command = "scripts/gate.sh"` (user config, so no workspace trust is needed). `test_command` is a restricted key: a workspace value applies only to a trusted workspace.
