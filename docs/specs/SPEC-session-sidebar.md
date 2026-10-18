# SPEC: session-sidebar

Status: Draft (v0.2.2)
Created: 2026-10-18
Related: issue #55; Dongjin 2026-10-18 09:42 KST ("a left sidebar listing sessions like ChatGPT"); SPEC-dashboard-parity (session panel, `load_conversation`, `new_conversation`); SPEC-multi-instance-workspace (shared conversation list, live turns per process); SPEC-running-indicators (run state)

## 1. Decision summary

Today the conversation list sits in a small box to the right of the transcript inside the session panel (`.session-conversations`). It shows titles only, and each row has a `Load` button. The TUI has a conversation picker (F3, `/conversations`, `/resume`).

Decision: move the list into a collapsible **left sidebar**, like ChatGPT's. It shows every conversation in the workspace (the current one first, then by last update) with its title, last agent, relative time and a running spinner. Clicking a row switches to it; a `New` button starts a new conversation. It reuses the existing `load_conversation` and `new_conversation` session commands. The snapshot's conversation entries gain the fields the list needs. The TUI already has the equivalent (F3 picker), so it only gets the same agent, time and running marks.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-860 | Must | `SessionConversation` in the snapshot gains `last_agent` (runner name or null), `turn_count` and `running` (true when any turn of that conversation is in flight in this process or a live peer, per SPEC-multi-instance-workspace). The list stays bounded (at most 200 entries, newest first; the active conversation is always included) | TC-860, TC-861 |
| FR-861 | Must | The dashboard has a left sidebar (`#sessions-sidebar`, header toggle `Sessions`). It lists the conversations with title (or a short id), last agent, relative time ("3m ago", absolute time in `title`), and a spinner while `running`. The active conversation is highlighted with `aria-current="true"` | TC-862, TC-863 |
| FR-862 | Must | Clicking a row (or Enter/Space on it) sends `load_conversation`; the `New` button sends `new_conversation`. Both are disabled in view-only mode, with the same hint as other actions. Switching while a turn runs is allowed; the running turn keeps going in its conversation, as in the TUI | TC-862, TC-863 |
| FR-863 | Must | The sidebar is collapsible. Its open state persists per workspace in `localStorage`, and it is open by default above 1200px. Below 900px it is an overlay drawer with a backdrop. A filter box narrows the list by title | TC-862, TC-863 |
| FR-864 | Should | The TUI conversation picker (F3, `/conversations`) shows the same last agent, relative time and running mark per row. No new TUI pane | TC-864 |
| UX-860 | Must | The list is a keyboard-navigable `nav` with `aria-label="Sessions"` (Up/Down move, Enter opens). The spinner follows `prefers-reduced-motion` and `[theme] reduced_motion` (SPEC-running-indicators UX-821). The old in-panel conversation box is removed, and `/conversations` in the palette focuses the sidebar | TC-862, TC-863 |

### Design notes

- `running` comes from the turn rows the multi-instance busy check already reads (in-flight turn of a live session), so no new table is needed. It is computed when the cached list is refreshed; the cache ages out after 2s while any entry is running.
- The `data-parity="conversations"` marker (SPEC-dashboard-parity TC-786, TC-787) moves with the list to the new sidebar.
- The left sidebar uses the same shell pattern as the right one (`.sidebar`, but docked on the left, 280px), so SPEC-centered-prompt can reserve its width.

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-860 | FR-860 | unit | `SessionConversation` serde | New fields serialize as `last_agent`, `turn_count` and `running`; old snapshots without them still deserialize (defaults) | `src/web/snapshot.rs` `session_conversation_fields_roundtrip` |
| TC-861 | FR-860 | unit | conversation engine with two conversations and one in-flight turn | The listed card for that conversation has `running = true` and its `last_agent`; the list is capped and keeps the active conversation | `src/app.rs` `session_conversations_report_running_and_agent` |
| TC-862 | FR-861..FR-863, UX-860 | unit (asset) | index.html, JS and CSS assets | `#sessions-sidebar` nav with `aria-label`, toggle with `aria-controls`/`aria-expanded`, `New` button, filter, spinner class, reduced-motion rule; the old `.session-conversations` box is gone | `src/web/assets.rs` `session_sidebar_assets` |
| TC-863 | FR-861..FR-863, UX-860 | e2e (ignored, headless Chrome) | `dmuxai serve` on a workspace with two stored conversations | The sidebar lists both with agent and time; clicking the other row makes it active (`aria-current`); `New` adds a row; collapse persists across reload; screenshot captured | `tests/web_session_sidebar.rs` `session_sidebar_renders_in_headless_chrome` |
| TC-864 | FR-864 | unit (TUI render) | App with two conversations, one running | The picker rows contain the agent name, relative time and the running mark | `src/app.rs` `conversation_picker_shows_agent_time_running` |
