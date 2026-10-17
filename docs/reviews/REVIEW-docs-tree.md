# REVIEW: docs-tree

Spec: [SPEC-docs-tree](../specs/SPEC-docs-tree.md)
Plan: [PLAN-docs-tree](../plans/PLAN-docs-tree.md)

## Spec compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-830 | Met | `tree_children_lists_one_level` (ordering dirs then files, non-ignored entries, symlink containment, `has_children` correctness); `tree_api_confined_and_bounded` (truncation at bound) |
| FR-831 | Met | `tree_api_confined_and_bounded` (root/subdir 200, `../` and absolute 403, escaping dir/file symlink 403, missing/ignored 404, file 400); `docs_routes_require_auth` (401 without token) |
| FR-832 | Met | `docs_tree_sidebar_assets` (lazy tree fetch, cache invalidation on refresh and turn changes, roving tabindex, filter tree rendering); `docs_tree_renders_in_headless_chrome` (e2e click and expansion) |
| FR-833 | Met | `docs_tree_sidebar_assets` (sidebar container, tabs, `localStorage` persistence keyed per workspace, header toggle button) |
| UX-830 | Met | `docs_tree_sidebar_assets` (`role="tree"`, `role="treeitem"`, `aria-expanded`, `aria-owns`, `@media (max-width: 900px)` drawer, Arrow keys and Enter handlers); `docs_tree_renders_in_headless_chrome` |

## Deviations and decisions

- Closed `.sidebar` is hidden via `visibility: hidden;` and receives `inert` in DOM so focus cannot leak into offscreen drawer controls while closed.
- At narrow widths (`<= 900px`), opening the sidebar moves focus directly into the sidebar (`#docs-filter` or close button), and closing the sidebar returns focus to `#sidebar-toggle-files`.
- Directory `has_children` calculation in `list_children` is deferred until after slice truncation, avoiding expensive recursive inspections on items that are not returned.
- Directory treeitems use `aria-owns` pointing to their child group container (`<div role="group">`), satisfying ARIA 1.2 treeitem ownership hierarchy without breaking row flexbox styling.
- Sequential counter-based IDs (`tree-group-${++treeIdSeq}`) are used for `aria-owns` targets to eliminate DOM id collisions across folders.
- Sidebar state is deferred until snapshot arrival so toggles prior to first snapshot do not persist under `'default'`.

## Test evidence

Environment: Linux.

- `node --check src/web/assets/*.js`: clean.
- `cargo fmt --check`: clean.
- `cargo clippy --locked --all-targets -- -D warnings`: clean.
- `cargo test --locked --all-targets`: 540 passed, 0 failed, 10 ignored.
- `cargo test --locked --test web_docs_preview -- --ignored`: 2 passed (`docs_preview_renders_in_headless_chrome`, `docs_tree_renders_in_headless_chrome`).

## Residual risks

- Headless Chrome tests require Playwright and Chromium installed on the host. In deterministic CI environments without Chromium, the test remains `#[ignore]` and unit/asset tests provide deterministic regression protection.

## Cross-review: AGY (gemini-3.8-flash-high): APPROVE

No blockers; FR-830..FR-833, UX-830, and TC-830..TC-834 are satisfied.

### Round 1 Review Findings

| Nit | Resolution |
| --- | --- |
| Loading/error hint rows used class `tree-item`, entering roving tabindex | Replaced with `.tree-hint` class; excluded from tabindex |
| `role="group"` placement under ARIA | Directory treeitems reference child group via `aria-owns` attribute |
| `dir_has_children` computed eagerly for all entries before truncation | Computed only for directory entries retained in the final slice after truncation |
| `DocKind` `PartialEq<&str>` and `PartialEq<str>` in production code | Moved to `#[cfg(test)] mod tests` block in `src/docs/mod.rs` |
| Dead `#docs-list` code (`refreshList`/`renderList` list rendering and `.docs-list` CSS) | Removed dead `#docs-list` list rendering and CSS while keeping cache/refresh and `/api/docs` filter retrieval |
| Sidebar workspace key initialized to 'default' before snapshot | Keyed off snapshot workspace; restores open state and expanded folders on snapshot arrival |
| Closed sidebar remaining tabbable | Made `visibility: hidden` and `inert` when closed; focus moves into drawer at `<=900px` and returns to toggle on close |
| Directory symlink escaping workspace in `tree_api_confined_and_bounded` | Added assertions that escaping `linkdir` is omitted from root and returns 403 on `/api/docs/tree?dir=linkdir` |
| Tautological assertions in `docs_tree_sidebar_assets` | Asserted specific strings: `role="tree"`, `role="treeitem"`, `aria-expanded`, `aria-controls="sidebar"`, `@media (max-width: 900px)`, and `ArrowDown`/`ArrowRight`/`ArrowLeft` handlers |

### Round 2 Review Findings

| Item | Type | Finding | Resolution |
| --- | --- | --- | --- |
| 1 | Blocker | `ticketUrl` deleted during dead-code cleanup, causing ReferenceError in preview iframes and download links | Restored `function ticketUrl` verbatim in `src/web/assets/docs.js` and added assertion in `docs_tree_sidebar_assets` (`src/web/assets.rs`); verified no other deleted identifiers referenced |
| 2 | Blocker | `refreshChanges` cleared `treeDirCache` and re-rendered on every 10s poll and snapshot, losing focus and refetching all expanded folders | Added signature comparison of paths+timestamps (`changesSignature`); invalidated cache and triggered re-render only when changes payload changed; preserved keyboard focus (`getFocusedTreePath` / `restoreFocusToPath`); avoided `replaceChildren` when entries/turns unchanged |
| 3 | Blocker | TC-833 `role="treeitem"` assertion passed on artificial comments | Removed artificial comments from `docs.js`; updated `docs_tree_sidebar_assets` to assert on real code (`setAttribute('role', 'treeitem')`, `setAttribute('aria-expanded'`, `localStorage.getItem('dmuxai.sidebar.`, arrow keys, Enter, `ticketUrl`) |
| 4 | Nit | `treeDirCache` not cleared on workspace change in `applyWorkspaceSnapshot` | Added `treeDirCache = {};` on workspace switch |
| 5 | Nit | Unused `fmtSize` remaining in `docs.js` | Deleted `fmtSize` |
| 6 | Nit | Path sanitization for `aria-owns` group IDs risked collisions | Switched to sequential counter IDs (`tree-group-${++treeIdSeq}`) |
| 7 | Nit | Sidebar toggles persisted before first snapshot under `'default'` key | Deferred saving until active workspace is known (`pendingOpen`/`pendingExpanded`) |
