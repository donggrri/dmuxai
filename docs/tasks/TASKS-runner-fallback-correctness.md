# TASKS: Runner fallback handover correctness

Status: complete  
Spec: [SPEC-runner-fallback.md](../specs/SPEC-runner-fallback.md)

- [x] T1 - Hold the handover state in `PendingFallback` (chain + original
  prompt), consume it at the top of `try_submit_to_agent`, and clear it on
  every exit path of `try_runner_fallback`.
  Requirements: FR-2130. Verify: `cargo test tc_2130_exhausted_handover_leaves_no_pending_chain`.
- [x] T2 - Carry the original user prompt in `ActiveTurn`
  (`fallback_original_prompt`), build each hop's prompt as
  `preface(this hop) + original prompt`, and always display the original user
  text. Requirements: FR-2131. Verify: `cargo test tc_2131_two_hops_keep_one_preface_and_the_original_prompt`.
- [x] T3 - Record the handover only after the target turn started; restore
  runner, model, selection and graph-node mapping when a target fails to
  start; let the original error be recorded and finished exactly once when
  every target fails. Requirements: FR-2132. Verify: `cargo test tc_2132_all_targets_failing_records_the_original_error_once`.
- [x] T4 - Apply the target runner and model before submitting and skip a
  target whose runner or model cannot be applied, with a system note.
  Requirements: FR-2133. Verify: `cargo test tc_2133_target_with_unusable_model_is_skipped`.
- [x] T5 - Build the headless answer text from the final runner's messages
  only and print the run's `fallbacks` array in the CLI JSON.
  Requirements: FR-2134, FR-2119. Verify: `cargo test tc_2134_headless_text_uses_the_final_runner_only`
  and `cargo test --test agent_api_cli tc_2136_cli_fallback_handover_uses_the_second_agent`.
- [x] T6 - Merge the user and workspace fallback tables field-wise.
  Requirements: FR-2101. Verify: `cargo test --lib tc_2135`.
- [x] T7 - Update `docs/specs/SPEC-runner-fallback.md` and run the gates.
  Requirements: all. Verify: `bash scripts/check-spec.sh docs/specs/SPEC-runner-fallback.md`,
  `python3 scripts/check-tc-links.py`, `cargo fmt --all -- --check`,
  `cargo check --all-targets`.
