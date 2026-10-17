# TASKS: todo-sidebar

Status: complete
Spec: [SPEC-todo-sidebar](../specs/SPEC-todo-sidebar.md)
Plan: [PLAN-todo-sidebar](../plans/PLAN-todo-sidebar.md)
Summary row: `docs/tasks/TASKS-v0.2.1.md` task 9.

- [x] T1 — Header `To-dos` button and badge, `To-dos` tab, and the to-do UI moved into the sidebar panel.
  Requirements: FR-840, UX-840. Verify: `cargo test --locked --lib web::assets::tests::todo_sidebar_assets`
- [x] T2 — State filter and search; actions unchanged.
  Requirements: FR-841. Verify: `cargo test --locked --lib web::parity::tests::todo_actions_still_covered`
- [x] T3 — Shared sidebar tabs, toggle state and Esc rule.
  Requirements: FR-842. Verify: `cargo test --locked --test web_todo_sidebar -- --ignored`
- [x] T4 — Gates. Verify: `node --check src/web/assets/*.js`, `cargo fmt --check`,
  `cargo clippy --locked --all-targets -- -D warnings` (Linux and `--target x86_64-pc-windows-gnu`),
  `cargo test --locked --all-targets`.
