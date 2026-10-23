# SPEC-graph-plan-header

Status: Implemented · Dogfood: #22

## 1. Decision summary
The `Graph assignment:` line printed `node.model_id`, which the planner always sets to composer-2.5, even for nodes that run on AGY or a custom runner. The line now shows each node's runner and effective model, using the same resolution as the status line (`effective_model_for`).

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-460 | Must | `GraphSession::assignment_line_with(resolve)` renders `<id>:<role>→<runner>:<model>`. `resolve(runner, planned)` returns the effective model. |
| FR-461 | Must | The app uses the resolved line everywhere `graph_assignment_line` is set. For non-Cursor runners with no F2 pick it shows `provider default`, never composer-2.5. |
| FR-462 | Must | Cursor nodes still show composer-2.5 (policy pin). |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-460 | FR-460 | `assignment_line_uses_resolver_per_runner` |
| TC-461 | FR-461 | `graph_assignment_shows_agy_model_not_composer` |
| TC-462 | FR-462 | `graph_assignment_cursor_stays_composer` |

## 6. Exit criteria
TCs pass in CI. Dogfood #22 is closed.
