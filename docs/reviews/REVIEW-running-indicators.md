# REVIEW: running-indicators

Spec: [SPEC-running-indicators](../specs/SPEC-running-indicators.md)
Plan: [PLAN-running-indicators](../plans/PLAN-running-indicators.md)
Tasks: [TASKS-running-indicators](../tasks/TASKS-running-indicators.md)

## Spec compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-820 | Met | `run_state_transitions_follow_turn_events` asserts idle -> running (`started_at_ms`) -> done/failed/cancelled with duration; `status_json_maps_pending_and_stalled_to_running_preserving_start_time` verifies `events.jsonl` mapping |
| FR-821 | Met | `status_bar_spinner_and_end_marks` and `spinner_frames_and_reduced_motion` verify braille spinner at ~10 fps with elapsed time and no idle redraw |
| FR-822 | Met | `status_bar_spinner_and_end_marks` verifies `✓ done 12s`, `✗ failed 3s`, and finish without running entry omitting elapsed (`✓ done` without `0s`) |
| FR-823 | Met | `dashboard_has_run_status_indicators` in `src/web/assets.rs` asserts `#run-status`, `.spinner`, `station-spinner`, in-place ticker, and reduced motion styles; verified in Chrome e2e |
| FR-824 | Met | `running_indicator_renders_in_headless_chrome` verifies title prefix `⏳`, same-origin SVG favicon swapped to `favicon-running.svg` under `default-src 'self'` CSP, and screenshot capture |
| UX-820 | Met | Symbols (`⠋`, `✓`, `✗`, `■`) and explicit words used together with colors; verified with `color_enabled = false` in `status_bar_spinner_and_end_marks` |
| UX-821 | Met | TUI renders static `●` and disables fast redraw when reduced motion is configured; `theme_reduced_motion_config_and_env_var_are_honored` tests config and env var; web uses `@media (prefers-reduced-motion: reduce)` with static mark |
| NFR-820 | Met | Snapshots carry only `started_at_ms`; elapsed time computed client-side with 1s ticker; snapshots taken 2s apart during a run are identical |

## Findings and resolutions

1. **Graph node jumping during animation**:
   - *Finding*: `app.js` placed `.spinner` on `<g>` elements that already had `transform="translate(x, y)"`. CSS keyframe rotation replaced the transform attribute entirely, collapsing translation and causing nodes to jump to `(0, 0)`.
   - *Resolution*: Moved rotation to a dedicated child element `<g class="station-spinner">` with `transform-box: fill-box; transform-origin: center; animation: spin 0.8s linear infinite;`. Under reduced motion, animation is disabled, preserving static ring and center dot.
2. **Animation restarts from `replaceChildren`**:
   - *Finding*: `tickDashboard` invoked `replaceChildren` every second on `#run-status` and `#agents`, destroying DOM nodes and restarting the CSS animation clock.
   - *Resolution*: Wrapped elapsed text in dedicated elements (`.run-status-text` and `.chip-run-elapsed`) and updated their `textContent` in place. Full replacement only occurs when the agent set or execution state transitions.
3. **Agent status mapping for pending/stalled**:
   - *Finding*: `status_json` in `src/agent_api.rs` only mapped `started` and `running` to `running`, losing start time when pending or stalled events arrived.
   - *Resolution*: Mapped `pending` and `stalled` to `running` while retaining the original `started_at_ms`.
4. **Headless Chrome e2e test robustness**:
   - *Finding*: Test skipped when Chrome binary was missing rather than failing explicitly when invoked via `--ignored`; WebSocket URL derivation relied on port splitting; screenshot existence was unasserted.
   - *Resolution*: Made missing Chrome panic with an actionable error; parsed WebSocket URL with URL path indexing; asserted screenshot exists and has non-zero byte size.
5. **Favicon swap under strict CSP**:
   - *Finding*: Embedded SVG assets `/assets/favicon-*.svg` allow dynamic `<link rel="icon">` swapping under strict `default-src 'self'` CSP without requiring `data:` URIs.
   - *Resolution*: Embedded 4 SVG favicons (`idle`, `running`, `done`, `failed`), served with standard headers, and swapped `href` dynamically. Removed the deviation note from `SPEC-running-indicators.md`.
6. **TUI event loop non-busy behavior**:
   - The TUI main loop sleeps for `EVENT_LOOP_PAUSE = 5ms` on every iteration (`sleep(EVENT_LOOP_PAUSE).await`).
   - When idle, `needs_fast_redraw()` returns false, meaning `terminal.draw` only occurs on key/mouse events or agent lifecycle transitions.
   - When running, fast redraw is capped at 100 ms intervals (`Duration::from_millis(100)`). No busy looping occurs.

7. **Round 2 (Haiku review of round 1)**:
   - Reduced-motion overrides sat before the base graph spinner rules, so the graph spinner still spun. All overrides now live in one `@media (prefers-reduced-motion: reduce)` block at the end of `app.css`; `dashboard_has_run_status_indicators` asserts it follows the `.station-spinner` rule and sets `animation: none`.
   - `[theme] reduced_motion` / `DMUXAI_REDUCED_MOTION` now reach the web: `applySnapshotUi` toggles `html.reduced-motion` from `snapshot.ui.reduced_motion`.
   - `theme_reduced_motion_config_and_env_var_are_honored` holds an env lock while `DMUXAI_REDUCED_MOTION` is set.
   - `status_json` no longer carries a previous run's `node` into a new run that has none (regression assertion added).
   - Removed the unused `/favicon.ico` route; TC-824 oracle now includes the favicon href.

8. **Round 3 (opencode-haiku review)**:
   - Summary turns (`/summarize`, automatic summaries) set `Running` but their completion, error, cancel and size-limit paths never wrote a terminal state, so the spinner ran forever. They now share `App::end_run_state` with normal turns; `summary_turns_end_their_run_state` and the oversized-summary test cover it.
   - `dmuxai status --json` now reports `ended_at_ms` and, when the start is known, `started_at_ms` and `duration_ms` for done/failed/cancelled (FR-820), asserted in `status_json_maps_pending_and_stalled_to_running_preserving_start_time`.
   - Pill and chip elapsed now use `runElapsedMs`, which applies the same server-clock skew correction as graph nodes.
   - Not changed: the e2e title/favicon test stays `#[ignore]` (needs Chrome); the pill keeping the last `✓ Done` until the next turn is intended (FR-824 says marks persist until the next turn).

## Test evidence

Environment: Linux x86_64. `CARGO_TARGET_DIR=/workspace/dmux-wt/agent-api/target-me` is this worktree's isolated target directory (not a repo setting). Fresh results after round 2:

- `node --check src/web/assets/app.js`: clean.
- `cargo fmt --check`: clean.
- `cargo clippy --locked --all-targets -- -D warnings`: clean.
- `cargo test --locked --all-targets`: 548 passed, 0 failed, 10 ignored (after round 3).
- `cargo test --locked --test web_running -- --ignored`: 1 passed (headless Chrome, screenshot captured).
- `scripts/spec-index.sh`: index unchanged.
