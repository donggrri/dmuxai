# PLAN: Cursor model pin (composer-2.5)

Status: complete  
Spec: [SPEC-cursor-model-pin.md](../specs/SPEC-cursor-model-pin.md)

1. Enforce `--model composer-2.5` at the lowest level in the Cursor adapter (`src/agent/cursor.rs` `configure_command`).
   - By default (`selected_model: None`), inject `--model composer-2.5`.
   - If a model is passed, validate it via `dmuxai::model_policy::validate_model_for_runner("cursor", ..)`. Disallowed models (e.g. `*-fast`, foreign models) are replaced with `composer-2.5`.
   - Update unit test `cursor_resume_session_is_passed_to_the_command` to use `composer-2.5`.
   - Add unit tests `cursor_argv_always_contains_composer_2_5_by_default` (TC-410) and `cursor_argv_rejects_and_replaces_fast_model` (TC-411). (INT-410, FR-411)

2. Enforce model selection and replacement in `src/app.rs`:
   - `effective_model_for`: For `Agent::Cursor`, always return an allowed model (default `composer-2.5`), never `None` or a disallowed model.
   - `apply_runner_and_model`: Uses `effective_model_for` to select and pin `composer-2.5` on Cursor.
   - `submit_to_agent`: Prior to submitting any turn (normal prompt, slash-prefixed prompt, graph node, single plan, resend), check Cursor's model catalog. If unset, initialize to `composer-2.5` and notify the adapter. If set to a disallowed model, replace it with `composer-2.5`, notify the adapter, and emit an explanatory system notice.
   - F2 model picker: When selecting a model for Cursor, if disallowed, replace with `composer-2.5` and push an explanatory system notice.
   - `draw_status`: In the status bar, Cursor must display `composer-2.5` instead of `"provider default"`.
   - Add unit tests in `src/app.rs`:
     - `disallowed_model_in_app_is_replaced_with_composer_2_5_and_notified` (TC-412, FR-411)
     - `slash_prefixed_prompt_to_cursor_still_pins_composer_2_5` (TC-413, FR-410, FR-412)
     - `cursor_status_shows_composer_2_5_instead_of_provider_default` (TC-414, UX-410)

3. Update documentation and traceability:
   - Add SPEC row to `docs/TEST-PLAN.md` index table and dogfood matrix.
   - Mark dogfood #18 as fixed in `docs/dogfood/README.md`.
   - Complete `docs/reviews/REVIEW-cursor-model-pin.md`.

## Risks and mitigations

- **Risk:** Existing tests or callers might expect `cursor.rs` without `--model` or with dummy model names.  
  **Mitigation:** Inspected all references in `src/agent/cursor.rs`. Only one test passed `"cursor-model"`; updating it to `"composer-2.5"` preserves the test's intent while adhering to policy.
- **Risk:** Status bar truncation on narrow viewports.  
  **Mitigation:** `composer-2.5` (12 chars) is shorter than `"provider default"` (16 chars), reducing viewport pressure.
- **Risk:** Overriding non-Cursor runners (AGY, opencode, Codex).  
  **Mitigation:** The model pinning specifically checks `agent == Agent::Cursor` or `runner == "cursor"`. Other runners continue their respective policies.

## Rollback

Code and documentation change only. Reverting the adapter argv builder, `src/app.rs` model resolution, and documentation restores previous behavior without database migration or persisted data loss.
