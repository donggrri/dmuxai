# PLAN: Codex model effort and mode-context transition

Status: complete
Spec: [SPEC-codex-model-effort-mode-context.md](SPEC-codex-model-effort-mode-context.md)

1. Add red UI tests for Tab model-to-effort transition and fresh shared-context
   request after mode change. (UX-040, FR-043, TC-047, TC-048)
2. Add a provider-neutral binding-retirement operation and use it only after an
   idle mode change is accepted. (FR-043, BR-042, NFR-042)
3. Implement Codex model-picker Tab behavior and update picker affordances.
   (UX-040, NFR-042)
4. Update baseline/review and run narrow, specification, and Rust gates.
