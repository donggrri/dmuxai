# PLAN: todo-sidebar

Spec: [SPEC-todo-sidebar](../specs/SPEC-todo-sidebar.md)
Tasks: [TASKS-todo-sidebar](../tasks/TASKS-todo-sidebar.md)

1. `index.html`: add a header `To-dos` button with a count badge, add a `To-dos` tab next to `Files`, and move the to-do section into a `tab-panel-todos` panel with a state select and a search box. Keep the element ids, so `app.js` keeps working unchanged.
2. `docs.js` (owns the sidebar shell): track the active tab per workspace, open or switch from either header toggle, set `aria-expanded`/`aria-selected` on toggles and tabs, and make Esc skip text fields and open modals.
3. `app.js`: filter `renderTodos` by state and search, keep the item being edited visible, and update the badge from the snapshot.
4. `docs.css`: hide hidden tab panels (the panel class sets `display: flex`), lay out the to-do panel and filter, and style the badge.
5. Tests: TC-840 asset test, TC-841 action coverage in `web::parity`, TC-842 headless Chrome e2e.
