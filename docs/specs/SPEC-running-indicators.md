# SPEC: running-indicators

Status: Implemented (v0.2.1; FR-981, FR-982 in v0.2.2)
Created: 2026-10-17
Related: v0.2.2 dogfood 2026-10-19 (FR-981, FR-982; [PLAN-v022-dogfood](../plans/PLAN-v022-dogfood.md)); Dongjin dogfood 2026-10-17 23:38/23:41 (`dmuxai --web` on Linux: "running state is not clearly visible"; wants a spinner in both TUI and web)

## 1. Decision summary

In v0.2.0 a running turn shows only as a status word (`WORKING`) and a once-per-second elapsed counter. The first token takes 3–10 s, and during that wait nothing moves. In the dashboard, a busy agent is a small chip-state change.

Decision: one shared run-state model (`idle | running | done | failed | cancelled`, with start time, end time and the current graph node), rendered as:

- an animated spinner with elapsed time while running;
- a clear done/failed/cancelled mark when the turn ends.

It applies to the TUI and the dashboard, and respects `NO_COLOR` and `prefers-reduced-motion`.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-820 | Must | Each agent has a run state: `idle`, `running` (with `started_at`), `done`, `failed` or `cancelled` (with `ended_at` and duration). The state is set from turn start/complete/error/cancel events and is exposed in `DashboardSnapshot` agents and in `dmuxai status --json` | TC-820 |
| FR-821 | Must | TUI: while the selected or any agent is running, the status bar shows a braille spinner (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`) next to the agent name with the elapsed time (`⠹ cursor 12s`). The in-progress assistant message shows the same spinner line until its first text arrives. The spinner advances at about 10 fps and redraws only while something is running (no idle CPU) | TC-821, TC-822 |
| FR-822 | Must | TUI: when a turn ends, the spinner is replaced by `✓ done 14s`, `✗ failed 3s` or `■ cancelled`, shown until the next turn starts | TC-821 |
| FR-823 | Must | Web: the header shows a global status pill: `Idle`, `Running · <agent> · 12s` with a CSS spinner, `Done`, `Failed` with the error tone, or `Cancelled`. Each agent chip shows the spinner and elapsed time while running and a ✓/✗ mark after. The active graph node card and graph-view node show the spinner. Elapsed time ticks client-side from `started_at` | TC-823, TC-824 |
| FR-824 | Should | Web: while anything is running, the browser tab title is prefixed with `⏳` and the favicon is swapped for a running dot. On done or failed, the prefix becomes `✓` or `✗` until the next turn starts or the tab regains focus | TC-824 |
| UX-820 | Must | Idle, running, done and failed are told apart by symbol and text, not by color alone. Under `NO_COLOR` the TUI still shows the spinner glyphs and marks | TC-821 |
| UX-821 | Must | Reduced motion: the TUI accepts `[theme] reduced_motion = true` or `DMUXAI_REDUCED_MOTION=1`, and the web follows `prefers-reduced-motion: reduce`. Either way the spinner becomes a static `●`/`…` running mark while elapsed time still updates | TC-822, TC-823 |
| NFR-820 | Should | The spinner adds no dashboard publish traffic beyond the existing busy interval. The snapshot carries `started_at`, not frames or elapsed time, so it is identical across ticks while nothing else changes. Frames and elapsed time are rendered client-side | TC-820, TC-823 |
| FR-981 | Must | Dashboard parity for the activity detail: while an agent is running, `DashboardSnapshot` `agents[].activity` carries the same bounded detail the TUI status bar shows (latest tool or progress line, for example `▸ read src/app.rs`), or null when there is none. The agent chip and the header `Running` pill show it after the elapsed time. It is cleared when the turn ends | TC-982, TC-983 |
| FR-982 | Must | Cancel notice, all runners: when a running turn is cancelled (TUI Ctrl+C, dashboard cancel, `/api/command cancel`), the visible transcript gets exactly one system line `<agent> turn cancelled.` after the partial reply, in the TUI and in the dashboard transcript. The stored turn status becomes `cancelled`, so reopening the conversation shows the same line. A later `Cancelled` event from the adapter for the same turn adds no second line | TC-984, TC-985 |

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-820 | FR-820, NFR-820 | unit | App with fake agent events | Start → running with `started_at`; complete → done with duration; error → failed; cancel → cancelled; the snapshot carries them, and two snapshots taken 2 s apart while running are equal (NFR-820) | `src/app.rs` `run_state_transitions_follow_turn_events` |
| TC-821 | FR-821, FR-822, UX-820 | unit (TestBackend) | busy agent, then completed and failed turns, with `NO_COLOR` | The status bar contains a braille frame and `12s`; after completion `✓ done`, after failure `✗ failed`; glyphs are present without color | `src/app.rs` `status_bar_spinner_and_end_marks` |
| TC-822 | FR-821, UX-821 | unit | busy agent, injected clock | The frame advances with time; reduced motion gives a static mark; there are no redraw ticks while idle | `src/app.rs` `spinner_frames_and_reduced_motion` |
| TC-823 | FR-823, UX-821, NFR-820 | unit (asset) | index.html, session.js and css assets | Header status pill element, `.spinner` CSS with a `prefers-reduced-motion` override, a client-side elapsed ticker, run-state rendering for chips and the graph node | `src/web/assets.rs` `dashboard_has_run_status_indicators` |
| TC-824 | FR-823, FR-824 | e2e (ignored, headless Chrome) | demo snapshot with one running agent | Pill reads `Running`; chip has the spinner; title starts with `⏳`; `#favicon` href points at `favicon-running.svg`; screenshot captured | `tests/web_running.rs` `running_indicator_renders_in_headless_chrome` |
| TC-982 | FR-981 | unit | App with a running agent that received `Activity(Tool)` and then `Completed` | `agents[].activity` is `▸ read src/app.rs` while running and null after completion; the TUI status bar shows the same text | `src/app.rs` `dashboard_agent_activity_matches_tui_status` |
| TC-983 | FR-981 | unit (asset) | app.js and session.js assets | Chip and status pill render `agent.activity`; `computeAgentsKey` includes it | `src/web/assets.rs` `dashboard_shows_agent_activity_detail` |
| TC-984 | FR-982 | unit | App for each runner (codex, cursor, agy, custom) with a running turn and partial text, then `cancel_selected`, then the adapter's late `Cancelled` | Exactly one `<agent> turn cancelled.` system message after the partial agent message; the dashboard snapshot transcript has it; the stored turn status is `cancelled` and a reopened conversation shows the line | `src/app.rs` `cancel_appends_one_cancelled_notice_for_every_runner` |
| TC-985 | FR-982 | unit | App with the dashboard hub, running turn, `SessionCommand::Cancel` | Snapshot transcript ends with the cancelled system line | `src/app.rs` `dashboard_cancel_shows_cancelled_notice` |
