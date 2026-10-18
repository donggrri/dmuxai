# SPEC: sidebar-resize

Status: Implemented (v0.2.2)
Created: 2026-10-18
Related: Dongjin 2026-10-18 KST ("sidebar 크기 조절할 수 있어야한다", "사이드바 크기 조절 v0.2.2에 넣어"); SPEC-session-sidebar (left Sessions sidebar), SPEC-docs-tree UX-830 and SPEC-todo-sidebar (right Files/To-dos sidebar), SPEC-centered-prompt (dock variables, centered prompt)

## 1. Decision summary

Both docked dashboard sidebars are a fixed 320px: the left Sessions sidebar (`#sessions-sidebar`) and the right Files/To-dos sidebar (`#sidebar`). Long file paths and session titles get cut off, and on a wide screen the user cannot give the sidebars more room.

Decision: each docked sidebar gets a drag handle on its inner edge (the right edge of the left sidebar, the left edge of the right sidebar). Dragging sets the sidebar width, which is clamped to a minimum and a maximum. The width drives the existing `--left-dock` / `--right-dock` variables, so the prompt box stays centered in the remaining space (SPEC-centered-prompt FR-850). The width persists in `localStorage`, a double-click resets it to the default, and the handle is keyboard accessible as an ARIA `separator`. Open/close (collapse) keeps working as before. At 900px and below the sidebars stay fixed-width overlay drawers and the handles are hidden.

The graph node detail pane (`#graph-detail`) is part of the Graph panel, not a docked sidebar, and is out of scope.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-970 | Must | On viewports wider than 900px, each docked sidebar has a resize handle on its inner edge: `#sessions-resize` (right edge of `#sessions-sidebar`) and `#sidebar-resize` (left edge of `#sidebar`). Dragging a handle with the pointer (mouse, pen or touch; pointer capture) sets that sidebar's width live. The sidebar width comes from the CSS variables `--left-sidebar-width` / `--right-sidebar-width` (default 320px) | TC-970, TC-971 |
| FR-971 | Must | Each width is clamped to [180px, min(50% of the viewport width, viewport width - other open sidebar width - 360px)], but never below 180px. The clamp is re-applied when the window is resized and when the other sidebar opens | TC-970, TC-972 |
| FR-972 | Must | While a sidebar is open on a wide viewport, its dock variable (`--left-dock` / `--right-dock`) equals its current width, so the prompt box (`.session-prompt`) stays centered (within 2px) in the space between the open sidebars and is never overlapped by them (keeps SPEC-centered-prompt FR-850 for any widths) | TC-971, TC-972 |
| FR-973 | Must | The width persists across reloads in `localStorage` under `dmuxai.sidebar.width.left` and `dmuxai.sidebar.width.right` (global, not per workspace). A missing, non-numeric or out-of-range stored value falls back to the default or is clamped. Double-clicking a handle resets that sidebar to the default 320px and removes the stored value | TC-970, TC-971 |
| FR-974 | Must | Collapsing and reopening a sidebar (header toggles, close buttons) still works and keeps the chosen width. A closed sidebar reserves no space. At 900px and below the handles are hidden (`display: none`), the sidebars are overlay drawers of `min(320px, 85vw)` as today, and stored widths have no effect | TC-970, TC-972 |
| UX-970 | Must | Each handle is a focusable element (`tabindex="0"`) with `role="separator"`, `aria-orientation="vertical"`, `aria-controls` naming its sidebar, an `aria-label`, and `aria-valuenow` / `aria-valuemin` / `aria-valuemax` in px kept in sync with the width. Left/Right arrows resize by 16px (Shift: 64px) in the natural direction (Right widens the left sidebar and narrows the right one), Home/End go to min/max, Enter resets to the default. The handle has a visible hover and `:focus-visible` style using theme colors (light and dark), a `col-resize` cursor, and a hit area of at least 6px | TC-970, TC-971 |
| UX-971 | Should | While dragging, the dock/width transition is disabled so the column follows the pointer, and text selection is suppressed (`body.sidebar-resizing`). The existing 0.2s open/close transition and the `prefers-reduced-motion` behaviour are unchanged | TC-970 |

### Design notes

- One small script (`assets/sidebar-resize.js`, served like the other assets) owns both handles. It sets `--left-sidebar-width` / `--right-sidebar-width` on `document.documentElement`; CSS uses them for `.sessions-sidebar` / `.sidebar` widths and for `--left-dock` / `--right-dock` under `body.left-sidebar-open` / `body.sidebar-open` above 900px. Open/close stays owned by `session.js` and `docs.js`.
- SPEC-centered-prompt FR-851 (both sidebars 320px) becomes "default 320px"; FR-850 centering holds for any width.
- The handle sits inside its sidebar (absolutely positioned on the inner edge), so it is hidden and inert together with a closed sidebar.

### Implementation decisions

- Widths are whole pixels. A pointer position is rounded, then clamped.
- `localStorage` is written on pointer-up and after a handled key, and the key is removed on Enter or a double-click reset. Resizing the window, or the other sidebar opening or closing, reclamps the displayed width and the ARIA values and does not rewrite storage. A non-numeric stored value is ignored (default 320). A numeric value outside the current range stays stored and is clamped only for display, so a wide preference survives a narrower window.
- A closed sidebar counts as zero width for the other sidebar's maximum. Its own preferred width is clamped only by the solo maximum (half the viewport, and the viewport minus 360px) until it opens. When both are open and both preferences do not fit, the left sidebar is clamped first, then the right, for a few passes.
- At 900px and below the script does not set the width variables and does not touch storage. The existing narrow rule keeps the drawers at `min(320px, 85vw)` and hides the handles.
- A second pointer-up within 500ms, with movement of 3px or less, also resets that sidebar. Pointer capture can suppress the `dblclick` event; the `dblclick` listener still resets.
- Above 900px, an open sidebar sets its dock variable to `var(--left-sidebar-width)` or `var(--right-sidebar-width)`. The defaults stay 320px. `prompt_centering_assets` checks those variable references.

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-970 | FR-970, FR-971, FR-973, FR-974, UX-970, UX-971 | unit (asset) | index.html, JS and CSS assets | Both handles exist with `role="separator"`, `tabindex="0"`, `aria-controls`, `aria-valuenow/min/max`; the script is linked and served; CSS uses `--left-sidebar-width` / `--right-sidebar-width` for widths and docks, hides handles at max-width 900px, has `col-resize`, `:focus-visible` and `sidebar-resizing` rules; the JS has the storage keys, the 180px minimum, the 50% cap, dblclick reset and the arrow/Home/End/Enter keys | `src/web/assets.rs` `sidebar_resize_assets` |
| TC-971 | FR-970, FR-972, FR-973, UX-970 | e2e (ignored, headless Chrome) | `dmuxai serve` on a temp workspace, 1440x900 | Dragging each handle changes the sidebar width and `aria-valuenow`; the prompt box stays centered (±2px) between the open sidebars and does not overlap them; the width survives a reload; ArrowRight/ArrowLeft on the focused handle change the width by 16px; double-click resets to 320px and clears storage | `tests/web_sidebar_resize.rs` `sidebar_resize_in_headless_chrome` |
| TC-972 | FR-971, FR-972, FR-974 | e2e (ignored, headless Chrome) | same server | Dragging far past the limits clamps to 180px and to the max; closing and reopening keeps the width and a closed sidebar reserves no space; at 800px wide the handles are hidden and the drawer width is `min(320px, 85vw)` even with a stored width; the centered-prompt check still passes | `tests/web_sidebar_resize.rs` `sidebar_resize_clamps_and_narrow_layout` |
