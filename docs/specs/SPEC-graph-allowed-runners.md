# SPEC-graph-allowed-runners

Status: Implemented (PR #19) · Dogfood: #20

## Decision summary
Graph orchestration is on by default, so a machine needs a way to keep graph nodes away from particular runners. One example is the box's company-account cursor-agent. This adds `[graph] allowed_runners` and a user-level config file. The user file is read first, and the workspace `dmuxai.toml` then overrides it key by key.

## Scope
In scope: allowlist for graph node runners, user-level config loading and merging, `/graph` status display, `/graph override` validation.
Out of scope: restricting the runner a user picks by hand for a single-agent turn.

## Terms / state
- `graph_allowed_runners: Option<Vec<String>>`. `None` means every runner is allowed. `Some([])` means no runner is allowed for graph nodes.
- User config: `$XDG_CONFIG_HOME/dmuxai/dmuxai.toml`, falling back to `~/.config/dmuxai/dmuxai.toml`.

## Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-430 | Must | `[graph] allowed_runners` limits which runners may be assigned to graph nodes. Matching is case-insensitive. Unknown names are dropped and a warning is shown. |
| FR-431 | Must | Assignment order: `routing.default_agent` if it is allowed, otherwise `cursor` if it is allowed, otherwise the first allowed runner. |
| FR-432 | Must | An empty list sends the turn to the single-agent path and shows a notice. |
| FR-433 | Must | The user-level config is loaded first, and workspace keys override it. If the user file is invalid, it is ignored and a warning names the file. |
| UX-430 | Should | `/graph` status lists the allowed runners. |
| INT-430 | Must | `/graph override` rejects a runner that is not allowed. |

## Acceptance
| TC | Covers | Test |
|---|---|---|
| TC-430 | FR-430, FR-431 | `assign_runner_honors_allowed_runners_and_order` |
| TC-431 | FR-431 | `routing_default_not_allowed_falls_back` |
| TC-432 | FR-432 | `empty_allowed_runners_planner_falls_back_to_single`, `empty_allowed_runners_falls_back_to_single_agent_with_notice` |
| TC-433 | FR-433 | `user_config_merged_and_overridden_by_workspace` |
| TC-434 | UX-430 | `graph_status_shows_allowed_runners` |
| TC-435 | INT-430 | `graph_override_rejects_disallowed_runner` |

## Data / safety
Box default: `~/.config/dmuxai/dmuxai.toml` sets `[graph] allowed_runners = ["agy"]`, so graph work never reaches the company-account cursor-agent.

## Exit criteria
All TCs pass in CI, and dogfood #20 is closed.
