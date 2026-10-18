# PLAN: v0.2.2

Status: Draft (v0.2.2)
Specs: SPEC-centered-prompt, SPEC-session-sidebar, SPEC-workspace-dir. Roadmap: [docs/ROADMAP.md](../ROADMAP.md)

## Process

- Spec first: this PR lands the three SPECs. Each feature then gets its own worktree under `/workspace/dmux-wt/<task>`, its own target dir, and its own PR closing its issue. Each PR flips its SPEC Status to `Implemented (v0.2.2)` and its TEST-PLAN rows from ⏳ to ✅.
- Runner roles (Dongjin, 2026-10-18 09:42):
  - AGY implements (non-fast, `DMUXAI_AGY_IDLE_TIMEOUT_SECS=1500`); the maintainer may hand-code;
  - opencode-haiku reviews; switch reviews to AGY if the OpenRouter balance drops below $0.50;
  - opencode free models (deepseek-v4.1-flash-free, glm-5.3-flash-free) answered again on 2026-10-18 and may be extra implementers or reviewers;
  - Cursor is **not** an implementer or reviewer; no fast models.
- Linux only. The Windows target must still pass clippy, because CI runs windows-latest.
- Each UI PR carries before/after screenshots (demo data, no tokens).
- Merge rule: squash-merge when there are no blockers and CI is green. Never push main. No release in this round.

## Order and design notes

The order is 1 → 2 → 3. Items 1 and 2 share the layout, so the left sidebar is built on the centering rule. Item 3 touches the CLI and server, and could run in parallel, but the header markup conflicts with item 2, so it goes last.

1. **Centered prompt (SPEC-centered-prompt)**:
   - replace `body.sidebar-open main { margin-right }` with dock variables and a padded content wrapper;
   - add an asset test and a geometry e2e for the four sidebar states and four widths.
2. **Session sidebar (SPEC-session-sidebar)**:
   - extend `SessionConversation` (`last_agent`, `turn_count`, `running`) from `ConversationCard` and the in-flight turn check;
   - left `nav#sessions-sidebar` in index.html, with the list, filter and `New` button in session.js (which owns the session panel);
   - drop `.session-conversations`;
   - add the agent, relative time and running mark to the TUI picker rows.
3. **Workspace dir (SPEC-workspace-dir)**:
   - `workspace::resolve_workspace_root_with(flag)` with precedence and `validate_workspace_dir` (deny list), used by `main.rs` for every mode;
   - `[workspace] dir` in the user settings;
   - `/api/workspace/{dirs,validate,switch}`; recent list in the state dir;
   - re-exec via `std::os::unix::process::CommandExt::exec`, behind a trait so tests can use a double;
   - a header button and dialog in a new `workspace.js`;
   - a runner cwd test across custom agents and graph nodes.
