# REVIEW: Codex model effort and mode-context transition

Status: complete
Spec: [SPEC-codex-model-effort-mode-context.md](../specs/SPEC-codex-model-effort-mode-context.md)

## Spec compliance

- UX-040: In the Codex model picker, Tab selects the highlighted model, sets
  its advertised default effort, and opens that model's effort picker. Enter
  retains the direct model-selection path.
- FR-043: An idle mode change retires only the selected conversation/agent
  binding. The next prompt has no provider resume id and carries the completed
  DmuxAI context projection into a fresh native session.
- BR-042 / NFR-042: Busy mode changes remain rejected. No agent other than the
  selected one has its mode or binding altered, and existing sandbox and
  approval contracts are unchanged.

## Regression and safety review

- The red TC-047 test reproduced the missing Tab behavior: the model picker
  remained open. The red TC-048 test reproduced the mode failure: the follow-up
  retained `resume_session` instead of receiving shared context.
- `ConversationEngine::retire_active_binding` checks that the agent is idle and
  updates only an active binding for the selected conversation/agent. It makes
  no transcript or schema change.
- The later provider binding is still recorded through the normal turn event,
  so the new native session becomes the only active binding after it starts.

## Test evidence

- Red reproduction: `cargo test model_picker_tab_selects_codex_model_and_opens_its_effort_picker` failed before the fix because `model_picker` stayed open.
- Red reproduction: `cargo test mode_change_reopens_the_agent_with_shared_context` failed before the fix because the follow-up had a resume session.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-codex-model-effort-mode-context.md`: passed.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/DMUXAI-BASELINE.md`: passed.
- `C:\Users\tlsfmswls\.cargo\bin\cargo.exe fmt --check`: passed.
- `C:\Users\tlsfmswls\.cargo\bin\cargo.exe test --all-targets` with an isolated temporary target directory: passed; 103 tests passed and 5 authenticated-provider smoke tests were ignored as intended.
- `C:\Users\tlsfmswls\.cargo\bin\cargo.exe clippy --all-targets --all-features -- -D warnings` with an isolated temporary target directory: passed.
- Live provider smoke tests: skipped because they require authenticated CLIs and can make real requests. The new behavior is covered by deterministic UI/conversation tests.

## Residual risks and rollback

Fresh-mode session startup relies on the existing context-projection budget; a
very long conversation may already invoke the established summary workflow.
Rollback is code-only: remove binding retirement and Tab transition; no data
migration is involved.
