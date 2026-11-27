# PLAN: cursor-agent keyword

Status: in progress  
Spec: [SPEC-cursor-agent-keyword.md](../specs/SPEC-cursor-agent-keyword.md)

1. Add `src/runner_id.rs` with `canonical_runner_id` (FR-3200, TC-3200).
2. Apply it at every parse boundary: CLI, settings (all keys), graph plan/override, TUI, MCP, web, custom-agent reserved names (FR-3201..FR-3205).
3. `runners --json` `aliases` (FR-3206).
4b. Owner decision OQ-3200: flip the canonical id to `cursor-agent` (`CURSOR_RUNNER_ID`), emit it in every output, normalize legacy `cursor` on read in SQLite, journals, events, graph stores, config (FR-3207..FR-3209, TC-3209..TC-3214).
4. Help text, README, docs, CHANGELOG (UX-3200).

## Risks and rollback

- **A boundary is missed.** The alias is rejected there with the existing unknown-agent error; never a policy bypass. The review records the scan.
- **Rollback.** Revert the commit. Data written after the flip stores `cursor-agent`; an older binary cannot read those rows as Cursor, so back up before downgrading.
