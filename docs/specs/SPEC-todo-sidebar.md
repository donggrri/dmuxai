# SPEC: todo-sidebar

Status: Draft (v0.2.1)
Created: 2026-10-17
Related: Dongjin dogfood 2026-10-17 23:38 ("let me open/browse the to-do list via a button or sidebar"); SPEC-web-dashboard to-do routes

## 1. Decision summary

To-dos (create, Jira preview, MCP import, run) are an inline panel in the dashboard's main column, below the agents. They are easy to lose while chatting.

Decision: move the to-do UI into a toggleable sidebar panel, opened from a header `To-dos (n)` button, using the same sidebar shell as the Files tree (SPEC-docs-tree). No new server API: the existing `/api/todos*` routes and the snapshot are reused.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-840 | Must | The header has a `To-dos` button with a count badge of open to-dos. It toggles a sidebar panel that holds the whole to-do UI (list, add form, Jira field, MCP import). The panel is closed by default, and its open state persists in `localStorage` | TC-840, TC-841 |
| FR-841 | Must | The to-do list in the panel can be filtered by state (`open` / `done` / `all`) and by a text search. Each item keeps its run/done/delete actions and view-only mode exactly as before | TC-840, TC-841 |
| FR-842 | Must | The Files and To-dos sidebars share one right-hand sidebar region with tabs: opening one switches to its tab, and the toggle buttons show which is active. `Esc` closes the sidebar | TC-840, TC-842 |
| UX-840 | Must | The badge and buttons have accessible names (`aria-controls`, `aria-expanded`). Below 900px the sidebar is an overlay drawer, consistent with UX-830 | TC-840 |

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-840 | FR-840..FR-842, UX-840 | unit (asset) | index.html, JS and CSS assets | Header `To-dos` button with badge and `aria-controls`; the to-do form/list live inside the sidebar tab panel; state filter and search; Esc handler | `src/web/assets.rs` `todo_sidebar_assets` |
| TC-841 | FR-840, FR-841 | unit | `web::parity` matrix | To-do actions keep their parity rows (create/run/done/delete/import) after the move | `src/web/parity.rs` `todo_actions_still_covered` |
| TC-842 | FR-840..FR-842 | e2e (ignored, headless Chrome) | demo snapshot with to-dos | Clicking `To-dos` opens the panel with items; switching to Files changes the tab; screenshot captured | `src/web/server.rs` `todo_sidebar_renders_in_headless_chrome` |
