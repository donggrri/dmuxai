# SPEC-process-supervisor-adoption

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `376e86d`
Related: refactor scan finding #2; PR-5b; SPEC-process-supervisor

## 1. Decision summary

Following up on PR-5 (`SPEC-process-supervisor.md`), runner process supervision is unified across all adapters to eliminate duplicated child management, custom process group handling, and raw shell kills (`kill` CLI invocations). PR-5b adds core supervisor extensions (`adopt`, `terminate_group`, `wait_exit`, and `tail_lines`) and migrates the AGY runner (`src/agent/agy.rs`) directly onto the shared supervisor (`dmuxai::process::supervisor`), with subsequent parts adopting Cursor legacy, ACP client, and MCP/Office cleanup.

This part (Part A) provides:
- `adopt(child, config)` to wrap pre-spawned or warm child processes into a `Supervised` handle, registering the process group in the live-group registry, running the background stderr drain, and initializing timers. `spawn` is refactored to delegate to `adopt`.
- `terminate_group(pgid, grace)` to unify graceful `SIGTERM` delivery with 25 ms polling and `SIGKILL` escalation across the codebase.
- `Supervised::wait_exit(timeout)` to wait for a leader process to exit on its own without sending signals, recording the exit status for subsequent `stop` or `finish` calls.
- `tail_lines(tail, max_lines)` to format the last non-empty stderr lines joined with `" | "` for runner diagnostic messages.
- Full migration of the AGY adapter onto `dmuxai::process::supervisor`, eliminating AGY's custom select loop, custom idle timer, `push_stderr`, stderr history `VecDeque`, and manual `GroupGuard`.

## 2. Scope

### In scope

- Supervisor API additions: `adopt`, `terminate_group`, `Supervised::wait_exit`, and `tail_lines` in `src/process/supervisor.rs`.
- AGY runner migration onto `Supervised` line streaming, idle watchdogs, result grace handling, and stderr tail capture in `src/agent/agy.rs`.
- Maintaining exact backward compatibility of AGY behavior, idle error messaging, and result grace semantics.
- Unit and integration tests for all new supervisor additions and AGY runner lifecycle under `TC-1960`..`TC-1964`.

### Out of scope / non-goals

