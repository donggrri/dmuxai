# TASKS: docs-tree

Status: complete
Spec: [SPEC-docs-tree](../specs/SPEC-docs-tree.md)
Summary row: `docs/tasks/TASKS-v0.2.1.md` tasks 7-8.

- [x] T1 — `list_children` API with confinement, symlink checking, and deferred `has_children`.
  Requirements: FR-830, FR-831. Verify:
  `cargo test --locked --lib docs::list::tests::tree_children_lists_one_level`
- [x] T2 — `GET /api/docs/tree` endpoint, auth verification, and 400/403/404 error responses.
  Requirements: FR-830, FR-831. Verify:
  `cargo test --locked --lib web::docs_api::tests::tree_api_confined_and_bounded`,
  `cargo test --locked --lib web::docs_api::tests::docs_routes_require_auth`
- [x] T3 — Sidebar shell and collapsible tree UI in `index.html`, `docs.css`, and `docs.js`.
  Requirements: FR-832, FR-833, UX-830. Verify:
  `cargo test --locked --lib web::assets::tests::docs_tree_sidebar_assets`
- [x] T4 — Headless browser verification of tree expansion, file clicking, and drawer behavior.
  Requirements: FR-832, UX-830. Verify:
  `cargo test --locked --test web_docs_preview -- --ignored`
- [x] T5 — Quality gates and linters. Verify:
  `node --check src/web/assets/*.js`,
  `cargo fmt --check`,
  `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked --all-targets`
