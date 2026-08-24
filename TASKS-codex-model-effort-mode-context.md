# TASKS: Codex model effort and mode-context transition

Status: complete
Spec: [SPEC-codex-model-effort-mode-context.md](SPEC-codex-model-effort-mode-context.md)

- [x] T1 — Add and run red TC-047/TC-048 regression tests. Requirements:
  UX-040, FR-043. Verify: the two named `cargo test` commands fail before the fix.
- [x] T2 — Retire only the selected idle agent binding on mode change. Requirements:
  FR-043, BR-042, NFR-042. Verify: `cargo test mode_change_reopens_the_agent_with_shared_context`.
- [x] T3 — Add Codex model-picker Tab-to-effort transition. Requirements:
  UX-040, NFR-042. Verify: `cargo test model_picker_tab_selects_codex_model_and_opens_its_effort_picker`.
- [x] T4 — Update baseline/review and run required gates. Requirements: all.
  Verify: spec checks, `cargo fmt --check`, `cargo test --all-targets`, and clippy.
