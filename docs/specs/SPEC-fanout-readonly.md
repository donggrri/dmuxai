# SPEC-fanout-readonly

Status: In progress · Series: orchestrator gap plan 2/5 · Builds on SPEC-node-turn-policy · Design source: fan-out feasibility review

## 1. Decision summary
There is no way to ask several runners the same question and compare answers. This adds an opt-in headless mode `dmuxai run <prompt> --agents a,b[:model],c` that runs the same prompt on N distinct runners in parallel inside one process, read-only, and returns every answer side by side. A failing leg does not fail the run as long as the quorum (`--min-ok`, default 1) is met. Each leg runs in its own fresh conversation, so no leg sees another leg's answer or the main conversation history (#172). Without `--agents` nothing changes.

## 2. Scope

In scope: `--agents` and `--min-ok` flags on `run`, leg execution, leg isolation, quorum, JSON and text output, runner-fallback exclusion, free-runner exclusion for company workspaces.
Out of scope: write legs (PR 4), aggregation or judge (PR 3), TUI and dashboard UI, running one runner twice with two models.

## 3. Terms and state model

- Leg: one runner turn of the fan-out. States: `queued`, `running`, `ok`, `failed`, `skipped`.
- Quorum: number of `ok` legs required for run success (`--min-ok`, default 1, at most N).
- Company workspace: `[fanout] company_workspace = true` in config (default `false`).
- Free runner: a runner whose resolved model id ends in `-free`, or a custom agent with `free = true`.

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-2910 | Must | `dmuxai run <prompt> --agents a,b[:model],...` runs the prompt on each listed runner concurrently in one process. `--agents` needs at least 2 entries, all distinct runners; duplicates or fewer than 2 exit 2 with a usage error. It conflicts with `--agent`, `--graph`, and `--model` (exit 2). |
| FR-2911 | Must | Every leg runs with `TurnPolicy::ReadOnly`. A runner with no read-only mode makes that leg `failed` with `error_kind` `no_read_only_mode`; it never runs with write access. |
| FR-2912 | Must | Each leg runs in a fresh conversation with an empty history. It receives only the user prompt: no replan hint, no write-step note, no stage skill prefix, no other leg's text, and no earlier conversation turns. |
| FR-2913 | Must | A failed leg (error, timeout, no read-only mode, unknown runner) is recorded and does not cancel other legs. The run succeeds (exit 0, `ok: true`) when at least `--min-ok` legs are `ok`; otherwise exit 1 and `ok: false`. |
| FR-2914 | Must | Runner fallback inside a leg never switches to a runner that is another leg's runner. If the only fallback candidates are in use, the leg fails instead. |
| FR-2915 | Must | `--json` prints `{ "ok", "fanout": { "min_ok", "legs": [ { "agent", "model", "status", "text", "error", "error_kind", "duration_ms" } ] } }`, legs in the order given on the command line. Text mode prints one titled section per leg. |
| FR-2916 | Must | When `[fanout] company_workspace = true`, free runners listed in `--agents` are skipped with a warning on stderr and status `skipped`. If fewer than 2 legs remain the run exits 2. Skipped legs do not count toward the quorum. |
| FR-2917 | Must | The workspace worktree fingerprint taken before and after the fan-out must match outside `.dmuxai/`; a change fails the run with `error_kind` `policy`. |
| NFR-2910 | Must | Runs without `--agents` add no measurable overhead: no new work on the single-agent or graph paths, and the benchmark stays within 10% of main (#301). |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-2910 | FR-2910 | `tc_2910_agents_flag_parse_and_conflicts` |
| TC-2911 | FR-2911 | `tc_2911_leg_without_read_only_mode_fails_not_writes` |
| TC-2912 | FR-2912 | `tc_2912_leg_prompt_and_conversation_are_isolated` |
| TC-2913 | FR-2913 | `tc_2913_failed_leg_tolerated_by_quorum` |
| TC-2914 | FR-2914 | `tc_2914_fallback_never_uses_runner_of_another_leg` |
| TC-2915 | FR-2915 | `tc_2915_json_output_shape_and_leg_order` |
| TC-2916 | FR-2916 | `tc_2916_company_workspace_skips_free_runners_with_warning` |
| TC-2917 | FR-2917 | `tc_2917_fanout_write_in_worktree_is_policy_error` |
| TC-2918 | NFR-2910 | `tc_2918_no_agents_flag_takes_existing_path` |

## 6. Data and safety contracts
No schema change for existing tables. Leg conversations are ordinary conversations titled `fanout: <prompt prefix> [<agent>]`. Concurrency stays inside one process, so the state DB lock (#167) is not an issue. No new network access. Free models never receive company code: FR-2916.

## 7. Exit criteria
All TCs pass, gate and web tests pass, benchmark within 10%.
