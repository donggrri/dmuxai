# SPEC: Agent graph orchestration (default)

Status: draft  
Baseline: image-attachment PR  
Related: MISSION.md, agent-graph-engineering skill

## Decision summary

DmuxAI plans multi-step work using the agent-graph-engineering shape by default. A grok-4.7-medium orchestrator step emits a graph.yaml-style plan with per-node runner and model assignments. Trivial prompts skip the graph and run one agent. The UI shows the plan and accepts overrides. Sensitive steps pause for confirmation. Graph runs persist under `.dmuxai/graphs/` for resume.

## Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-GRAPH-1 | Must | Orchestrator model is `grok-4.7-medium` on graph plans | `planner_sets_orchestrator_model` |
| FR-GRAPH-2 | Must | Role buckets map to models (research/explore/test → haiku; implement/draft → composer-2.5; plan/route/review → grok) | `model_for_role_bucket` |
| FR-GRAPH-3 | Must | Reject any model id containing `fast` | `validate_model_id` |
| FR-GRAPH-4 | Must | ≤2-step estimates use single-agent path | `simple_prompt_uses_single_agent` |
| UX-GRAPH-1 | Must | UI shows assignment summary; user can override via `/graph` | `graph_assignment_rendered` |
| FR-GRAPH-5 | Must | Gates pause on merge/DB/auth-class steps until `/graph continue` | `gate_pauses_until_continue` |
| NFR-GRAPH-1 | Must | Cap parallel nodes (default 2) and retries (default 2) | `parallel_cap_respected` |
| FR-GRAPH-6 | Must | Persist and reload graph run JSON | `store_round_trip` |
| FR-GRAPH-7 | Must | Discover `.agents/skills/*/SKILL.md` for node skill binding | `discover_skills` |

## Non-goals

- Live calls to external orchestrator APIs outside existing CLIs
- tmux multiplexer panes (logical parallel only)
