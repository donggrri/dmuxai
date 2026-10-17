# REVIEW: cursor-latency

Spec: [SPEC-cursor-latency](../specs/SPEC-cursor-latency.md)
Plan: [PLAN-cursor-latency](../plans/PLAN-cursor-latency.md)
Tasks: [TASKS-cursor-latency](../tasks/TASKS-cursor-latency.md)

## Spec compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-805 | Met | `prewarm_reused_when_args_match` (warm process spawned after turn 1; no stdin bytes sent before turn 2 prompt; reused on turn 2); `app_new_conversation_discards_warm_cursor_process` (selection & /new trigger); `prewarm_skipped_when_mcp_servers_apply` (skips when MCP applies) |
| FR-806 | Met | `prewarm_reused_when_args_match` (turn 2 reuses warm process matching program, argv, cwd, and env); `prewarm_discarded_on_arg_change` (mode and model changes reap and respawn fresh); `prewarm_exited_process_is_replaced` (exited child detected via `try_wait` and replaced) |
| FR-807 | Met | `prewarm_discarded_on_arg_change` (/new conversation reset reaps and respawns without resume); `prewarm_disabled_and_expiry` (idle timeout after `prewarm_idle_secs` reaps process; adapter drop/quit reaps process; `[cursor] prewarm = false` disables pre-start); `WarmProcess::drop` kills process group |
| NFR-805 | Met | `prewarm_disabled_and_expiry` (graph nodes via `send_graph_node` discard existing warm process and never spawn one); `app_headless_never_prestarts_cursor` (`dmuxai run` / `App::new` never spawns warm processes) |

## Deviations and decisions

- **Adapter startup pre-start removed:** The initial implementation spawned a warm process when `CursorAdapter` initialized. Per FR-805 review, this was removed so pre-start only occurs when Cursor is actively selected or immediately after a Cursor turn ends.
- **MCP server coexistence:** `.cursor/mcp.json` is written on-demand during active turns. To prevent warm processes from either locking the guard file or missing runtime config, pre-warming is skipped when shared MCP servers apply to Cursor (`can_prewarm = config.prewarm && applicable.is_empty()`).
- **Turn process group cleanup:** `process_group(0)` is reverted for normal turn processes. Normal turn cancellation uses Tokio task abortion and `kill_on_drop(true)`. `process_group(0)` is retained exclusively for idle `WarmProcess` instances so that terminating an unprompted warm process cleans up its whole process group (`SIGKILL -{id}`) without affecting siblings.
- **Stale session ID invalidation:** If a user resets a conversation (`/new`) while a Cursor turn is still running, `reset_epoch` tracking ensures the finishing turn's `session_id` is discarded rather than saved to `--resume`.
- **Indicative latency framing:** Documented the ~1.4–2.1 s cold start savings in `README.md` and `CHANGELOG.md` as indicative (measured cold start reduction, dependent on typing time and turn interval).

## Review findings and resolutions

| Finding | Resolution |
| --- | --- |
| Startup warm spawn in adapter loop | Removed initial warm spawn in `runtime_loop`; only pre-starts when Cursor is selected or after turns |
| MCP guard conflict during pre-warm | Checked `applicable.is_empty()`; pre-warming is disabled when MCP servers target Cursor |
| Argument reuse check only checked argv | Updated check to compare `program`, `argv`, `cwd`, and `DMUXAI_CURSOR_BIN` environment override |
| Turn cancel/error did not respawn warm process | Updated turn completion and cancellation handlers to respawn warm process when `can_prewarm` |
| `process_group(0)` on normal turn processes | Reverted `process_group(0)` on normal turn processes; kept only on `spawn_warm_process` |
| Process leak on drop or panic | Implemented `Drop for WarmProcess` with process group kill (`SIGKILL -{id}`) and `start_kill()` |
| Stale `--resume` after reset during turn | Added `reset_epoch` checking on completed turns |
| Race conditions in tests modifying env | Acquired `TEST_CURSOR_MUTEX` across all tests setting `DMUXAI_CURSOR_BIN` |
| Missing App-level tests | Added `app_new_conversation_discards_warm_cursor_process` and `app_headless_never_prestarts_cursor` |
| Blocking `std::process::Command` in async reap | Replaced with `tokio::process::Command` |
| Starter config missing Cursor options | Added `# prewarm = true` and `# prewarm_idle_secs = 300` to `src/init.rs` template |

