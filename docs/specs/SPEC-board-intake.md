# SPEC: Board intake from a prompt (planner auto plan)

Status: implemented
Owner: Dongjin
Created: 2026-11-28
Baseline: `068b920`
Related: #375, #374 (out of scope), SPEC-decision-board FR-3105 FR-3106 FR-3132, SPEC-graph-plan-approval-269, SPEC-plan-gate, SPEC-spec-first-workflow FR-3503

## 1. Decision summary

To put work on the board today a bot must run `run --graph`, read `plan_hash`
from the exit-7 JSON and then call `board add --plan-hash`. When the planner is
unavailable the heuristic fallback plan exits 7 without a hash, so the item can
never be approved on the board (#375). This spec lets a manager bot hand the
coordinator a task in one call (`board add --prompt`): dmuxai creates the item,
runs the planner in the item's workspace, and attaches the plan for the owner's
review. Fallback plans get a pending file and hash like planner plans, and the
JSON says the plan came from the fallback.

## 2. Scope

### In scope

- `run --graph --plan-only` (always plan, never execute).
- `board add --prompt T|-` (MCP `board_add` `prompt`, HTTP `POST /api/board` with `prompt`), planning status on the item.
- Pending file + `plan_hash` for fallback plans in headless `run --graph`, with `plan_source` and `planner_error` fields (#375).

### Out of scope / non-goals

- Spec-first steps on board-edited plans (#374; handled by the spec review gate work).
- Automatic approval of anything (approval stays human-only).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3860 | A fallback plan is attachable but the item shows a warning "planner unavailable — fallback plan"; rules may still put it in `needs_decision` | Dongjin | Answered 2026-11-28 |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| planning status | `item.planning`: `{state: "planning"|"planned"|"failed", started_ms, ended_ms?, plan_source?, error?}` | Item lane |
| plan source | `planner` or `fallback` | Triage source |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3860 | Must | Given headless `run --graph` ends at the approval gate with a fallback plan (`triage.source = "fallback"` or the planner failed) | Then the plan is written to `graph-pending/<hash>.json` exactly like a planner plan (after spec-first insertion, FR-3503) and the JSON has `plan_hash`, `plan_source: "fallback"` and `planner_error` (masked, ≤ 500 chars); planner plans get `plan_source: "planner"` | `run --graph --approve-plan-id <hash>` works for that hash | TC-3860 |
| FR-3861 | Must | Given `board add --title T --kind K -C <ws> --prompt P|-` (MCP `board_add` with `prompt`, HTTP `POST /api/board` `{title, kind, workspace, prompt}` on `--web`) | Then the item is created (rules pick the lane, non-owner `auto`/`approved`/`rejected` refused as FR-3105), `planning.state = planning` is set, and `dmuxai -C <ws> run --graph --plan-only --json -` (FR-3866) with P on stdin runs (detached for MCP/HTTP and with `--detach`; blocking otherwise, limit 900 s); on exit 7 with a `plan_hash` the plan is attached (FR-3106 validation, plan copy FR-3130), `planning.state = planned`, `plan_source` recorded, and a `planner` thread message names the step count | any other exit → `failed` with masked error; workspace missing → exit 2 before creating the item; `--prompt` with `--plan-hash` → exit 2 | TC-3861 |
| FR-3862 | Must | Given a fallback plan attached via FR-3861 | Then the item shows a `planner unavailable — fallback plan` warning in `board show`, the modal and `board pending`; owner approval works as for any plan | — | TC-3862 |
| FR-3863 | Must | Given `--key K` with `--prompt` | Then an existing item with the same key and bot is returned and no second planner run starts | — | TC-3863 |
| FR-3864 | Must | Given the intake planner child | Then it runs without `--approve-plan`, `--allow-sensitive` or `--trust-workspace-config`, inherits the environment except `DMUXAI_WEB_TOKEN`, has `DMUXAI_RUN_ID` set, and at most one planning run per item is active (second → conflict exit 4 / 409) | — | TC-3864 |
| FR-3865 | Must | Given the dashboard Board view in `--web` | Then a "New task" button opens a small form (title, kind, workspace = current, prompt) that calls FR-3861 and shows `Planning…` on the card until `planning.state` changes (SSE) | Hidden in `serve` | TC-3865 |
| FR-3866 | Must | Given headless `run --graph --plan-only` (MCP `run` `plan_only: true`) | Then the run always plans (as `/plan`: no judge shortcut to the simple route, no trivial-prompt shortcut), never executes any step, and ends with exit 7 and `plan_hash` (planner or fallback plan, FR-3860); the run manifest is `awaiting_approval` | `--plan-only` with `--approve-plan`, `--approve-plan-id`, `--agent` or `--agents` → exit 2 | TC-3866 |
| NFR-3860 | Must | `run --graph` | Writing the fallback pending file adds no agent turn and no measurable latency | — | TC-3860 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3860 | FR-3860, NFR-3860 | integration | Planner runner that fails fast; complex write prompt | `run --graph --json -` | exit 7 with plan_hash, plan_source fallback, pending file exists; `--approve-plan-id` accepts it | `tests/headless_fallback_plan.rs` `tc_3860_fallback_plan_has_hash` |
| TC-3861 | FR-3861 | unit + cli | Fake `dmuxai` child printing exit-7 JSON with a pending file | `board add --prompt` blocking and detached; error variants | Plan attached, planning states; errors as specified | `src/board/runner.rs` `tc_3861_board_add_prompt_plans` |
| TC-3866 | FR-3866 | integration | Fake planner runner; a trivial prompt `hi` and a complex prompt | `run --graph --plan-only --json -` | Both exit 7 with plan_hash, no step executed (no file changes); flag conflicts → exit 2 | `tests/headless_fallback_plan.rs` `tc_3866_plan_only_never_executes` |
| TC-3862 | FR-3862 | unit | Fallback plan JSON | add --prompt; show; pending | Warning present | `src/board/store.rs` `tc_3862_fallback_warning` |
| TC-3863 | FR-3863 | unit | Existing keyed item | add --prompt with same key | Same item, no child started | `src/board/runner.rs` `tc_3863_key_idempotent_no_second_plan` |
| TC-3864 | FR-3864 | unit | Fake child recording argv/env | add --prompt; second concurrent | No forbidden flags; run id set; token removed; conflict | `src/board/runner.rs` `tc_3864_intake_child_policy` |
| TC-3865 | FR-3865 | unit (asset) + e2e (browser, ignored) | `--web` dashboard | New task form submit | Card shows Planning…, then plan | `src/web/assets.rs` `tc_3865_board_new_task_form`, `tests/web_board_e2e.rs` `tc_3865_*` |
