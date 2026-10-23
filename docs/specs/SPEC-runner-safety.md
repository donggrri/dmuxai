# SPEC: Runner safety (questions, resume, pinned runners)

Status: approved  
Owner: dmuxai  
Baseline: `main`

## Problem

Agy implement turns used full edit permissions on simple questions (#112). `/resume` and conversation open could select a runner outside `[graph] allowed_runners` (#101). Dashboard `POST /api/prompt` with `mode: ask` and an explicit agent still entered triage and planned multi-runner graphs (#106).

## 4. Requirements

| ID | Priority | Given | Then | Edge cases | Tests |
| --- | --- | --- | --- | --- | --- |
| FR-1640 | Must | Triage says `simple` and `is_question_prompt` is true | The agy turn uses `TurnPolicy::ReadOnly` (`--mode plan --sandbox`, no `--dangerously-skip-permissions`) | Non-question simple implement turns unchanged (FR-1642) | TC-1640 |
| FR-1641 | Must | Same as FR-1640 | The adapter prompt is prefixed with the question hint | Other runners on the same simple path get the hint | TC-1641 |
| FR-1642 | Must | Simple non-question implement turn on agy | Still uses agent/implement flags as today | — | TC-1642 |
| FR-1643 | Must | `/resume`, dashboard `load_conversation`, or startup restore of a conversation whose last runner is not in `allowed_runners` | Select the first allowed runner and emit `Runner '<x>' is not in allowed_runners; using '<y>'` | `allowed_runners` unset leaves runner as restored | TC-1643, TC-1647, TC-1648 |
| FR-1644 | Must | User prompt with read-only mode (`ask` / `plan`, dashboard or TUI) | No judge or planner graph: one direct read-only turn on the selected runner | Slash commands unchanged | TC-1644 |
| FR-1645 | Must | User picked a runner (API `agent`, TUI/dashboard selected agent for implement/plan, not explicit graph) | Every planned node uses that runner and its effective model | `RunMode::Graph`, headless `--graph`, and todo graph mode may use multiple allowed runners | TC-1645 |
| FR-1646 | Must | A planned node names another runner or a disallowed runner while pinned | Rewrite to the pinned runner; log a plan note | — | TC-1646 |
| FR-1647 | Must | App startup restores the latest conversation | FR-1643 applies to the restored runner | — | TC-1647 |
| FR-1648 | Must | Dashboard opens a conversation | FR-1643 applies | — | TC-1648 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement | Command |
| --- | --- | --- |
| TC-1640 | FR-1640 | `cargo test tc_1640_1641_agy_simple_question_is_read_only_with_hint` |
| TC-1641 | FR-1641 | `cargo test tc_1640_1641_agy_simple_question_is_read_only_with_hint` |
| TC-1642 | FR-1642 | `cargo test tc_1642_non_question_implement_unchanged` |
| TC-1643 | FR-1643 | `cargo test tc_1643_resume_honours_allowed_runners` |
| TC-1644 | FR-1644 | `cargo test tc_1644_ask_mode_skips_judge` |
| TC-1645 | FR-1645 | `cargo test tc_1645_pins_planned_runners` |
| TC-1646 | FR-1646 | `cargo test tc_1646_pin_graph_plan_runners_rewrites_disallowed` |
| TC-1647 | FR-1647 | `cargo test tc_1647_startup_restore_honours_allowed_runners` |
| TC-1648 | FR-1648 | `cargo test tc_1648_dashboard_load_honours_allowed_runners` |
