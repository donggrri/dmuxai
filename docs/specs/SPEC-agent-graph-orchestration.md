# SPEC: Agent graph orchestration (default)

Status: draft  
Baseline: image-attachment PR  
Related: MISSION.md, agent-graph-engineering skill  
Superseded in part: keyword role inference (`infer_roles`) becomes the fallback only; the selected agent judges and plans per [SPEC-plan-gate](SPEC-plan-gate.md).

**Status (v0.2.3):** [SPEC-plan-gate](SPEC-plan-gate.md) implements default orchestration in place of this doc’s keyword routing and fixed `composer-2.5` on every node: the selected agent judges and plans; `infer_roles` runs only when the judge fails; per-step runner and model come from the plan JSON and model policy. Sensitive-step gates and `.dmuxai/graphs/` persistence remain as described here.

## 1. Decision summary

DmuxAI plans multi-step work using the agent-graph-engineering shape by default. The orchestrator step emits a graph.yaml-style plan with per-node runner assignments; **every node model is `composer-2.5`**. Trivial prompts skip the graph and run one agent. The UI shows the plan and accepts overrides. Sensitive steps pause for confirmation. Graph runs persist under `.dmuxai/graphs/` for resume.

## 4. Requirements

*Note: Numeric requirement IDs FR-1880..FR-1888 are aliases for legacy non-numeric IDs FR-GRAPH-1..FR-GRAPH-7, UX-GRAPH-1, and NFR-GRAPH-1.*

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-1880 | Retired (superseded by plan gate, #67) | Same as FR-GRAPH-1 (Orchestrator model is `composer-2.5` on graph plans) |
| FR-1881 | Must | Same as FR-GRAPH-2 (All role buckets use `composer-2.5` only) |
| FR-1882 | Must | Same as FR-GRAPH-3 (Reject any model id containing `fast` or not equal to composer-2.5) |
| FR-1883 | Retired (superseded by plan gate, #67) | Same as FR-GRAPH-4 (≤2-step estimates use single-agent path) |
| UX-1884 | Must | Same as UX-GRAPH-1 (UI shows assignment summary; user can override via `/graph`) |
| FR-1885 | Must | Same as FR-GRAPH-5 (Gates pause on merge/DB/auth-class steps until `/graph continue`) |
| NFR-1886 | Must | Same as NFR-GRAPH-1 (Cap parallel nodes (default 2) and retries (default 2)) |
| FR-1887 | Must | Same as FR-GRAPH-6 (Persist and reload graph run JSON) |
| FR-1888 | Must | Same as FR-GRAPH-7 (Discover `.agents/skills/*/SKILL.md` for node skill binding) |
| FR-2421 | Must | Given a graph node with role `test` or `verify`, the system/node prompt instructs the agent to confirm external work was accepted (via job id or queue response) before waiting, bound every wait/poll loop (e.g. 10 minutes), and fail with a clear message instead of waiting indefinitely. |

### Legacy id map

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-GRAPH-1 | Must | Orchestrator model is `composer-2.5` on graph plans | `planner_sets_orchestrator_model` |
| FR-GRAPH-2 | Must | All role buckets use `composer-2.5` only | `validate_rejects_fast_and_non_composer_models` |
| FR-GRAPH-3 | Must | Reject any model id containing `fast` or not equal to composer-2.5 | `validate_model_id` |
| FR-GRAPH-4 | Must | ≤2-step estimates use single-agent path | `simple_prompt_uses_single_agent` |
| UX-GRAPH-1 | Must | UI shows assignment summary; user can override via `/graph` | manual / integration |
| FR-GRAPH-5 | Must | Gates pause on merge/DB/auth-class steps until `/graph continue` | `gate_pauses_until_continue` |
| NFR-GRAPH-1 | Must | Cap parallel nodes (default 2) and retries (default 2) | executor defaults |
| FR-GRAPH-6 | Must | Persist and reload graph run JSON | `store_round_trip` |
| FR-GRAPH-7 | Must | Discover `.agents/skills/*/SKILL.md` for node skill binding | `discover_skills` |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Test |
| --- | --- | --- | --- |
| TC-1881 | FR-1881 | unit | `tests/graph_orchestration.rs` `tc_1881_validate_rejects_fast_and_non_composer_models` |
| TC-1882 | FR-1882 | unit | `src/model_policy.rs` `tc_1882_validate_rejects_fast_and_foreign_models` |
| TC-1884 | UX-1884 | manual | manual / integration |
| TC-1885 | FR-1885 | unit | `tests/graph_orchestration.rs` `tc_1885_gate_pauses_until_continue` |
| TC-1886 | NFR-1886 | unit | `tests/graph_orchestration.rs` `tc_1886_executor_and_plan_limits_defaults` |
| TC-1887 | FR-1887 | unit | `tests/graph_orchestration.rs` `tc_1887_store_round_trip` |
| TC-1888 | FR-1888 | unit | `tests/graph_orchestration.rs` `tc_1888_discover_skills_finds_agent_graph_engineering` |
| TC-2421 | FR-2421 | unit | `src/graph/executor.rs` `tc_2421_test_node_prompt_has_wait_guard` (test-role node prompt has the guard; implement-role prompt does not) |

## 6. Non-goals

- Live calls to external orchestrator APIs outside existing CLIs
- tmux multiplexer panes (logical parallel only)

### Retired test cases

TC-1880 and TC-1883 are retired. Their behavior was the static keyword planner that set the orchestrator model and the single-agent bypass for estimates of 2 steps or fewer. #67 (`a4d1b9b`) replaced both with agent-planned dynamic graphs and judge triage (see SPEC-plan-gate), and removed their tests `planner_sets_orchestrator_model` and `simple_prompt_uses_single_agent`. No code path remains to test.
