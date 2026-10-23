# SPEC: cursor-model-pin

Status: draft
Owner: Antigravity
Created: 2026-10-17
Baseline: `73e3f4d`
Related: dogfood #18, M7, SPEC-model-policy, SPEC-agent-graph-orchestration

## 1. Decision summary

Every Cursor turn in DmuxAI must pass `--model composer-2.5` to `cursor-agent` unless the user explicitly selected a model via F2 that satisfies `dmuxai::model_policy::validate_model_for_runner("cursor", ..)`. Cursor must never fall back to the provider default model and must never execute with any `*-fast` model. In the status bar, Cursor must display `composer-2.5` rather than "provider default". If a disallowed model is somehow configured or supplied to Cursor, DmuxAI replaces it with `composer-2.5` and emits an explanatory system notice.

## 2. Scope

### In scope

- Enforcing `--model composer-2.5` in Cursor adapter argv generation (`src/agent/cursor.rs`) when no model or an invalid/disallowed model is supplied.
- Rejecting and replacing fast models (e.g. `composer-2.5-fast`, `*-fast`) and foreign models for Cursor across `effective_model_for`, `apply_runner_and_model`, and `submit_to_agent` in `src/app.rs`.
- Emitting a system notice whenever a disallowed model is replaced with `composer-2.5`.
- Guaranteeing that slash-prefixed prompts (e.g., prompts starting with `/` that bypass graph auto-planning) sent to Cursor pin `composer-2.5`.
- Displaying `composer-2.5` instead of `"provider default"` in the Cursor status bar.
- Automated unit tests covering adapter argv generation, fast model replacement, slash prompt model pinning, and status bar rendering.

### Out of scope / non-goals

- Altering AGY, Codex, or custom agent model policies (AGY continues to allow `gemini-*-flash-*`, opencode allows `opencode/*-free`, Codex uses its catalog).
- Redesigning the F2 model picker UI layout.
- Network calls or live provider smoke testing during unit tests.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | None; all requirements are fully specified by dogfood #18. | Antigravity | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Pinned model | The mandatory repository model `composer-2.5` for Cursor. | Provider default model advertised by `cursor-agent`. |
| Disallowed model | Any model for Cursor that fails `validate_model_for_runner("cursor", ..)`, such as `*-fast` or non-composer models. | Allowed AGY flash models or allowed opencode free models. |
| Status model segment | The text rendered in the TUI status bar indicating the active model for the selected agent. | Graph orchestrator status line. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-410 | Must | Given the Cursor agent is active and no model has been selected in F2, when a turn is submitted via normal prompt, graph node, single plan, or resend | Then `submit_to_agent` pins `composer-2.5` as the model for the turn | If the catalog selection is None, it is initialized to `composer-2.5` | TC-413 |
| FR-411 | Must | Given a model is set for Cursor that fails `validate_model_for_runner("cursor", ..)` (e.g., contains `fast` or is a foreign model), when evaluated or submitted | Then it is replaced with `composer-2.5` and a system notice is pushed to the chat | If the model string is empty or malformed, it is also replaced with `composer-2.5` | TC-411, TC-412 |
| FR-412 | Must | Given a prompt starting with `/` (e.g. `/custom prompt`) submitted to Cursor outside graph auto-planning, when `submit_to_agent` executes | Then the request sent to Cursor agent has model pinned to `composer-2.5` | If previously unset, the model catalog selection becomes `composer-2.5` | TC-413 |
| UX-410 | Must | Given Cursor is the selected agent, when the status bar is rendered | Then the model segment displays `composer-2.5` instead of `provider default` | If a disallowed model is currently selected in the catalog, `composer-2.5` is displayed | TC-414 |
| INT-410 | Must | Given the Cursor adapter builds CLI arguments in `configure_command`, when `--model` is configured | Then the argv contains `--model composer-2.5` by default, and any disallowed or fast model is replaced with `composer-2.5` | If `selected_model` is `None`, `--model composer-2.5` is emitted | TC-410, TC-411 |
| FR-1866 | Must | Cursor adapter configure_command model validation (`src/agent/cursor.rs`, `src/model_policy.rs`). | `--model <model>` verifies `validate_model_for_runner("cursor", model)`. If None or invalid, falls back to `ALLOWED_MODEL` (`composer-2.5`). | Neither foreign nor fast models are ever passed to cursor-agent. | TC-1866 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-410 | INT-410 | unit | `configure_command` with `selected_model: None` | Generate argv for Cursor process | `args.windows(2)` contains `["--model", "composer-2.5"]` | `src/agent/cursor.rs` |
| TC-411 | FR-411, INT-410 | unit | `configure_command` with `selected_model: Some("composer-2.5-fast")` or `"other-model"` | Generate argv for Cursor process | Fast/foreign model is rejected; args contains `["--model", "composer-2.5"]` | `src/agent/cursor.rs` |
| TC-412 | FR-411 | unit | App state with Cursor selected and catalog model set to `"composer-2.5-fast"` | User submits prompt or calls `submit_to_agent` | Model is replaced with `composer-2.5`, system notice is emitted | `src/app.rs` |
| TC-413 | FR-410, FR-412 | unit | Fresh app with Cursor agent and unset model catalog, input starting with `/test` | `app.handle_enter()` / `submit_to_agent` | Model passed to adapter and catalog selected is `composer-2.5` | `src/app.rs` |
| TC-414 | UX-410 | unit | Fresh app or app with Cursor selected and no F2 model selected | Render status bar or inspect status segments | Model segment displays `"composer-2.5"`, not `"provider default"` | `src/app.rs` |
| TC-1866 | FR-1866 | unit | `configure_command` with unset or foreign model | build argv | argv contains `["--model", "composer-2.5"]` | `src/agent/cursor.rs` `cursor_argv_always_contains_composer_2_5_by_default`, `cursor_argv_rejects_and_replaces_fast_model` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Recorded turn outputs in conversation store store `composer-2.5` as `model_id` for Cursor turns. | Fully compatible with schema v2. | TC-413 |
| Provider protocol | `cursor-agent` CLI is always invoked with `--model composer-2.5` unless an allowed model is chosen. | Prevents silent fallback to non-composer or fast models. | TC-410, TC-411 |
| Security / permissions | Enforces model policy at both app orchestrator and adapter process levels (defense in depth). | Disallowed models cannot leak through any submission path. | TC-411, TC-412 |
| Platform | Cross-platform (Linux / macOS / WSL). | No platform dependencies. | TC-410 |

## 7. Implementation plan

Link to `../plans/PLAN-cursor-model-pin.md` and `../tasks/TASKS-cursor-model-pin.md`.

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Cursor adapter argv model pinning & validation | `src/agent/cursor.rs` | INT-410, FR-411 | TC-410, TC-411 |
| 2 | Model selection enforcement & notice in App (`effective_model_for`, `apply_runner_and_model`, `submit_to_agent`, F2) | `src/app.rs` | FR-410, FR-411, FR-412 | TC-412, TC-413 |
| 3 | Status bar display for Cursor | `src/app.rs` | UX-410 | TC-414 |
| 4 | Test plan index update & dogfood log update | `docs/TEST-PLAN.md`, `docs/dogfood/README.md` | FR-410 | Manual inspection |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] External-provider smoke tests are marked run, skipped, or blocked with a reason; skipped is never reported as passing.
- [ ] `../reviews/REVIEW-cursor-model-pin.md` records commands, result, residual risk, and rollback notes.
