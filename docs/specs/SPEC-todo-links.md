# SPEC-todo-links

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `b1eb6a4`
Related: refactor scan finding #7, PR-8b

## 1. Decision summary

To-do run links in `src/app.rs` are consolidated into a dedicated `src/todo_link.rs` module. `App` replaces six loose fields with one `TodoLinks` value. All to-do link state and store finishing calls move behind methods returning `TodoFinish`, so shared reporting stays in `App`. Behavior is preserved.

## 2. Scope

### In scope

- `TodoLinks` owning store, turn links, graph link, pending graph link, pending run id, and submitting flag.
- Methods for linking and finishing turns, pending graph commit, graph and linked finishing, shutdown drain, and submitting flag access.
- `App::report_todo_finish` shared reporting helper.

### Out of scope / non-goals

- Changing dashboard to-do CRUD flows beyond state access.
- Changing user-visible strings or publish ordering.
- Moving `TodoStore` implementation.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2020 | Temporary databases follow existing to-do tests. | dmuxai | PR-8b implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| TodoLinks | Module-owned to-do run link state. | TodoStore persistence. |
| TodoFinish | Result of a finishing attempt. | Run success. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2020 | Must | Given a linked turn | finishing it finishes the store row and removes the link; a second finish is `NotLinked` | store errors become `Failed` | TC-2020 |
| FR-2021 | Must | Given a pending graph to-do | `todo_for_new_run` returns it and commit binds the plan id and clears pending | no link when none pending | TC-2021 |
| FR-2022 | Must | Given graph, pending, or last-run to-do ids | `finish_linked` resolves graph, then pending, then last-run and clears all three | `NotLinked` when none | TC-2022 |
| FR-2023 | Must | Given linked turns/graph on shutdown | `finish_all` drains them with the supplied reason and returns store errors | empty when nothing linked | TC-2023 |
| FR-2024 | Must | Given a bound graph to-do for a plan | `finish_graph_for_plan` finishes only when bound to that plan id | `NotLinked` otherwise | TC-2021 |
| FR-2025 | Must | Given a bound graph to-do | `finish_graph` takes and finishes it | `NotLinked` when none | TC-2022 |
| FR-2026 | Must | Given `App` reporting | `Failed` pushes the shared message and publishes when a store exists | `NotLinked` does not publish | TC-2020 |
| FR-2027 | Must | Given shutdown | shutdown message uses the shutdown-specific text | publish only when something changed | TC-2023 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2020 | FR-2020, FR-2026 | unit | temp `TodoStore`, linked turn | finish twice | row finished then `NotLinked` | `src/todo_link.rs` `tc_2020_todo_links_turn_finish` |
| TC-2021 | FR-2021, FR-2024 | unit | temp `TodoStore`, pending graph | commit to a run | pending cleared, plan bound | `src/todo_link.rs` `tc_2021_pending_graph_commit_and_new_run_link` |
| TC-2022 | FR-2022, FR-2025 | unit | temp `TodoStore`, graph/pending/last ids | finish linked | resolution order and clearing | `src/todo_link.rs` `tc_2022_finish_linked_resolution_order` |
| TC-2023 | FR-2023, FR-2027 | unit | temp `TodoStore`, linked turn and graph | finish all | rows opened with reason, state drained | `src/todo_link.rs` `tc_2023_finish_all_on_shutdown` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change | TodoStore unchanged | TC-2020 |
| Provider protocol | No provider change | N/A | TC-2020 |
| Security / permissions | No change | N/A | TC-2020 |
| Platform | No platform change | N/A | TC-2020 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Create `src/todo_link.rs` | `src/todo_link.rs`, `src/main.rs` | FR-2020 | TC-2020 |
| 2 | Refactor `App` | `src/app.rs` | FR-2020..FR-2027 | TC-2020..TC-2023 |
| 3 | Add spec | `docs/specs/SPEC-todo-links.md` | FR-2020..FR-2027 | `python3 scripts/check-spec.py` |

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] Required Rust checks pass.
