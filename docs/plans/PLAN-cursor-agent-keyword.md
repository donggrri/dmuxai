# PLAN: cursor-agent keyword

Status: in progress  
Spec: [SPEC-cursor-agent-keyword.md](../specs/SPEC-cursor-agent-keyword.md)

1. Add `src/runner_id.rs` with `canonical_runner_id` (FR-3100, TC-3100).
2. Apply it at every parse boundary: CLI, settings (all keys), graph plan/override, TUI, MCP, web, custom-agent reserved names (FR-3101..FR-3105).
3. `runners --json` `aliases` (FR-3106).
4. Help text, README, docs, CHANGELOG (UX-3100).

## Risks and rollback

- **A boundary is missed.** The alias is rejected there with the existing unknown-agent error; never a policy bypass. The review records the scan.
- **Rollback.** Remove the helper calls; `cursor` keeps working untouched.
