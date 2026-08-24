# PLAN: Codex effort, tool progress, and usage status

Status: complete
Spec: [SPEC-codex-effort-tool-usage.md](../specs/SPEC-codex-effort-tool-usage.md)

1. Extend provider-neutral event and adapter interfaces with Codex effort,
   bounded tool-progress, and status-report contracts. Preserve the existing
   non-blocking command queue. (FR-040, FR-041, FR-042, NFR-040)
2. Update the Codex App Server adapter to parse advertised efforts, attach the
   selected effort to a turn, normalize tool item lifecycle messages, and read
   status endpoints without turning their failure into a turn failure.
   (FR-040, FR-041, FR-042)
3. Add `/effort` and `/status` to the TUI command model, picker routing,
   bounded rendering, and status bar. Keep effort Codex-only and idle-only.
   (FR-040, BR-040, FR-042)
4. Add deterministic fixtures/state tests for request construction, tool event
   ownership/bounds, status success/unavailable, and UI behavior. (TC-040–045)
5. Run spec validation and Rust gates, then review every requirement and
   record live-provider smoke-test status.

Risks: App Server schemas evolve and account endpoints require certain auth
modes. Parsing stays optional and field-tolerant, while tests use saved JSON.
Rollback: revert the new adapter/UI modules; there is no SQLite migration or
provider configuration write to undo.
