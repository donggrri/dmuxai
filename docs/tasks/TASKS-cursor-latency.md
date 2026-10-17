# TASKS: cursor-latency

Status: complete
Spec: [SPEC-cursor-latency](../specs/SPEC-cursor-latency.md)
Plan: [PLAN-cursor-latency](../plans/PLAN-cursor-latency.md)
Summary row: `docs/tasks/TASKS-v0.2.1.md` task 10.

- [x] T1 — Settings schema and starter template: add `prewarm` and `prewarm_idle_secs` to `CursorToml`.
  Requirements: FR-807. Verify:
  `cargo test --locked --lib settings::builtin_memory_mcp_tests::cursor_prewarm_settings_parse_and_override` and
  `cargo test --locked --lib settings::builtin_opencode_tests::starter_config_parses`
- [x] T2 — Cursor adapter `WarmProcess` process group isolation, drop cleanup, and async non-blocking reap.
  Requirements: FR-807. Verify:
  `cargo test --locked --lib agent::cursor::tests::prewarm_disabled_and_expiry`
- [x] T3 — Cursor pre-start lifecycle: trigger on selection (TUI and dashboard) and after turns (success, error, cancel);
  skip on adapter startup; MCP safety check (`can_prewarm`); stale `--resume` protection with `reset_epoch`.
  Requirements: FR-805, FR-807. Verify:
  `cargo test --locked --lib agent::cursor::tests::prewarm_reused_when_args_match`,
  `cargo test --locked --lib agent::cursor::tests::prewarm_skipped_when_mcp_servers_apply`, and
  `cargo test --locked app_new_conversation_discards_warm_cursor_process`
- [x] T4 — Turn reuse check comparing `program`, `argv`, `cwd`, and `DMUXAI_CURSOR_BIN`; reap exited/mismatched processes;
  revert `process_group(0)` on normal turn processes.
  Requirements: FR-806. Verify:
  `cargo test --locked --lib agent::cursor::tests::prewarm_reused_when_args_match`,
  `cargo test --locked --lib agent::cursor::tests::prewarm_discarded_on_arg_change`, and
  `cargo test --locked --lib agent::cursor::tests::prewarm_exited_process_is_replaced`
- [x] T5 — Invalidation on mode change, model change, `/new` reset, idle expiry, graph nodes, and headless run.
  Requirements: FR-807, NFR-805. Verify:
  `cargo test --locked --lib agent::cursor::tests::prewarm_discarded_on_arg_change`,
  `cargo test --locked --lib agent::cursor::tests::prewarm_disabled_and_expiry`, and
  `cargo test --locked app_headless_never_prestarts_cursor`
- [x] T6 — Test suite hardening: `TEST_CURSOR_MUTEX` for environment variable safety, one-spawn assertions, zero stdin bytes assertions, and polling loops.
  Requirements: FR-805, FR-806, FR-807, NFR-805. Verify:
  `cargo test --locked prewarm`
- [x] T7 — Documentation: README latency note with indicative measured saving and configuration options; CHANGELOG entry.
  Requirements: FR-805, FR-807. Verify:
  Review diff against README.md and CHANGELOG.md.
- [x] T8 — Gates: verify fmt, clippy, and all tests green.
  Requirements: FR-805..FR-807, NFR-805. Verify:
  `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked --all-targets`
