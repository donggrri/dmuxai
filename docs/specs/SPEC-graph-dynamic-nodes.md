# SPEC-graph-dynamic-nodes

Status: In progress · Series: orchestrator gap plan 5/5 · Builds on SPEC-node-turn-policy

## 1. Decision summary
Graph plans are fixed once approved; only replanning can change them. This adds opt-in dynamic nodes: a running node may ask for extra nodes with a `DMUXAI_ADD_NODES` marker, but only inside a cap the user approved together with the plan. The cap is part of the plan the user approves, so the user bounds the agent. Without `[graph] dynamic_nodes = true` the marker is ignored and nothing changes.

## 2. Scope

In scope: `[graph] dynamic_nodes` and `dynamic_node_cap` settings, the marker format and parsing, validation, appending nodes to a running plan, recording changes, the approval hash, journal events, free-runner exclusion for company workspaces.
Out of scope: raising the cap during a run, removing or editing existing nodes, dynamic nodes in single-agent runs, TUI and dashboard design changes beyond showing the new nodes.

## 3. Terms and state model

- Cap: maximum number of dynamically added nodes for one run. Stored in the plan (`dynamic_cap`), shown at plan approval, and included in the plan hash.
- Marker: a line `DMUXAI_ADD_NODES: [ {"id","role","job","runner","model_id","depends_on"} ]` in a node's final answer. Parsed like the existing `DMUXAI_REPLAN` marker.

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-2940 | Must | `[graph] dynamic_nodes` (bool, default `false`) and `[graph] dynamic_node_cap` (integer, default `3`, 0 disables) exist in user and workspace config and merge like other `[graph]` keys. |
| FR-2941 | Must | With `dynamic_nodes = false` or cap 0, `DMUXAI_ADD_NODES` markers are ignored, the plan, its hash, and the run are byte-for-byte as before. |
| FR-2942 | Must | With the feature on, the effective cap is written into the plan as `dynamic_cap` before approval, shown in the approval summary, and part of the plan hash; a run that was approved with cap C never adds more than C nodes, even if config changes later. |
| FR-2943 | Must | A marker from a running node is validated: JSON array of objects; ids unique and not existing; role is one of the valid roles and not `orchestrate`; runner allowed by `graph.allowed_runners` and model valid for that runner (`validate_model_id` rules); `depends_on` names only existing nodes or earlier entries in the same marker; no cycles; total nodes within `max_nodes`; the added count stays within the cap. Any violation rejects the whole marker, adds nothing, and records a rejection; the node still completes normally. |
| FR-2944 | Must | Accepted nodes are appended in `pending` state, scheduled by the existing executor, and recorded as a `PlanChange` with trigger `dynamic_add` naming the adding node; they appear in the run journal and graph view like other nodes. Added nodes do not need re-approval because they are inside the approved cap. |
| FR-2945 | Must | With `enforce_read_only_roles` on, added read-only roles run read-only (SPEC-node-turn-policy). Added nodes cannot have `requires_gate` cleared: a node added with a gated role keeps the sensitive gate. |
| FR-2946 | Must | When `[fanout] company_workspace = true`, a marker that names a free runner is rejected as a whole with a warning. |
| FR-2947 | Should | A node may add nodes at most once; a second marker from the same node is rejected. A failed or cancelled node adds nothing. |
| NFR-2940 | Must | With the feature off, marker scanning does not run on the node-finish path; benchmark within 10% of main (#301). |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-2940 | FR-2940 | `tc_2940_dynamic_settings_defaults_and_merge` |
| TC-2941 | FR-2941 | `tc_2941_marker_ignored_when_feature_off` |
| TC-2942 | FR-2942 | `tc_2942_cap_in_plan_hash_and_not_exceeded_after_config_change` |
| TC-2943 | FR-2943 | `tc_2943_marker_validation_rejects_each_violation` |
| TC-2944 | FR-2944 | `tc_2944_accepted_nodes_scheduled_and_recorded_as_plan_change` |
| TC-2945 | FR-2945 | `tc_2945_added_nodes_keep_read_only_policy_and_gate` |
| TC-2946 | FR-2946 | `tc_2946_company_workspace_rejects_free_runner_marker` |
| TC-2947 | FR-2947 | `tc_2947_second_marker_and_failed_node_add_nothing` |
| TC-2948 | NFR-2940 | `tc_2948_no_marker_scan_when_feature_off` |

## 6. Data and safety contracts
Persistence: the stored graph run gains optional `dynamic_cap` and `dynamic_added` (default absent, so old runs load unchanged). Rollback: removing the feature leaves old plans loadable. Security: added nodes never exceed approved bounds; marker text is treated as data and validated, not executed.

## 7. Exit criteria
All TCs pass, gate and web tests pass, benchmark within 10%.