- Cursor legacy runner migration onto `Supervised` and warm process adoption (`adopt`) is deferred to Part B (planned FR-1965).
- ACP client migration to shared `terminate_group` and stderr formatting is deferred to Part C (planned FR-1966).
- MCP stdio client and Office conversion shell `kill` elimination is complete in Part D (FR-1967..FR-1968).
- Windows non-unix supervisor support remains non-goal.
- Unification of per-runner command enums (`CursorCommand`, `CodexCommand`, `AgyCommand`, `CustomCommand`) and their channel runtime loops is out of scope and deferred.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1960 | AGY result grace period of 3 seconds is preserved, with immediate leftover group termination upon leader exit. | dmuxai | Part A implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Adopted child | A child process spawned with `configure_child` wrapped into `Supervised` after creation. | Fresh child created directly via `supervisor::spawn`. |
| Unsignaled wait | Awaiting child leader exit without delivering `SIGTERM` or `SIGKILL` signals. | `stop()` or `finish()`, which always deliver termination signals. |
| Diagnostic tail | Joined string of the most recent non-empty stderr lines formatted with `" | "`. | Full captured stderr text. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1960 | Must | Given a Tokio child process spawned with `configure_child` and piped stdout/stderr | `adopt(child, config)` returns a `Supervised` handle registered in `live_groups()` with background stderr draining and watchdogs initialized; `spawn` delegates directly to `configure_child` and `adopt` | Fails with `std::io::Error` if the child PID, stdout, or stderr cannot be acquired | TC-1960 |
| FR-1961 | Must | Given a process group ID `pgid` and a `grace` duration | `terminate_group(pgid, grace)` sends `SIGTERM` to the group, returns immediately if the group is already gone, polls `group_alive` every 25 ms up to `grace`, then sends `SIGKILL` if the group remains alive | Returns immediately if `signal_group` returns false (group gone or invalid pgid) | TC-1961 |
| FR-1962 | Must | Given a `Supervised` handle and a `timeout` duration | `wait_exit(timeout)` waits up to `timeout` for the child leader to exit without sending signals, stores the status in `leader_exit_status`, and returns `Some(status)` | Returns `None` on timeout without signaling the process group, preserving the ability for subsequent `stop` or `finish` calls to reap | TC-1962 |
| FR-1963 | Must | Given a stderr tail string and a `max_lines` limit | `tail_lines(tail, max_lines)` returns the last `max_lines` non-empty lines joined with `" | "` | Returns an empty string if `max_lines` is 0 or if the input contains no non-empty lines | TC-1963 |
| FR-1964 | Must | Given an AGY agent turn execution | The child process is supervised with `supervisor::spawn`, configures idle timeout from `agy_idle_timeout_secs()` (300 s default; 0 disables) and 500 ms term grace; streams stdout via `next_line()`, and reaps the child/grandchild group upon result completion (`wait_exit` + `finish(0)` or `stop`), idle (`stop`), or EOF (`finish(3s)`) | Stderr diagnostic formatting uses `tail_lines(..., 8)`; idle timeout returns standard `AgentError::idle` text; group is registered in `live_groups()` during execution | TC-1964 |
| FR-1965 | Must | Given a Cursor legacy agent turn execution (fresh or warm-adopted child) | Fresh turn children are spawned with `supervisor::spawn`, matching warm children are wrapped with `supervisor::adopt` at turn start, and `stream_child` loops over `next_line()` with terminal-result grace (`wait_exit(TERMINAL_EXIT_GRACE)`), forced stop on timeout, and `tail_lines(..., 8)` diagnostics | Cursor's private `terminate_group`, `push_stderr`, and stderr ring are deleted; warm-process idle expiry/reaping behavior remains unchanged; group is registered in `live_groups()` during the turn | TC-1965 |
| FR-1966 | Must | Given an ACP client turn execution | The ACP client uses the shared `supervisor::terminate_group(pgid, 500ms)` for group termination and formats its stderr diagnostic with `tail_lines`-compatible output (last 8 non-empty lines joined with `" | "`) | No private `terminate_group` copy remains in `src/agent/acp/client.rs`; full `adopt` of ACP remains out of scope | TC-1966 |
| FR-1967 | Must | Given an MCP stdio client connection | The child process is built with `crate::process::supervisor::configure_child` (own process group, `kill_on_drop`, and Linux `PR_SET_PDEATHSIG`), and killed with `signal_group(pgid, GroupSignal::Kill)` plus `start_kill`; no `std::process::Command::new("kill")` subprocess is used | MCP stdio servers now also receive `PR_SET_PDEATHSIG` on Linux (behavior change); process group is killed on drop, timeout, and error paths | TC-1967 |
| FR-1968 | Must | Given an Office (LibreOffice) conversion that times out or fails | `kill_tree` sends `signal_group(child.id(), GroupSignal::Kill)` under `cfg(unix)` instead of a shell `kill` subprocess, then `child.kill()` and `child.wait()`; the whole process group (including `soffice.bin` forked by the wrapper) is killed | No `std::process::Command::new("kill")` subprocess remains in `src/docs/office.rs`; `process_group(0)` is retained | TC-1968 |
| FR-1970 | Must | Given a channel send error (`tokio::sync::mpsc::error::TrySendError<T>`) | `map_send_error(error)` maps `TrySendError::Full` to `AdapterError::CommandQueueFull` and `TrySendError::Closed` to `AdapterError::RuntimeStopped` uniformly across all runner command channels | Returns `AdapterError` variant preserving original runner error classifications | TC-1970 |
| FR-1971 | Must | Given an event channel, runner `Agent`, optional `&OutputContext`, and `AgentEvent` | `emit(event_tx, agent, context, event)` constructs `AgentOutput` tagged with agent, conversation_id, and turn_id and asynchronously sends it to `event_tx`, ignoring send errors | Unified helper used by Cursor, Codex, Agy, ACP client, and Custom runners directly or via thin wrappers | TC-1971 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1960 | FR-1960 | unit (unix) | Spawned `sh -c 'echo a; sleep 30'` with `configure_child` and piped stdio | Adopt child into `Supervised` | `next_line()` yields `Line("a")`, `live_groups()` contains pgid while alive, and `stop()` kills group so pgid is unregistered and process is gone | `src/process/supervisor.rs` `tc_1960_adopt_spawns_child_yields_lines_and_stop_kills_group` |
| TC-1961 | FR-1961 | unit (unix) | Process group with a child trapping `SIGTERM` and sleeping, and a non-existent group | Call `terminate_group(pgid, 300ms)` and `terminate_group(dead_pgid, 500ms)` | Terminating dead group returns immediately; stubborn group is terminated within grace plus kill escalation; group is dead | `src/process/supervisor.rs` `tc_1961_terminate_group_escalates_to_kill_and_handles_dead_group` |
| TC-1962 | FR-1962 | unit (unix) | One child that exits promptly with code 0 and one child sleeping 30 s | Call `wait_exit` on both | Returns `Some(status)` with code 0 for exiting child and `None` on timeout for sleeping child without sending signals | `src/process/supervisor.rs` `tc_1962_wait_exit_waits_without_signals_and_records_status` |
| TC-1963 | FR-1963 | unit | Various stderr strings with empty lines, trailing newlines, and varying line counts | Call `tail_lines(tail, max_lines)` | Yields last `max_lines` non-empty lines joined with `" | "`; handles 0 max_lines, empty string, and whitespace-only lines | `src/process/supervisor.rs` `tc_1963_tail_lines_formats_last_non_empty_lines` |
| TC-1964 | FR-1964 | integration (unix) | Fake AGY turn running in own process group | Run turn, verify live group tracking, idle timeout, and result grace | `live_groups()` contains AGY pgid during turn and unregisters after completion; existing idle kill and result grace tests continue to pass | `src/agent/agy.rs` `tc_1964_agy_uses_supervisor_and_registers_group` |
| TC-1965 | FR-1965 | integration (unix) | Fake Cursor legacy turn and warm-adopted turn | Run both turns and inspect live group registration | Both turns complete; `live_groups()` contains the cursor pgid during each turn; fake Cursor binary only, no real Cursor CLI | `src/agent/cursor.rs` `tc_1965_cursor_legacy_uses_supervisor_and_registers_group` |
| TC-1966 | FR-1966 | integration (unix) | Fake ACP agent turn | Run turn and verify process cleanup | ACP turn completes and no fake ACP agent process outlives the turn via shared `terminate_group` | `src/agent/cursor.rs` `tc_1966_acp_client_uses_shared_terminate_group` |
| TC-1967 | FR-1967 | integration (unix) | MCP stdio server script that spawns a sleeping grandchild | Connect then drop (or time out) the client | The sleeping grandchild is gone within 2 s; no shell `kill` subprocess is used | `tests/mcp_client.rs` `tc_1967_mcp_stdio_drop_kills_grandchild_group` |
| TC-1968 | FR-1968 | unit (unix) | `sh -c 'sleep 30 & sleep 30'` spawned in its own process group | Call `kill_tree` on the leader | Both sleeping processes are gone; no shell `kill` subprocess is used | `src/docs/office.rs` `tc_1968_office_kill_tree_kills_process_group` |
| TC-1970 | FR-1970 | unit | Full bounded channel and dropped channel receiver | Call `map_send_error` on `TrySendError::Full` and `TrySendError::Closed` | `TrySendError::Full` maps to `AdapterError::CommandQueueFull` and `TrySendError::Closed` maps to `AdapterError::RuntimeStopped` | `src/agent/mod.rs` `tc_1970_map_send_error_maps_full_and_closed` |
| TC-1971 | FR-1971 | unit | Event channel with `Some(context)` and `None` context | Call `emit` with Cursor/Some(context) and Agy/None | Receiver yields `AgentOutput` with matching agent, conversation_id, turn_id, and event | `src/agent/mod.rs` `tc_1971_emit_tags_agent_conversation_and_turn` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Process group lifecycle | Children adopted via `adopt` are registered in `live_groups()` and cleaned up on stop or drop | Backward compatible; preserves live-group registry semantics | TC-1960 |
| Termination escalation | `terminate_group` unifies `SIGTERM` delivery, 25 ms polling, and `SIGKILL` escalation | Replaces duplicated private copies in runner modules | TC-1961 |
| Output streaming | Line streaming, idle watchdogs, and stderr tail capture unified under `Supervised` | Same error messages and behavior as existing AGY implementation | TC-1964 |
| MCP stdio PDEATHSIG | MCP stdio servers are now spawned with `configure_child`, gaining `PR_SET_PDEATHSIG` on Linux | Behavior change: previously MCP used `process_group(0)` + `kill_on_drop` without parent-death signal; now matches other supervised children | TC-1967 |
| Runner adapter helpers | Generic `map_send_error` and `emit` unified in `src/agent/mod.rs` across all runner adapters | Backward compatible; preserves error variants and event envelope fields | TC-1970, TC-1971 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Supervisor API additions (adopt, terminate_group, wait_exit, tail_lines) | `src/process/supervisor.rs` | FR-1960, FR-1961, FR-1962, FR-1963 | `tc_1960`, `tc_1961`, `tc_1962`, `tc_1963` |
| 2 | AGY runner migration onto supervisor | `src/agent/agy.rs` | FR-1964 | `tc_1964`, existing agy tests |
| 3 | Cursor legacy migration onto supervisor | `src/agent/cursor.rs` | FR-1965 | `tc_1965`, existing cursor tests |
| 4 | ACP client shared terminate_group adoption | `src/agent/acp/client.rs` | FR-1966 | `tc_1966`, existing acp tests |
| 5 | MCP client & Office conversion cleanup of shell kill | `src/mcp/client.rs`, `src/docs/office.rs` | FR-1967, FR-1968 | TC-1967, TC-1968 |
| 6 | Shared runner adapter helpers (map_send_error, emit) | `src/agent/mod.rs`, `src/agent/cursor.rs`, `src/agent/codex.rs`, `src/agent/agy.rs`, `src/agent/custom.rs`, `src/agent/acp/client.rs` | FR-1970, FR-1971 | TC-1970, TC-1971 |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-1960..FR-1968, FR-1970..FR-1971) map to passing test evidence (TC-1960..TC-1968, TC-1970..TC-1971).
- [x] The diff has been reviewed against scope and non-goals (Part A scope only).
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [x] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
