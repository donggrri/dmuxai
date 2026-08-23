# Plan: Phase 1 workspace UI

Status: complete
Spec: `SPEC-phase1-workspace-ui.md`

## Implementation order

1. Define the dark visual tokens and responsive three-column/three-row layout.
2. Render Workspace, Conversation, Context, Activity, Prompt, and status
   regions while retaining the existing conversation renderer.
3. Bind Workspace `New task` and recent conversation rows to the existing
   persistence engine through rendered mouse hit targets.
4. Add deterministic rendering and sidebar-state tests.
5. Run formatting, narrow tests, the full target suite, and Clippy.

## Modules changed

- `src/app.rs`: Ratatui layout, presentation, responsive behavior, and
  Workspace sidebar hit testing.

## Risks and mitigations

- Small terminals can overflow: hide both sidebars below 76 columns and test
  the compact layout.
- Drawn hit targets can become stale: reset them before every sidebar render.
- Sidebar selection could affect persistence incorrectly: select by stored ID
  and assert the selected conversation title remains intact.
- Visual shell work could imply unavailable data: document Context and
  Activity as Phase 1 placeholders.

## Rollback

Revert the `src/app.rs` UI-shell change and its documentation files. No schema,
stored-record format, adapter protocol, or external-provider state is changed.
