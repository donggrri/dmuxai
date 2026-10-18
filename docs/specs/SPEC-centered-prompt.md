# SPEC: centered-prompt

Status: Draft (v0.2.2)
Created: 2026-10-18
Related: issue #54; Dongjin 2026-10-18 09:42 KST ("keep the prompt input box centered"); SPEC-docs-tree UX-830 (right sidebar), SPEC-session-sidebar (left sidebar)

## 1. Decision summary

The chat column (`main`, max 880px) is centered with `margin: auto`. When the right sidebar opens on a wide screen, `main` gets `margin-right: 320px`, so the column, and the prompt box with it, moves left and is no longer centered. The left sessions sidebar (SPEC-session-sidebar) would make this worse.

Decision: the layout reserves space for each open docked sidebar. The chat column, and so the prompt box, is centered in the space between them, and is never covered by a docked sidebar. With no sidebars, or with both open (equal widths), that is the viewport center. Overlay drawers (narrow screens) do not move the column. A headless Chrome test measures the geometry, so the centering cannot regress silently.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-850 | Must | On viewports wider than 900px, the prompt box (`.session-prompt`) is horizontally centered in the content area: the viewport minus each open docked sidebar (left sessions, right Files/To-dos). Its center is within 2px of the content area's center, and no docked sidebar overlaps it. This holds for the four states: none, left only, right only, both | TC-850, TC-851 |
| FR-851 | Must | With no sidebar, or with both sidebars open, the prompt box center is within 2px of the viewport center at widths 1280, 1440 and 1920 | TC-851 |
| FR-852 | Must | At 900px and below, sidebars are overlay drawers. The prompt box stays centered in the viewport (within 2px) and keeps a width of at least min(100% - 20px, 320px) | TC-851 |
| UX-850 | Should | Opening or closing a sidebar moves the column with the existing 0.2s transition (none under `prefers-reduced-motion`). The prompt keeps focus and its text | TC-850, TC-851 |

### Design notes

- Layout: `body` classes (`left-sidebar-open`, `sidebar-open`) set the CSS custom properties `--left-dock` and `--right-dock` (0, or the docked sidebar width) above 900px. A content wrapper is padded by `--left-dock` and `--right-dock`, and `main` keeps `max-width: 880px; margin-inline: auto` inside it. Centering then comes from one rule instead of per-state margins.
- The prompt box keeps its place in the session panel; this SPEC does not make it sticky.

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-850 | FR-850, UX-850 | unit (asset) | CSS assets | Dock variables and the centering rule exist; the old `margin-right: 320px` per-state rule is gone; reduced-motion disables the transition | `src/web/assets.rs` `prompt_centering_assets` |
| TC-851 | FR-850..FR-852, UX-850 | e2e (ignored, headless Chrome) | `dmuxai serve` on a temp workspace | For the widths 1280, 1440, 1920 and 800, and the four sidebar states, the measured `getBoundingClientRect()` of `.session-prompt` is centered (±2px) in the content area and does not intersect an open docked sidebar; typed prompt text survives the toggles | `tests/web_prompt_center.rs` `prompt_stays_centered_in_headless_chrome` |
