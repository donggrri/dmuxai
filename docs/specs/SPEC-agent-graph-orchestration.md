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
| FR-1880 | Must | Same as FR-GRAPH-1 (Orchestrator model is `composer-2.5` on graph plans) |
| FR-1881 | Must | Same as FR-GRAPH-2 (All role buckets use `composer-2.5` only) |
| FR-1882 | Must | Same as FR-GRAPH-3 (Reject any model id containing `fast` or not equal to composer-2.5) |
| FR-1883 | Must | Same as FR-GRAPH-4 (≤2-step estimates use single-agent path) |
| UX-1884 | Must | Same as UX-GRAPH-1 (UI shows assignment summary; user can override via `/graph`) |
| FR-1885 | Must | Same as FR-GRAPH-5 (Gates pause on merge/DB/auth-class steps until `/graph continue`) |
| NFR-1886 | Must | Same as NFR-GRAPH-1 (Cap parallel nodes (default 2) and retries (default 2)) |
| FR-1887 | Must | Same as FR-GRAPH-6 (Persist and reload graph run JSON) |
| FR-1888 | Must | Same as FR-GRAPH-7 (Discover `.agents/skills/*/SKILL.md` for node skill binding) |

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
| TC-1880 | FR-1880 | unit | planned — no automated test yet |
| TC-1881 | FR-1881 | unit | `tests/graph_orchestration.rs` `validate_rejects_fast_and_non_composer_models` |
| TC-1882 | FR-1882 | unit | `src/model_policy.rs` `validate_model_id` |
| TC-1883 | FR-1883 | unit | planned — no automated test yet |
| TC-1884 | UX-1884 | manual | manual / integration |
| TC-1885 | FR-1885 | unit | `tests/graph_orchestration.rs` `gate_pauses_until_continue` |
| TC-1886 | NFR-1886 | unit | executor defaults (planned — no automated test yet) |
| TC-1887 | FR-1887 | unit | `tests/graph_orchestration.rs` `store_round_trip` |
| TC-1888 | FR-1888 | unit | `tests/graph_orchestration.rs` `discover_skills_finds_agent_graph_engineering` |

## 6. Non-goals

- Live calls to external orchestrator APIs outside existing CLIs
- tmux multiplexer panes (logical parallel only)