### Round 2 (hand-coded; AGY quota exhausted)

- `reset()` (startup, `/new`, `/resume`) only discards the warm process and bumps the epoch; it never spawns. The App no longer pre-starts from `apply_runner_and_model` (graph routing). Tests: `app_does_not_prestart_cursor_on_startup_or_new`, `app_new_conversation_discards_warm_cursor_process`, `prewarm_discarded_on_arg_change` step 4.
- `selecting_mode_is_acknowledged_without_starting_a_process` takes `TEST_CURSOR_MUTEX` and disables pre-start explicitly.
- SPEC "Implementation decisions" records triggers, reset epoch, process groups, the MCP skip and the Windows deferral. ROADMAP "Later" has the Windows item.

### Round 3 (opencode-haiku: REQUEST_CHANGES, fixed)

1. Cancelling a graph-node turn spawned a warm process. The Cancel arm now skips it for graph turns.
2. `reap_warm_process` only sent group `SIGKILL` on timeout, so TERM-ignoring descendants leaked. It now always kills the group after the TERM grace period. Test: `reaping_warm_process_kills_term_ignoring_grandchild`.
3. `reset()` used `try_send` and could drop the reset on a full queue. A full queue now falls back to an async send.
4. `stop_turn` sent `SIGKILL` right after `SIGTERM`. It now uses `terminate_group` (TERM, poll up to 500 ms, then KILL).

Not changed: warm stdout/stderr are not drained while idle (cursor-agent prints nothing before it reads the prompt). After a restart, a selection pre-start has no `--resume`, so the first resumed turn discards it; this is wasteful but correct. Cold turns still stop via task abort, as before.

## Test evidence

Environment: Linux x86_64. `CARGO_TARGET_DIR=/workspace/dmux-wt/dashboard-parity/target-me` is this worktree's isolated target dir (not a repo setting). Fresh results after round 3 are below; the earlier per-test list follows.

- `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings`: clean.
- `cargo test --locked --all-targets`: 562 passed, 0 failed, 11 ignored. The pre-start tests were re-run 3× with no flakes.
- `scripts/spec-index.sh`: index unchanged.
- Live check (2026-10-18, composer-2.5, prompt "Reply with one word: ok.", 5 runs each): time to first text was 3.2–6.3 s cold (median 4.5 s) and 2.5–4.1 s when pre-started 8 s earlier (median 3.3 s), about 1.2 s (26%) faster.

- Unit tests:
  - `agent::cursor::tests::prewarm_reused_when_args_match` (TC-805): PASSED
  - `agent::cursor::tests::prewarm_discarded_on_arg_change` (TC-806): PASSED
  - `agent::cursor::tests::prewarm_disabled_and_expiry` (TC-807): PASSED
  - `agent::cursor::tests::prewarm_exited_process_is_replaced` (TC-808): PASSED
  - `agent::cursor::tests::prewarm_skipped_when_mcp_servers_apply`: PASSED
  - `settings::builtin_memory_mcp_tests::cursor_prewarm_settings_parse_and_override`: PASSED
  - `settings::builtin_opencode_tests::starter_config_parses`: PASSED
- Integration tests:
  - `app::tests::app_new_conversation_discards_warm_cursor_process`: PASSED
  - `app::tests::app_headless_never_prestarts_cursor`: PASSED
- Gates:
  - `cargo fmt --check`: clean
  - `cargo clippy --locked --all-targets -- -D warnings`: clean
  - `cargo test --locked --all-targets`: all green (543 passed, 0 failed, 9 ignored)
