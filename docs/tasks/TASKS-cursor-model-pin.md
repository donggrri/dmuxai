# TASKS: Cursor model pin (composer-2.5)

Status: complete  
Spec: [SPEC-cursor-model-pin.md](../specs/SPEC-cursor-model-pin.md)

- [x] T1 - Enforce `--model composer-2.5` in `src/agent/cursor.rs` `configure_command` and add unit tests `cursor_argv_always_contains_composer_2_5_by_default` and `cursor_argv_rejects_and_replaces_fast_model`. Update `cursor_resume_session_is_passed_to_the_command`.
  Requirements: INT-410, FR-411. Verify: `cargo test cursor_argv`.
- [x] T2 - Enforce model resolution and replacement in `src/app.rs`: update `effective_model_for`, `apply_runner_and_model`, `submit_to_agent`, and F2 selection for Cursor. Push system notice when disallowed model is replaced.
  Requirements: FR-410, FR-411, FR-412. Verify: `cargo test disallowed_model_in_app` and `cargo test slash_prefixed_prompt_to_cursor`.
- [x] T3 - Update Cursor status bar model rendering in `src/app.rs` `draw_status` to show `composer-2.5` instead of `"provider default"`.
  Requirements: UX-410. Verify: `cargo test cursor_status_shows_composer_2_5`.
- [x] T4 - Update `docs/TEST-PLAN.md` spec index table, mark dogfood #18 as fixed in `docs/dogfood/README.md`, and write `docs/reviews/REVIEW-cursor-model-pin.md`.
  Requirements: all. Verify: manual inspection and check-spec script.
