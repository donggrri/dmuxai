# REVIEW: todo-sidebar

Spec: [SPEC-todo-sidebar](../specs/SPEC-todo-sidebar.md)
Reviewer: opencode-haiku (read-only, `--mode ask`). Implementer: hand-coded (AGY quota exhausted).

## Round 1 — REQUEST_CHANGES

- Accessibility (UX-840): `aria-label` on the plain `<span id="todos-count">` is ignored, so the button name never announced the open count.
  Fixed: the badge is `aria-hidden`, and `updateTodoCount` sets the button's `aria-label` to `To-dos, N open`. Pinned by TC-840 and asserted in TC-842.
- No XSS, lost-input or persistence issues found: to-do text uses `textContent`, ticket links are http/https only, the inline editor survives re-renders, and Esc skips text fields and visible modals.

## Round 2 — APPROVE

Non-blocking note: the static label reads `0 open` until the first snapshot renders, which matches the initial badge.
