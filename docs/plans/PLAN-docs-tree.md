# PLAN: docs-tree

Spec: [SPEC-docs-tree](../specs/SPEC-docs-tree.md)
Tasks: [TASKS-docs-tree](../tasks/TASKS-docs-tree.md)

## Order of work

1. `src/docs/list.rs`: `TreeEntry`, `TreeList`, `list_children(ws, dir, limit)`
   with directory confinement, symlink filtering, ignored directory omission,
   and deferred `has_children` calculation after truncation (FR-830, FR-831).
2. `src/web/docs_api.rs`: `GET /api/docs/tree?dir=...` router wiring, query
   decoding, authentication enforcement, and confinement error mappings (FR-830, FR-831).
3. `src/web/assets/index.html`: Header `Files` toggle button, right-hand `#sidebar`
   container with tabs and files tree container `#docs-tree`, and backdrop element (FR-833, UX-830).
4. `src/web/assets/docs.css`: Right-hand `.sidebar` positioning, open/closed transform,
   `visibility: hidden` when closed, overlay drawer styles at `<=900px`, and `.tree-hint` styling (UX-830).
5. `src/web/assets/docs.js`: Collapsible tree lazy fetching (`/api/docs/tree`),
   per-workspace sidebar state keyed off snapshot/state workspace, focus management
   for narrow width and toggle restoration, cache invalidation on turn snapshot changes,
   `aria-owns` hierarchy, and roving tabindex (FR-832, FR-833, UX-830).
6. Tests and verification: `src/docs/list.rs` (TC-830), `src/web/docs_api.rs` (TC-831, TC-832),
   `src/web/assets.rs` (TC-833), and headless Chrome E2E test `tests/web_docs_preview.rs` (TC-834).

## Risks

- Confinement: Symlinked directories pointing outside the workspace or inside ignored
  folders must not leak. Confined via `Workspace::resolve` and canonicalization checks.
- Scalability: Lazy expansion ensures large repositories only fetch one folder level at
  a time; child existence is computed only for entries within the page limit.
- Accessibility: Focus traps and tab navigation must be preserved; closed drawer must
  be `inert` and `visibility: hidden` so offscreen elements are not focusable.

## Rollback

Revert the right-hand sidebar container in `index.html` and router route in `docs_api.rs`.
The `/api/docs` flat list and document view/preview APIs remain intact.
