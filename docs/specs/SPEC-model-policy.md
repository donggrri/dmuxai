# SPEC: Model policy (composer-2.5) and per-runner model selection

Status: Implemented (PR #2, #4, #6); grok rows draft (SPEC-grok-runner)  
Owner: DmuxAI  
Created: 2026-10-17  
Baseline: `73e3f4d`  
Related: MISSION §7, SPEC-agent-graph-orchestration, SPEC-grok-runner, dogfood #3

## 1. Decision summary

Repository graph definitions, subagents and DmuxAI routing defaults use only `composer-2.5`. At runtime a model the user picked with F2 for an agent always wins. The composer-2.5 graph default applies only to the Cursor runner. AGY accepts `gemini-*-flash-*`, opencode accepts `opencode/*-free`, and the grok runner accepts `grok-*` ids that do not contain `fast`. Any id containing `fast` is rejected for every runner. A `grok-*` id is accepted only for the grok runner. Cursor stays composer-2.5 only. Repo graph files stay composer-2.5 only, and `docs/dmuxai-graph.yaml` keeps `*grok*` in `disallow_patterns`.

## 2. Scope

### In scope

- `src/model_policy.rs` `validate_model_for_runner`
- `App::effective_model_for` / `apply_runner_and_model`
- `tests/model_policy_repo.rs` repo scans
- `/graph override` validation
- The grok amendment: reject `fast` for every runner, allow `grok-*` only on the grok runner, and comment `disallow_patterns` in `docs/dmuxai-graph.yaml` without changing its node models

### Out of scope / non-goals

- Model pricing / quota
- Assigning a `grok-*` model to a node in `docs/dmuxai-graph.yaml` (those nodes stay `composer-2.5`)

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
| FR-910 | Must | `validate_model_for_runner` is called for any runner, including agy, opencode, grok, cursor, codex, and a custom agent. | An id whose trimmed lowercase form contains `fast` is rejected before any runner-specific allowance. The error text contains `fast` and the model id (the existing `fast models are blocked by policy` wording). `gemini-3.8-flash-medium` stays allowed on agy, and `opencode/glm-5.3-flash-free` stays allowed on opencode, because `flash` does not contain `fast`. | `composer-2.5-fast`, `grok-4.7-build-fast`, `gemini-3-flash-fast`, and `opencode/glm-fast-free` are rejected on every runner. | TC-910 |
| FR-911 | Must | `validate_model_for_runner` is called with a trimmed id that matches `(?i)^grok-[a-z0-9][a-z0-9._-]*$` and does not contain `fast`. | The id is accepted only when the runner id is `grok` (ASCII case-insensitive). `grok-4.7` and `grok-4.6` pass for `grok`. Cursor still accepts only `composer-2.5`. AGY flash and opencode free allowances are unchanged aside from FR-910. | The same id is rejected for cursor, agy, codex, opencode, and any other runner, including when that runner's `allowed_models` would match it. `composer-2.5` is rejected for the grok runner even when the custom-agent registry is empty. `grok` with no hyphen suffix is rejected. | TC-911 |
| FR-912 | Must | `docs/dmuxai-graph.yaml` is updated for the grok runner. | Every node `model` stays `composer-2.5`. `allowed_only` stays `composer-2.5`. `disallow_patterns` still contains `*grok*` and `*-fast`. A comment immediately above `disallow_patterns` states that this repo graph stays composer-2.5 on every node, that runtime policy allows `^grok-` models on the grok runner only, and that `*grok*` stays so this file does not assign a grok model. | The comment must not contain the substring `model: grok`. Removing `*grok*` from `disallow_patterns`, or setting a node model to a grok id, fails TC-912 and TC-310. | TC-912 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-310 | FR-310 | integration | repository files | scan | composer-only | `cargo test graph_agent_definitions_use_composer_only`; `dmuxai_graph_yaml_declares_composer_only_policy`; `mission_doc_states_composer_policy` |
| TC-311 | FR-311 | unit | model ids | validate | only composer-2.5 passes | `cargo test only_composer_2_5_is_allowed`; `validate_rejects_fast_and_foreign_models`; `validate_rejects_fast_and_non_composer_models` |
| TC-312 | FR-312 | unit | AGY + F2 Gemini Flash | submit | AGY request model = F2 model | `cargo test user_selected_agy_model_wins_over_graph_default` |
| TC-313 | FR-313 | unit | no F2 selection | submit to Cursor / AGY | composer-2.5 only on Cursor | `cargo test graph_default_model_only_applies_to_cursor` |
| TC-314 | FR-314 | unit | runner-aware policy | validate per runner | AGY flash allowed only on AGY | `cargo test runner_aware_policy_allows_agy_flash_only_on_agy` |
| TC-315 | FR-314 | unit | opencode runner; `/graph override` | validate free/non-free model | free allowed, others rejected | *proposed*: `opencode_policy_allows_free_models_only`, `graph_override_validates_runner_policy` |
| TC-910 | FR-910 | unit | Ids `composer-2.5`, `composer-2.5-fast`, `grok-4.7-build-fast`, `gemini-3.8-flash-medium`, `gemini-3-flash-fast`, `opencode/glm-5.3-flash-free`, `opencode/glm-fast-free` | `validate_model_for_runner` for cursor, agy, opencode, grok, and codex | Every id containing `fast` is an error on every runner; flash and free ids that do not contain `fast` stay allowed on their runners; `composer-2.5` stays allowed on cursor | `src/model_policy.rs` `fast_models_are_rejected_for_every_runner` |
| TC-911 | FR-911 | unit | `grok-4.7`, `grok-4.6`, `Grok-4.7`, `composer-2.5`, `grok`, `opencode/glm-5.3-flash-free` | `validate_model_for_runner` with runner `grok`, `GROK`, `cursor`, `agy`, `codex`, `opencode` | `grok-*` without `fast` passes only for runner `grok`; cursor still accepts `composer-2.5` and rejects `grok-4.7`; grok rejects `composer-2.5` with an empty registry | `src/model_policy.rs` `grok_models_are_allowed_only_for_the_grok_runner` |
| TC-912 | FR-912 | integration | `docs/dmuxai-graph.yaml` | Read the file | `allowed_only: composer-2.5` remains; `disallow_patterns` still lists `*grok*`; no node line is `model: grok`; the comment above `disallow_patterns` names the grok runner and says the file stays composer-2.5 | `tests/model_policy_repo.rs` `dmuxai_graph_yaml_keeps_grok_disallow_and_explains_why` |

## 6. Data, integration, and safety contracts

- Model ids are passed to CLIs as argv, never interpolated into a shell.
- `docs/dmuxai-graph.yaml` node models stay `composer-2.5`. The grok amendment adds a comment on `disallow_patterns` and does not add a grok model (FR-912). The comment to place immediately above `disallow_patterns` is:

```yaml
  # This repo graph stays composer-2.5 on every node. Runtime policy allows
  # ^grok- models on the grok runner only (SPEC-model-policy FR-911).
  # *grok* stays here so this file does not assign a grok model.
```

## 7. Implementation plan

Shipped rows stay as they are. FR-910..FR-912 and TC-910..TC-912 are implemented with the grok runner: [PLAN-grok-runner.md](../plans/PLAN-grok-runner.md) and [TASKS-grok-runner.md](../tasks/TASKS-grok-runner.md). TC-315 remains proposed.

## 8. Review and test exit criteria

- All non-proposed TCs pass. TC-315 stays proposed until an opencode override test is added.
- TC-910, TC-911, and TC-912 pass before the grok runner is marked complete.
- `cargo test --test model_policy_repo` still passes after the yaml comment lands.
