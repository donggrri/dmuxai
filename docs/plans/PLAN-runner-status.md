# PLAN: runner status and enable switch

Status: complete
Spec: [SPEC-runner-status.md](../specs/SPEC-runner-status.md)

1. Probe core: parsers, masking, async probe with timeout, cache, probe table keyed by runner id (FR-3300..FR-3305, NFR-3311).
2. `[runners.<id>] enabled` setting (user config only), comment-preserving TOML editor with atomic write, admission checks (FR-3307, FR-3308).
3. CLI: `runners status`, `runners enable|disable` (FR-3306, FR-3308).
4. HTTP route pair and MCP tool (FR-3309, FR-3310).
5. Dashboard Runners panel (FR-3311).

## Risks and rollback

- **Probe output drift.** Parsers are strict and fall back to `unknown`/`error`; fake-binary tests pin the shapes observed on 2026-11-27.
- **User config edit.** Line-based edit validated by re-parsing; refused unchanged on any doubt.
- **Rollback.** Remove the new routes, subcommands and key; no data migration.
