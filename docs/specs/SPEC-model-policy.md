# SPEC: Model policy (composer-2.5) and per-runner model selection

Status: Implemented (PR #2, #4, #6); some TCs proposed  
Owner: DmuxAI  
Created: 2026-10-17  
Baseline: `73e3f4d`  
Related: MISSION §7, SPEC-agent-graph-orchestration, dogfood #3

## 1. Decision summary

Repository graph definitions, subagents and DmuxAI routing defaults use only `composer-2.5`. At runtime a model the user picked with F2 for an agent always wins. The composer-2.5 graph default applies only to the Cursor runner; AGY accepts `gemini-*-flash-*`, opencode accepts `opencode/*-free`; other runners use their provider default.

## 2. Scope

### In scope

- `src/model_policy.rs`
- `App::effective_model_for` / `apply_runner_and_model`
- `tests/model_policy_repo.rs` repo scans
- `/graph override` validation

### Out of scope / non-goals

- Model pricing / quota

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| Q-1 | Proposed TCs (marked *proposed*) have no code yet and must be implemented before this spec is Approved. | DmuxAI | Release gate |

## 3. Terms and state model

- F2 selection: per-agent model chosen in the model picker.
- Runner-aware policy: allowed model pattern per runner.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-310 | Must | Repo graph/agent definitions and MISSION are scanned. | Only composer-2.5 appears as a model. | Any haiku/grok/sonnet/gpt/gemini/`*-fast` model fails the test. | TC-310 |
| FR-311 | Must | Model id validated for graph roles. | Only `composer-2.5` accepted. | Ids containing `fast` or foreign models rejected. | TC-311 |
| FR-312 | Must | User picked a model with F2 for the selected agent; planner proposes composer-2.5. | Request uses the F2 model. | — | TC-312 |
| FR-313 | Must | No F2 selection; graph default composer-2.5. | Applied only when the runner is Cursor; AGY/others use provider default. | — | TC-313 |
| FR-314 | Must | `/graph override` or runtime validation per runner. | AGY: gemini flash only; opencode: `*-free` only; Cursor: composer-2.5 only. | Disallowed model rejected with a message. | TC-314, TC-315 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-310 | FR-310 | integration | repository files | scan | composer-only | `cargo test graph_agent_definitions_use_composer_only`; `dmuxai_graph_yaml_declares_composer_only_policy`; `mission_doc_states_composer_policy` |
| TC-311 | FR-311 | unit | model ids | validate | only composer-2.5 passes | `cargo test only_composer_2_5_is_allowed`; `validate_rejects_fast_and_foreign_models`; `validate_rejects_fast_and_non_composer_models` |
| TC-312 | FR-312 | unit | AGY + F2 Gemini Flash | submit | AGY request model = F2 model | `cargo test user_selected_agy_model_wins_over_graph_default` |
| TC-313 | FR-313 | unit | no F2 selection | submit to Cursor / AGY | composer-2.5 only on Cursor | `cargo test graph_default_model_only_applies_to_cursor` |
| TC-314 | FR-314 | unit | runner-aware policy | validate per runner | AGY flash allowed only on AGY | `cargo test runner_aware_policy_allows_agy_flash_only_on_agy` |
| TC-315 | FR-314 | unit | opencode runner; `/graph override` | validate free/non-free model | free allowed, others rejected | *proposed*: `opencode_policy_allows_free_models_only`, `graph_override_validates_runner_policy` |

## 6. Data, integration, and safety contracts

- Model ids are passed to CLIs as argv, never interpolated into a shell.

## 7. Implementation plan

See status above; proposed tests are tracked in [TEST-PLAN.md](../TEST-PLAN.md).

## 8. Review and test exit criteria

- All non-proposed TCs pass; TC-315 implemented when opencode runner lands.
