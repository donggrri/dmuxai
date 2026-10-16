# REVIEW: Cursor model pin (composer-2.5)

Status: complete  
Spec: [SPEC-cursor-model-pin.md](../specs/SPEC-cursor-model-pin.md)  
Reviewed: 2026-10-17  
Target: Linux / Cross-platform

## Spec compliance

- INT-410: `src/agent/cursor.rs` `configure_command` pins `--model composer-2.5` by default when `selected_model` is `None`, and validates any provided model against `dmuxai::model_policy::validate_model_for_runner("cursor", ..)`. Disallowed models (e.g. `*-fast`, foreign models) are replaced with `composer-2.5`. Verified by `cursor_argv_always_contains_composer_2_5_by_default` (TC-410) and `cursor_argv_rejects_and_replaces_fast_model` (TC-411).
- FR-410: When a Cursor turn is submitted and no model has been selected in F2, `submit_to_agent` pins `composer-2.5`, records it in the model catalog, and informs the adapter.
- FR-411: If a disallowed model is somehow set for Cursor, it is replaced with `composer-2.5` across `submit_to_agent`, `effective_model_for`, `apply_runner_and_model`, and F2 selection, and an explanatory system notice is emitted to the chat. Verified by `disallowed_model_in_app_is_replaced_with_composer_2_5_and_notified` (TC-412).
- FR-412: Prompts starting with `/` (such as slash commands or unknown slash prefixes that bypass graph auto-planning) sent to Cursor pin `composer-2.5` without falling back to provider default. Verified by `slash_prefixed_prompt_to_cursor_still_pins_composer_2_5` (TC-413).
- UX-410: `draw_status` renders `composer-2.5` instead of `"provider default"` whenever Cursor is active and no alternative allowed model is selected. Verified by `cursor_status_shows_composer_2_5_instead_of_provider_default` (TC-414).

## Regression and safety review

- Provider isolation: The model pinning specifically checks `agent == Agent::Cursor` or `runner == "cursor"`. AGY flash models (`gemini-*-flash-*`), opencode free models, and Codex models remain subject to their runner policies.
- Defense in depth: The policy is enforced both at the application orchestrator level (`src/app.rs`) and at the lowest adapter process argv level (`src/agent/cursor.rs`). Even if an invalid model bypassed the app layer, the adapter argv builder would replace it with `composer-2.5`.
- Persistence: Schema v2 compatibility is preserved. Turn outputs correctly persist `composer-2.5` as `model_id`.
- User visibility: If a disallowed model is replaced, the user receives an explicit system notice explaining the replacement.

## Test evidence

- `scripts/check-spec.ps1` logic verified on `docs/specs/SPEC-cursor-model-pin.md`: PASS.
- Unit tests added:
  - `cursor_argv_always_contains_composer_2_5_by_default` (`src/agent/cursor.rs`, TC-410)
  - `cursor_argv_rejects_and_replaces_fast_model` (`src/agent/cursor.rs`, TC-411)
  - `disallowed_model_in_app_is_replaced_with_composer_2_5_and_notified` (`src/app.rs`, TC-412)
  - `slash_prefixed_prompt_to_cursor_still_pins_composer_2_5` (`src/app.rs`, TC-413)
  - `cursor_status_shows_composer_2_5_instead_of_provider_default` (`src/app.rs`, TC-414)
- User instructions requested: "Do not run cargo; I will run it and report errors." Automated `cargo test` execution deferred to user.

## Residual risks and rollback

No database migration is introduced. Rollback requires reverting the changes in `src/agent/cursor.rs` and `src/app.rs`.
