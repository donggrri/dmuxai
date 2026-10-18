# TASKS: sidebar-resize

Status: complete
Spec: [SPEC-sidebar-resize](../specs/SPEC-sidebar-resize.md)
Plan: [PLAN-sidebar-resize](../plans/PLAN-sidebar-resize.md)

- [x] T1 — Handles in `index.html`, drag and persist logic in `src/web/assets/sidebar-resize.js`, width and handle layout in `docs.css`.
  Requirements: FR-970, FR-971, FR-972, FR-973, FR-974, UX-970, UX-971. Verify: TC-970 `cargo test --locked --lib web::assets::tests::sidebar_resize_assets`
- [x] T2 — Embed the script in `src/web/assets.rs` and serve it from `src/web/server.rs`.
  Requirements: FR-970. Verify: TC-970 `cargo test --locked --lib web::assets::tests::sidebar_resize_assets`
- [x] T3 — Drag, reload, keys, clamp, and the narrow drawer.
  Requirements: FR-970, FR-971, FR-972, FR-973, FR-974, UX-970. Verify: TC-971, TC-972 `cargo test --locked --test web_sidebar_resize -- --ignored`
