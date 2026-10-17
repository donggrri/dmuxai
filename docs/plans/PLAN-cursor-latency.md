# PLAN: cursor-latency

Spec: [SPEC-cursor-latency](../specs/SPEC-cursor-latency.md)
Tasks: [TASKS-cursor-latency](../tasks/TASKS-cursor-latency.md)

## Order of work

1. Settings: `[cursor] prewarm: Option<bool>` (default true for interactive sessions,
   explicitly disabled in headless run), `prewarm_idle_secs: Option<u64>` (default 300)
   in `src/settings.rs` and `src/init.rs`.
2. Cursor adapter (`src/agent/cursor.rs`):
   - `WarmProcess`: owns `child: Option<tokio::process::Child>`, `program`, `argv`, `cwd`,
     `env_bin`, and `spawned_at`.
   - Spawned in its own process group (`process_group(0)`) with piped stdin.
   - `Drop for WarmProcess`: kills process group (`SIGKILL -{id}`) and `start_kill()`
     to avoid orphans on drop or panic.
   - Non-blocking reap using `tokio::process::Command` with SIGTERM and async wait.
3. Trigger lifecycle (FR-805):
   - Trigger pre-start when Cursor is selected in TUI (`/agent`, agent picker,
     `apply_runner_and_model`) and dashboard (`SessionCommand::SelectAgent`).
   - Trigger immediately after a Cursor turn ends (on success, error, or cancellation).
   - Removed startup warm spawn on adapter creation: pre-starts only when selected or after turns.
   - Stale session protection: track `reset_epoch` across turns so turns started prior to `/new`
     cannot attach stale `--resume <session-id>` to warm processes.
   - MCP safety: skip pre-warming when MCP servers apply to Cursor (`can_prewarm = prewarm && applicable.is_empty()`).
4. Turn execution and reuse check (FR-806):
   - Exact reuse check: compare `program`, `argv`, `cwd`, and env override (`DMUXAI_CURSOR_BIN`).
   - If dead (`try_wait`) or arguments differ: reap and spawn fresh.
   - Revert `process_group(0)` for normal turn processes; rely on task abort and `kill_on_drop(true)`.
5. Invalidation and isolation (FR-807, NFR-805):
   - Discard on mode change, model change, `/new` conversation reset, idle timeout (`prewarm_idle_secs`),
     graph nodes, and adapter drop/shutdown.
   - Never pre-warm in headless CLI (`dmuxai run`, `App::new`).
6. Tests:
   - TC-805 (`prewarm_reused_when_args_match`): assert one spawn total for turn 2 and zero stdin bytes before prompt.
   - TC-806 (`prewarm_discarded_on_arg_change`): mode, model, `/new` discard and reap process.
   - TC-807 (`prewarm_disabled_and_expiry`): disabled config, graph nodes, idle expiry, shutdown.
   - TC-808 (`prewarm_exited_process_is_replaced`): exited process reaped, prompt runs in fresh process.
   - MCP safety test (`prewarm_skipped_when_mcp_servers_apply`).
   - App tests: `app_new_conversation_discards_warm_cursor_process`, `app_headless_never_prestarts_cursor`.
   - Test isolation: acquire `TEST_CURSOR_MUTEX` in all tests that modify `DMUXAI_CURSOR_BIN`.
   - Polling loops with timeout instead of fixed sleeps where practical.
7. Documentation:
   - `README.md` Cursor latency section (indicative 1.4–2.1 s measured saving, `[cursor]` config).
   - `CHANGELOG.md` Unreleased entry.
   - `src/init.rs` starter template.

## Risks

- Orphan processes if dmuxai panics or adapter drops: mitigated by `WarmProcess::drop` killing
  its process group (`SIGKILL -{id}`) and `kill_on_drop(true)`.
- Stale `--resume` after conversation reset (`/new`): mitigated by `reset_epoch` tracking.
- Pre-mature `.cursor/mcp.json` creation: mitigated by skipping pre-warm when MCP servers apply to Cursor.
- Environment variable races in tests: mitigated by `TEST_CURSOR_MUTEX`.

## Rollback

Set `[cursor] prewarm = false` in `dmuxai.toml` or config. Adapter defaults fall back to
one-shot turn spawning on prompt submission.
