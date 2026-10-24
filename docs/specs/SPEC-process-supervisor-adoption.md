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
- MCP stdio client and Office conversion shell `kill` elimination is deferred to Part D (planned FR-1967..FR-1968).
- Windows non-unix supervisor support remains non-goal.

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

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1960 | FR-1960 | unit (unix) | Spawned `sh -c 'echo a; sleep 30'` with `configure_child` and piped stdio | Adopt child into `Supervised` | `next_line()` yields `Line("a")`, `live_groups()` contains pgid while alive, and `stop()` kills group so pgid is unregistered and process is gone | `src/process/supervisor.rs` `tc_1960_adopt_spawns_child_yields_lines_and_stop_kills_group` |
| TC-1961 | FR-1961 | unit (unix) | Process group with a child trapping `SIGTERM` and sleeping, and a non-existent group | Call `terminate_group(pgid, 300ms)` and `terminate_group(dead_pgid, 500ms)` | Terminating dead group returns immediately; stubborn group is terminated within grace plus kill escalation; group is dead | `src/process/supervisor.rs` `tc_1961_terminate_group_escalates_to_kill_and_handles_dead_group` |
| TC-1962 | FR-1962 | unit (unix) | One child that exits promptly with code 0 and one child sleeping 30 s | Call `wait_exit` on both | Returns `Some(status)` with code 0 for exiting child and `None` on timeout for sleeping child without sending signals | `src/process/supervisor.rs` `tc_1962_wait_exit_waits_without_signals_and_records_status` |
| TC-1963 | FR-1963 | unit | Various stderr strings with empty lines, trailing newlines, and varying line counts | Call `tail_lines(tail, max_lines)` | Yields last `max_lines` non-empty lines joined with `" | "`; handles 0 max_lines, empty string, and whitespace-only lines | `src/process/supervisor.rs` `tc_1963_tail_lines_formats_last_non_empty_lines` |
| TC-1964 | FR-1964 | integration (unix) | Fake AGY turn running in own process group | Run turn, verify live group tracking, idle timeout, and result grace | `live_groups()` contains AGY pgid during turn and unregisters after completion; existing idle kill and result grace tests continue to pass | `src/agent/agy.rs` `tc_1964_agy_uses_supervisor_and_registers_group` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Process group lifecycle | Children adopted via `adopt` are registered in `live_groups()` and cleaned up on stop or drop | Backward compatible; preserves live-group registry semantics | TC-1960 |
| Termination escalation | `terminate_group` unifies `SIGTERM` delivery, 25 ms polling, and `SIGKILL` escalation | Replaces duplicated private copies in runner modules | TC-1961 |
| Output streaming | Line streaming, idle watchdogs, and stderr tail capture unified under `Supervised` | Same error messages and behavior as existing AGY implementation | TC-1964 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Supervisor API additions (adopt, terminate_group, wait_exit, tail_lines) | `src/process/supervisor.rs` | FR-1960, FR-1961, FR-1962, FR-1963 | `tc_1960`, `tc_1961`, `tc_1962`, `tc_1963` |
| 2 | AGY runner migration onto supervisor | `src/agent/agy.rs` | FR-1964 | `tc_1964`, existing agy tests |
| 3 | (planned) Cursor legacy migration onto supervisor | `src/agent/cursor.rs` | FR-1965 (planned) | Part B verification |
| 4 | (planned) ACP client terminate_group adoption | `src/agent/acp/client.rs` | FR-1966 (planned) | Part C verification |
| 5 | (planned) MCP client & Office conversion cleanup of shell kill | `src/mcp/client.rs`, `src/docs/office.rs` | FR-1967, FR-1968 (planned) | Part D verification |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-1960..FR-1964) map to passing test evidence (TC-1960..TC-1964).
- [x] The diff has been reviewed against scope and non-goals (Part A scope only).
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [x] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
