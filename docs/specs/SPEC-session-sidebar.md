# SPEC: session-sidebar

Status: Implemented (v0.2.2)
Created: 2026-10-18
Related: issue #55; Dongjin 2026-10-18 09:42 KST ("a left sidebar listing sessions like ChatGPT"); SPEC-dashboard-parity (session panel, `load_conversation`, `new_conversation`); SPEC-multi-instance-workspace (shared conversation list, live turns per process); SPEC-running-indicators (run state)

## 1. Decision summary

Today the conversation list sits in a small box to the right of the transcript inside the session panel (`.session-conversations`). It shows titles only, and each row has a `Load` button. The TUI has a conversation picker (F3, `/conversations`, `/resume`).

Decision: move the list into a collapsible **left sidebar**, like ChatGPT's. It shows every conversation in the workspace (the current one first, then by last update) with its title, last agent, relative time and a running spinner. Clicking a row switches to it; a `New` button starts a new conversation. It reuses the existing `load_conversation` and `new_conversation` session commands. The snapshot's conversation entries gain the fields the list needs. The TUI already has the equivalent (F3 picker), so it only gets the same agent, time and running marks.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-860 | Must | `SessionConversation` in the snapshot gains `last_agent` (runner name or null), `turn_count` and `running` (true when any turn of that conversation is in flight in this process or a live peer, per SPEC-multi-instance-workspace). The list stays bounded and ordered: the active conversation first, then the rest by `updated_at` descending, at most 200 entries in total | TC-860, TC-861 |
| FR-861 | Must | The dashboard has a left sidebar (`#sessions-sidebar`, header toggle `Sessions`). It lists the conversations with title (or a short id), last agent, relative time ("3m ago", absolute time in `title`), and a spinner while `running`. The active conversation is highlighted with `aria-current="true"` | TC-862, TC-863 |
| FR-862 | Must | Clicking a row (or Enter/Space on it) sends `load_conversation`; the `New` button sends `new_conversation`. Both are disabled in view-only mode, with the same hint as other actions. Switching while a turn runs is allowed; the running turn keeps going in its conversation, as in the TUI | TC-862, TC-863 |
| FR-863 | Must | The sidebar is collapsible. Its open state persists per workspace in `localStorage`, and it is open by default above 1200px. Below 900px it is an overlay drawer with a backdrop. A filter box narrows the list by title | TC-862, TC-863 |
| FR-864 | Should | The TUI conversation picker (F3, `/conversations`) shows the same last agent, relative time and running mark per row. No new TUI pane | TC-864 |
| FR-865 | Should | The F3 picker scales with the terminal: its width is about 90% of the terminal, at least 60 columns (or the whole terminal when it is narrower) and at most 160, and it is never wider than the content needs. When a row does not fit, the title is shortened with `…` so the running mark, short id, turn count, agent and relative time stay visible. At 120 and 200 columns the agent and time of a typical row are not cut off; at 80x24 the picker still fits inside the terminal | TC-865 |
| UX-860 | Must | The list is a keyboard-navigable `nav` with `aria-label="Sessions"` (Up/Down move, Enter opens). The spinner follows `prefers-reduced-motion` and `[theme] reduced_motion` (SPEC-running-indicators UX-821). The old in-panel conversation box is removed, and `/conversations` in the palette focuses the sidebar | TC-862, TC-863 |

### Design notes

- `running` comes from the turn rows the multi-instance busy check already reads (in-flight turn of a live session), so no new table is needed. It is computed when the cached list is refreshed; the cache ages out after 2s while any entry is running.
- The `data-parity="conversations"` marker (SPEC-dashboard-parity TC-786, TC-787) moves with the list to the new sidebar.
- The left sidebar uses the same shell pattern as the right one (`.sidebar`, but docked on the left, 320px like the right one), so SPEC-centered-prompt can reserve its width.

### Implementation decisions

- `ConversationCard` (`src/conversation.rs`) computes `running: bool` via an `EXISTS` subquery on `turns.status IN ('queued', 'running')`, matching `agent_is_busy`.
- `SessionConversation` (`src/web/snapshot.rs`) carries `#[serde(default)]` on `last_agent`, `turn_count` and `running` so older snapshots deserialize cleanly.
- Per-workspace sidebar open state is persisted in `localStorage` under `dmuxai.sessions.open.<workspace>` (`'1'` or `'0'`), defaulting to open when `window.innerWidth > 1200`. Toggling syncs `body.left-sidebar-open` so SPEC-centered-prompt reserves `--left-dock: 320px` above 900px.
- Because `dmuxai serve` is view-only (`load_conversation` and `new_conversation` disabled), `TC-863` launches interactive `dmuxai --web --port <port>` under a pseudo-terminal (`pty.openpty()`) after seeding conversations with `dmuxai run`.
- `running` reflects queued or running turn rows. A turn left `running` by a crashed peer stays marked until the startup recovery of SPEC-multi-instance-workspace resets it.
- The dashboard header sits inside the dock padding too, wrapping when narrow, so the left sidebar never covers the brand and controls. This replaces the full-width header counter-margins from #54.
- The TUI picker puts `● running` at the start of the row, so a narrow picker never cuts it off.
- The picker width (FR-865) is `clamp(90% of the terminal, 60, 160)`, capped by the widest row and by the terminal width. The title is the only part that is shortened.
- A sidebar toggle made before the first snapshot names the workspace is held and saved under the real workspace key, never under a placeholder key.

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-860 | FR-860 | unit | `SessionConversation` serde | New fields serialize as `last_agent`, `turn_count` and `running`; old snapshots without them still deserialize (defaults) | `src/web/snapshot.rs` `session_conversation_fields_roundtrip` |
| TC-861 | FR-860 | unit | conversation engine with two conversations and one in-flight turn | The listed card for that conversation has `running = true` and its `last_agent`; the list is capped, keeps the active conversation first and orders the rest by `updated_at` descending | `src/app.rs` `session_conversations_report_running_and_agent` |
| TC-862 | FR-861..FR-863, UX-860 | unit (asset) | index.html, JS and CSS assets | `#sessions-sidebar` nav with `aria-label`, toggle with `aria-controls`/`aria-expanded`, `New` button, filter, spinner class, reduced-motion rule; the old `.session-conversations` box is gone | `src/web/assets.rs` `session_sidebar_assets` |
| TC-863 | FR-861..FR-863, UX-860 | e2e (ignored, headless Chrome) | `dmuxai --web` on a workspace with two stored conversations | The sidebar lists both with agent and time, active first; clicking the other row makes it active (`aria-current`); `New` adds a row; collapse persists across reload; screenshot captured | `tests/web_session_sidebar.rs` `session_sidebar_renders_in_headless_chrome` |
| TC-864 | FR-864 | unit (TUI render) | App with two conversations, one running | The picker rows contain the agent name, relative time and the running mark | `src/app.rs` `conversation_picker_shows_agent_time_running` |
| TC-865 | FR-865 | unit (TUI render) | App with a running conversation that has a long title, rendered at 80x24, 120x40 and 200x50 | The picker fits inside the terminal; at 120 and 200 columns it is wider than the old fixed 76 columns; every row ends with the agent and relative time inside the border; an over-long title is shortened with `…` | `src/app.rs` `conversation_picker_scales_with_terminal_width` |
