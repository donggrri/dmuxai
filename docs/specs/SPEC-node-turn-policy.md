# SPEC-node-turn-policy

Status: In progress · Series: orchestrator gap plan 1/5 (then fan-out, judge, worktree fan-out, dynamic nodes)

## 1. Decision summary
Graph nodes today all use the App-wide `pending_turn_policy`, so a planning or research node can write files if its runner allows it. This adds an opt-in `[graph] enforce_read_only_roles` setting (default `false`). When on, each graph node's turn policy is derived from its role: `research`, `explore`, `plan`, `route`, and `review` nodes run `TurnPolicy::ReadOnly`; `implement`, `draft`, and `test` nodes run `TurnPolicy::Agent`. With the setting off, behavior and cost are unchanged.

## 2. Scope

In scope: role-to-policy mapping, per-node policy applied at node start and restored after the node, the setting, `/graph` status line, runner fallback when a runner has no read-only mode, a workspace write guard for read-only nodes.
Out of scope: changing the policy of single-agent turns, changing roles chosen by the planner, new runner modes.

## 3. Terms and state model

- Node policy: the `TurnPolicy` a single graph node turn runs with. Derived from `NodeRole` only when the setting is on.
- Read-only role: one of `research`, `explore`, `plan`, `route`, `review`.
- Write role: `implement`, `draft`, `test`. `orchestrate` nodes run no turn.

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-2900 | Must | `[graph] enforce_read_only_roles` (bool, default `false`) exists in user and workspace config and merges like other `[graph]` keys. |
| FR-2901 | Must | With the setting on, a graph node whose role is a read-only role starts its runner turn with `TurnPolicy::ReadOnly`; a write role starts with `TurnPolicy::Agent`. |
| FR-2902 | Must | With the setting off, every graph node starts with the same policy as before this change (the App-wide pending policy). The role mapping function is not consulted on the node start path. |
| FR-2903 | Must | A node policy never leaks: after a node finishes, fails, or is cancelled, the App-wide `pending_turn_policy` equals its value from before the node started. |
| FR-2904 | Must | If the node's runner has no read-only mode, the existing runner fallback applies (`FallbackCause::NoReadOnlyMode`). If no allowed runner can run read-only, the node fails with a clear message instead of running with write access. |
| FR-2905 | Should | When the setting is on, the headless worktree fingerprint guard (SPEC-headless-read-only-write-guard) also covers read-only graph nodes: a read-only node that changes files outside `.dmuxai/` fails that node with `error_kind: policy`. |
| UX-2900 | Should | `/graph` status shows `read-only roles: on` or `off`. |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-2900 | FR-2900 | `tc_2900_enforce_read_only_roles_default_off_and_workspace_overrides_user` |
| TC-2901 | FR-2901 | `tc_2901_role_policy_mapping_read_only_vs_write_roles` |
| TC-2902 | FR-2901 | `tc_2902_start_graph_node_uses_read_only_policy_for_research_node_when_on` |
| TC-2903 | FR-2902 | `tc_2903_setting_off_keeps_app_wide_policy_for_all_roles` |
| TC-2904 | FR-2903 | `tc_2904_pending_policy_restored_after_node_finish_fail_cancel` |
| TC-2905 | FR-2904 | `tc_2905_read_only_node_without_read_only_runner_falls_back_or_fails` |
| TC-2906 | FR-2905 | `tc_2906_read_only_graph_node_that_writes_files_fails_with_policy_error` |
| TC-2907 | UX-2900 | `tc_2907_graph_status_shows_read_only_roles_state` |

## 6. Data and safety contracts
No schema change. No change to plan JSON. Free runners and company workspaces: no change in this PR. Performance: when the setting is off the added cost is one boolean check per node start (NFR: within the 10% bench budget of #301).

## 7. Exit criteria
All TCs pass, gate and web tests pass, `scripts/bench-term.sh` or the existing bench shows no more than 10% regression versus main.
