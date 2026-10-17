# SPEC: docs-tree

Status: Draft (v0.2.1)
Created: 2026-10-17
Related: Dongjin dogfood 2026-10-17 23:38 ("Documents shows every file flat"); SPEC-docs-preview (FR-741..FR-759)

## 1. Decision summary

The dashboard Documents panel (SPEC-docs-preview FR-741) lists every workspace file as one flat, sorted list. In a real repo that is hundreds of rows.

Decision:

- Add a lazy, per-directory listing API.
- Render the files as a collapsible folder tree in a **right-hand sidebar**: toggleable, with its width persisted.
- Preview and edit keep using the existing `/api/docs/file`, `/api/docs/render` and view-ticket routes.
- Confinement (`docs::confine`), `IGNORED_DIRS`, the symlink rules and the caps are unchanged and reused.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-830 | Must | `GET /api/docs/tree?dir=<workspace-relative dir>` (empty = root) returns the immediate children `{name, path, type: "dir"\|"file", kind, size, mtime_ms, editable, has_children}`, with directories first and then files, each sorted by name. Ignored directories are omitted. Symlinks are listed only when they resolve inside the workspace, and symlinked directories are never expanded. The listing is capped at `MAX_LIST_ENTRIES` children with `truncated` | TC-830, TC-831 |
| FR-831 | Must | `dir` goes through the same confinement as file reads. Absolute paths, `..` escapes, ignored dirs, and symlinks leaving the workspace return 400/403 using the existing docs error shapes. The route requires dashboard auth like the other docs routes | TC-831, TC-832 |
| FR-832 | Must | Web: the Documents view is a right sidebar with a collapsible tree. Folders expand lazily (one request per first expand, then cached until refresh or a docs-change event invalidates that dir). Clicking a file opens the existing preview/edit pane. The path filter still works: it searches the flat list endpoint and shows the matches with their parent folders expanded | TC-833, TC-834 |
| FR-833 | Should | Expanded folders and the sidebar open/closed state persist in `localStorage` per workspace. A header button `Files` toggles the sidebar | TC-833 |
| UX-830 | Must | The tree is keyboard accessible (`role="tree"`/`treeitem`, `aria-expanded`, Up/Down/Left/Right/Enter) and readable at narrow widths: below 900px the sidebar becomes an overlay drawer | TC-833, TC-834 |

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-830 | FR-830 | unit | temp workspace with nested dirs, `target/`, `.git/`, an in-workspace and an escaping symlink | Children of root and of a subdir have the right order, types, `has_children` and omissions | `src/docs/list.rs` `tree_children_lists_one_level` |
| TC-831 | FR-830, FR-831 | integration | axum router + temp workspace | `/api/docs/tree` 200 for root and subdir; 400/403 for `../`, absolute, ignored and escaping symlink; cap sets `truncated` | `src/web/docs_api.rs` `tree_api_confined_and_bounded` |
| TC-832 | FR-831 | integration | router without token | 401 for `/api/docs/tree` | `src/web/docs_api.rs` `docs_routes_require_auth` (extended) |
| TC-833 | FR-832, FR-833, UX-830 | unit (asset) | index.html, docs.js, docs.css | Sidebar container with a toggle button, `role="tree"`, lazy-expand fetch of `/api/docs/tree`, `localStorage` persistence, keyboard handlers, a narrow-width drawer media query | `src/web/assets.rs` `docs_tree_sidebar_assets` |
| TC-834 | FR-832, UX-830 | e2e (ignored, headless Chrome) | demo workspace | Expanding a folder shows children; clicking a file opens the preview; screenshot captured | `src/web/docs_api.rs` `docs_tree_renders_in_headless_chrome` |
