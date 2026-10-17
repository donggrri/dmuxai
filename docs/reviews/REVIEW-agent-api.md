# REVIEW: Agent API (headless CLI, HTTP, MCP, state-change events)

Spec: [SPEC-agent-api](../specs/SPEC-agent-api.md)

## Spec compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-760 | Met | `parse_cli_agent_api_subcommands`, `parse_cli_routes_agent_subcommands`; a value flag never swallows a following `--flag`; `--help` after `--` is a prompt |
| FR-761 | Met | `headless_run_json_success_and_failure` (unix binary test, includes `run -` from stdin and empty stdin → 2), `headless_run_returns_agent_text`, `headless_run_reports_agent_error`, `headless_run_cancel_exits_130` (all platforms) |
| FR-762 | Met | `todo_cli_add_list_run` (unix): add, list, run to done, missing id → 3 |
| FR-763 | Met | `status_json_has_contract_keys`; the token itself is never printed (`dashboard.token_file` only) |
| FR-764 | Met | `read_since_skips_malformed_and_filters`, `events_cli_since_json` |
| FR-765 | Met | `prompt_route_contract`, `dashboard_submit_prompt_runs_like_enter` |
| FR-766 | Met | `events_log_and_sse_state_change`; `serve` keeps the event log even when the to-do DB cannot open |
| FR-767 | Met | `mcp_stdio_protocol_and_tools` (jsonrpc "2.0" required, notifications get no reply, `run` has a timeout) |
| FR-768 | Met | `tracker_emits_one_event_per_transition`, `event_log_seq_is_monotonic_across_handles`, `app_emits_agent_lifecycle_events`; `serve` writes no events, so nothing is duplicated |
| FR-769 | Met | `stall_detector_emits_once_and_resets` |
| FR-770 | Met | `webhook_posts_json_with_secret_no_redirect` (secret from env var name, 302 not followed, slow server times out) |
| FR-771 | Met | `command_hook_receives_event_on_stdin` (unix) |
| FR-772 | Met | `webhook_posts_json_with_secret_no_redirect` (filtered `to_state` skipped) |
| FR-773 | Met | `discovery_file_and_token_env` (0600 on Unix, even over a pre-existing 0644 file; invalid `DMUXAI_WEB_TOKEN` ignored) |
| NFR-760 | Met | JSON-only stdout asserted in the CLI tests; the repo-DB notice goes to stderr |
| NFR-761 | Met | Notifier tests use a local `TcpListener` mock; no network |
| NFR-762 | Met | `headless_run_respects_allowed_runners` (exit 5 / exit 3) |
| UX-760 | Met | README "Calling dmuxai from an agent", `docs/API.md` |

## Test evidence

Local Linux results:

- `cargo fmt --check` is clean.
- `cargo clippy --all-targets --all-features -- -D warnings` is clean, also cross-checked for `x86_64-pc-windows-gnu`.
- `cargo test --all-targets` (rebased on `4c7b5c4`): 426 passed, 0 failed, 5 ignored.
- `scripts/spec-index.sh` lists 33 specs; `spec_ids_are_unique_across_specs` passes.
- Smoke run of the debug binary in a scratch workspace: `status --json`, `todo add/list --json`, `todo run 999` → 3, `run --agent nope` → 3, `mcp` initialize + tools/list, `events --since 0 --json`.

## Cross-reviews (through dmuxai)

- glm reviewed `events.rs`: poisoned-lock panics, full-file rereads for the last seq, a blocking ticker, graph state reset on a vanished graph, silent append failures and a fragile TC-770. All fixed (deepseek).
- deepseek reviewed `notify.rs`/`discovery.rs`: token-file permission window, blocking drop log, `eprintln!` from the notifier, env races in tests, the 302 log line and pid-based temp dirs. All fixed (cursor).
- glm reviewed the CLI/MCP code: stderr notice, value-flag consumption, `--help` after `--`, jsonrpc checks, MCP run timeout, stdin test, corrupt graph warning. All fixed except the two residual items below.
- cursor (composer-2.5) reviewed the full diff: TC-761 not exercised on Windows, `serve` losing `events_path` when the to-do DB fails, TC-769 reused for agent lifecycle (now TC-775), `messages[].role` in API.md, `events.first_seq > last_seq` for runs without events (now `null`), `since` not validated without a log, the 504 wording, no cancel test, the `sh` dependency of the to-do CLI test. All fixed.
- Orchestrator review: SSE tail task kept polling after client disconnect; now stops on a closed channel.

## Residual risk

- `events --follow` and the SSE tail re-read the log every 500 ms (no inotify, no rotation); fine for normal logs, slow for very large ones.
- Stale-lock recovery deletes `.dmuxai/events.lock` after 5 s; a writer paused longer than that could interleave.
- A TUI that re-lists to-dos after another process changed them can emit a to-do transition the other process already logged.
- The MCP server handles requests sequentially; a long `run` blocks other calls until it ends (timeout 2 h, `DMUXAI_MCP_RUN_TIMEOUT_SECS`).
- Multi-line prompt whitespace is joined with single spaces when passed as several argv words (use `run -` with stdin to keep formatting).
- Binary-level CLI tests with the shell fixture are unix-only; Windows CI covers the same paths through in-process `App::run_headless` tests.

## Rollback

Revert the PR commit. The only persisted artifacts are `.dmuxai/events.jsonl`, `.dmuxai/events.lock`, `.dmuxai/notify.log` and `.dmuxai/web.json` in the workspace; they can be deleted. The `[notify]` table is optional and ignored by older builds.
