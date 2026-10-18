# PLAN: sidebar-resize

Spec: [SPEC-sidebar-resize](../specs/SPEC-sidebar-resize.md)
Tasks: [TASKS-sidebar-resize](../tasks/TASKS-sidebar-resize.md)

1. `index.html`: add `#sessions-resize` and `#sidebar-resize` as separator handles on the inner edges, and load `/assets/sidebar-resize.js`.
2. `src/web/assets/sidebar-resize.js`: drag, keyboard, clamp, and `localStorage` for both docked sidebars. A click does not persist. A cancelled pointer reverts. Capture failure falls back to window listeners.
3. `docs.css`: `--left-sidebar-width` / `--right-sidebar-width` and the dock variables, handles fully outside the inner edge, hidden at 900px, plus `sidebar-resizing` and `sidebar-resize-instant`.
4. `src/web/assets.rs` and `src/web/server.rs`: embed the script and serve `GET /assets/sidebar-resize.js`.
5. Tests: TC-970 in `src/web/assets.rs`, TC-971 and TC-972 in `tests/web_sidebar_resize.rs`.
