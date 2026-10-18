# REVIEW: session-sidebar

Spec: [SPEC-session-sidebar](../specs/SPEC-session-sidebar.md) (#55)
Implementer: AGY (`dmuxai run --agent agy --mode implement`, about 46 min, including the pty-based e2e). Maintainer fixes by hand. Reviewer: opencode-haiku (read-only).

## Maintainer checks before review

- The dashboard header was covered by the left sidebar (the brand and theme controls were hidden). The header now sits inside the dock padding; the #54 counter-margins are removed.
- The TUI picker's `● running` mark sat at the end of the row and was cut off at the picker width. It now leads the row (seen in a live tmux capture).

## Round 1 — REQUEST_CHANGES (3, fixed)

1. The JS row classes (`active`, `disabled`, `sessions-row-top`) did not match the CSS (`sessions-row-active`, `-disabled`, `-head`). Renamed in the JS.
2. The running spinner's `aria-label` sat on a bare span. Added `role="img"`.
3. A toggle made before the first snapshot was saved under `dmuxai.sessions.open.default`. It is now held and saved under the real workspace key.

## Round 2 — APPROVE

Non-blocking: if a snapshot ever reported the workspace as the literal `default`, the pending toggle would not be saved. Workspaces are absolute paths, so this cannot happen in practice.
