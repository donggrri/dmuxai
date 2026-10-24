# SPEC-process-supervisor

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `93b1fe6`
Related: #123, #162; PR-5

## 1. Decision summary

Runner process management is unified into a shared process supervisor module (`src/process/supervisor.rs`, lib crate `dmuxai::process::supervisor`). The supervisor centralizes child process spawning in isolated process groups, idle and total timeout watchdogs, non-blocking stderr tail capture, graceful shutdown escalation (`SIGTERM` -> grace period -> `SIGKILL`), and grandchild cleanup after turn completion. This closes #123 (custom agent hang without output) and #162 (SIGTERM/SIGHUP leaving orphaned runner processes editing the workspace) by ensuring one robust supervisor controls the lifecycle and group signaling of runner processes.

## 2. Scope

### In scope

- Unix process-group signaling (`libc::killpg`) with safety guards preventing signaling 0, 1, or the caller's own process group (`signal_group`).
- Child process configuration (`configure_child`) with process group isolation (`process_group(0)`), `kill_on_drop(true)`, and Linux parent death signal (`PR_SET_PDEATHSIG` SIGTERM) with parent PID verification.
- Live-group registry (`live_groups`, `stop_all_live_groups`) backed by a global mutex-protected set.
- `Supervised` child handle with line-by-line stdout streaming (`next_line`), idle and total duration watchdogs (`LineEvent::Idle`, `LineEvent::Timeout`), and non-blocking stderr tail ring buffer (`stderr_tail`).
- Two-phase graceful termination (`stop`), grandchild reaping after leader exit (`finish`), and safe drop termination (`Drop for Supervised`).
- Diagnostic error messages (`idle_error_message`, `timeout_error_message`).

### Out of scope / non-goals

- Porting specific runners (`custom`, `agy`, `cursor`, `acp`) to the supervisor; agy, cursor, and acp are addressed in subsequent parts of PR-5.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1779 | Stderr tail keeps older bytes dropped in memory and never stalls child writes. | dmuxai | Part A implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Process group | POSIX process group (`pgid`) where all descendants share a group ID. | Individual process PID. |
| Supervised | Handle managing a runner child process, its stdout line stream, and stderr tail. | Raw `tokio::process::Child`. |
| Idle watchdog | Timer measuring elapsed duration since the last received stdout line (or spawn). | Total execution timeout. |
| Grandchild leak | Child process spawned by the runner that outlives the runner leader process. | Direct zombie child. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1779 | Must | Given a process group ID and a `GroupSignal` (Term, Kill, Int) | `signal_group(pgid, sig)` delivers the signal via `libc::killpg` and returns `true` on success | Returns `false` and performs no signal delivery when `pgid` is 0, 1, > `i32::MAX`, or equals our own process group | TC-1779 |
| FR-1780 | Must | Given a Tokio command and `SupervisorConfig` | `spawn` puts the child in its own process group (`pgid == child.id()`), distinct from the supervisor's group, and registers `pgid` in `live_groups()` | Unregisters the group from `live_groups()` when stopped or reaped | TC-1780 |
| FR-1781 | Must | Given a running supervised process emitting stdout lines | `next_line()` yields `LineEvent::Line(line)` for each line in sequence and `LineEvent::Eof` upon EOF | Preserves partial line content across polls until newline is received | TC-1781 |
| FR-1782 | Must | Given a supervised child configured with `idle_timeout` that emits no output | `next_line()` yields `LineEvent::Idle(d)` after `idle_timeout` elapses; calling `stop()` terminates and reaps the child | Reaping clears the child PID | TC-1782 |
| FR-1783 | Must | Given a child emitting lines periodically with intervals less than `idle_timeout` | Each line resets the idle timer; all lines stream to `LineEvent::Eof` without triggering `LineEvent::Idle` | An interval exceeding `idle_timeout` triggers `Idle` | TC-1783 |
| FR-1784 | Must | Given a child running longer than `total_timeout` | `next_line()` yields `LineEvent::Timeout(d)` even if lines continue arriving and resetting the idle timer | Disabled when `total_timeout` is `None` | TC-1784 |
| FR-1785 | Must | Given a completed turn where a grandchild process continues running and holding open stdout | `finish(exit_grace)` waits up to `exit_grace` for the leader, then sends `SIGTERM` to the process group, terminating the grandchild | Grandchild process is terminated and turn finishes without blocking | TC-1785 |
| FR-1786 | Must | Given a child writing substantial volume to stderr | A background reader task drains stderr to EOF without blocking child execution, retaining at most `stderr_tail_bytes` in a sliding tail window returned by `stderr_tail()` | Returns lossy UTF-8 string bounded by tail limit | TC-1786 |
| FR-1787 | Must | Given an uncooperative child ignoring `SIGTERM` | `stop()` sends `SIGTERM`, waits up to `term_grace` for the leader, then escalates by sending `SIGKILL` to the group and killing the leader, successfully reaping it | Idempotent on repeated calls | TC-1787 |
| FR-1788 | Must | Given an active `Supervised` handle that is dropped before being reaped | `Drop` sends `SIGTERM` then `SIGKILL` to the group, kills the leader, and unregisters the group from `live_groups()` | Does not hang on drop | TC-1788 |
| FR-1789 | Must | Given `ChildOptions` with `parent_death_signal` set to `true` or `false` on Linux | Child processes spawn and execute normally under either setting | In pre_exec, exits with code 1 if parent death signal cannot be configured or parent died | TC-1789 |
| FR-1790 | Must | Given `dmuxai run ... --json` is running and receives SIGTERM or SIGHUP | The cancel flag is set like Ctrl+C, producing a `cancelled` event and final JSON with exit code 130 | Same path as Ctrl+C; no orphaned runner edits remain | TC-1790 |
| FR-1791 | Must | Given `dmuxai run` exits after a stop signal | The runner process group is gone (leader and descendants reaped) | `kill -0` fails for both leader and grandchild PIDs | TC-1791 |
| FR-1792 | Must | Given a stop signal after which the run has not finished within 10 s | `stop_all_live_groups` is called and the process exits 130 with `error: stopped after signal`; a second signal does so immediately | Grace period is 10 s; no hang | TC-1792 |
| FR-1793 | Must | Given `CursorAdapterConfig::default()` | Its `protocol` equals `DmuxSettings::default().cursor_protocol` (`Legacy`) | Matches settings default; no ACP default | TC-1793 |
| FR-1794 | Must | Given a custom agent configured with `idle_timeout_secs` (default 300) that produces no stdout for that duration | The turn fails with `AgentErrorKind::Idle` naming the agent label/name and `idle_timeout_secs` in the error message; the child process group is terminated and reaped | If output arrives within `idle_timeout_secs`, no error occurs | TC-1794 |
| FR-1795 | Must | Given a running custom agent emitting stdout lines | Each stdout line resets the idle timer; streaming continues without `Idle` timeout as long as the interval between lines is under `idle_timeout_secs` | An idle interval exceeding `idle_timeout_secs` triggers the watchdog | TC-1795 |
| FR-1796 | Must | Given a custom agent with `idle_timeout_secs` unset or explicitly set to 0 | Unset defaults to 300 seconds; explicit 0 disables the idle watchdog completely | Disabled watchdog allows turns with long pauses between stdout lines | TC-1796 |
| FR-1797 | Must | Given a custom agent turn without `done_when` where the leader process exits 0 while a detached grandchild holds stdout open | The turn does not block waiting for EOF; it completes within 10 s with the text and reaps the grandchild process group | Grandchild PID is gone after turn finishes | TC-1797 |
| FR-1798 | Must | Given a runner that keeps its own `tokio::process::Child` (agy, cursor legacy, ACP) | `group_alive(pgid)` reports whether any process in the group is alive (false for 0, 1, and the caller's own group); `GroupGuard::register(pgid)` adds the group to the live-group registry for the life of the turn and unregisters on drop | Group stays registered only while the runner is running | TC-1798 |
| FR-1799 | Must | Given agy, cursor legacy, and ACP child processes | They run in their own process group, are registered in `live_groups()` while running, and are stopped with `libc::killpg` via `signal_group`; no `Command::new("kill")` shell-outs remain in `src/agent` or `src/process` | Kill path is idempotent and leaves no orphaned descendants | TC-1799 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1779 | FR-1779 | unit | `GroupSignal` variants | Call `signal_group` with 0, 1, and own process group | Returns `false` for 0, 1, and own process group; supervisor survives | `src/process/supervisor.rs` `tc_1779_signal_group_refuses_zero_one_and_own_group` |
| TC-1780 | FR-1780 | unit | Child running `sleep 30` | `spawn` child process | `pgid == child.id()`, `pgid != own_pgid`, registered in `live_groups()`, gone after `stop()` | `src/process/supervisor.rs` `tc_1780_spawn_puts_child_in_own_group_and_registers_it` |
| TC-1781 | FR-1781 | unit | Child echoing two lines | Call `next_line()` repeatedly | Streams `hello`, `world`, then `Eof` | `src/process/supervisor.rs` `tc_1781_next_line_streams_lines_then_eof` |
| TC-1782 | FR-1782 | unit | Silent child sleeping with 1 s idle timeout | Await `next_line()` | Yields `LineEvent::Idle(1s)` within 5 s; `stop()` terminates child and pid disappears | `src/process/supervisor.rs` `tc_1782_idle_watchdog_fires_for_silent_child` |
| TC-1783 | FR-1783 | unit | Child emitting 5 lines at 300 ms intervals with 1 s idle timeout | Await `next_line()` | Yields all 5 lines then `Eof` without `Idle` | `src/process/supervisor.rs` `tc_1783_each_line_resets_idle_timer` |
| TC-1784 | FR-1784 | unit | Child emitting lines at 200 ms with 5 s idle and 1 s total timeout | Await `next_line()` in loop | Yields `LineEvent::Timeout(1s)` | `src/process/supervisor.rs` `tc_1784_total_timeout_fires` |
| TC-1785 | FR-1785 | unit | Child backgrounding grandchild `sleep 30` and echoing done | Read done, call `finish(500ms)` | Returns within 6 s and grandchild pid disappears | `src/process/supervisor.rs` `tc_1785_finish_reaps_grandchild_holding_stdout` |
| TC-1786 | FR-1786 | unit | Child writing 200 KiB to stderr ending with TAIL-MARK | Finish child and call `stderr_tail()` | Child does not block; tail length <= 1024 bytes and ends with TAIL-MARK | `src/process/supervisor.rs` `tc_1786_stderr_tail_keeps_last_bytes` |
| TC-1787 | FR-1787 | unit | Child ignoring SIGTERM with 300 ms term_grace | Call `stop()` | Returns within 5 s after escalating to SIGKILL and child pid disappears | `src/process/supervisor.rs` `tc_1787_stop_escalates_to_kill` |
| TC-1788 | FR-1788 | unit | Running supervised child | Drop `Supervised` handle | Pid disappears within 3 s and group is removed from `live_groups()` | `src/process/supervisor.rs` `tc_1788_drop_kills_group` |
| TC-1789 | FR-1789 | unit | Children spawned with parent_death_signal true and false | Spawn, read output, and stop | Both start, output lines, and stop normally | `src/process/supervisor.rs` `tc_1789_child_options_parent_death_signal_spawns` |
| TC-1790 | FR-1790, FR-1791 | integration (unix) | `pidsleep` fake agent | Send SIGTERM to `dmuxai run` | Exit code 130, JSON `exit_code` 130, cancelled event, both PIDs gone | `tests/agent_api_cli.rs` `tc_1790_sigterm_cancels_headless_run_and_reaps_runner` |
| TC-1791 | FR-1790, FR-1791 | integration (unix) | `pidsleep` fake agent | Send SIGHUP to `dmuxai run` | Exit code 130, JSON `exit_code` 130, cancelled event, both PIDs gone | `tests/agent_api_cli.rs` `tc_1791_sighup_cancels_headless_run_and_reaps_runner` |
| TC-1792 | FR-1792 | manual / code review | `dmuxai run` with a hung runner | Wait 10 s after signal or send second signal | `stop_all_live_groups` invoked and process exits 130 | manual / covered by code review |
| TC-1793 | FR-1793 | unit | Default config and settings | Assert equality | `CursorAdapterConfig::default().protocol == DmuxSettings::default().cursor_protocol` | `src/agent/cursor.rs` `tc_1793_cursor_adapter_default_protocol_matches_settings` |
| TC-1794 | FR-1794 | integration (unix) | Silent fake custom agent sleeping 30 s with `idle_timeout_secs = 1`, `timeout_secs = 60` | Run turn | Turn ends within 10 s with `AgentErrorKind::Idle` naming the agent and `idle_timeout_secs`, child process group is gone | `src/agent/custom.rs` `tc_1794_silent_custom_agent_fails_idle_naming_agent` |
| TC-1795 | FR-1795 | integration (unix) | Fake custom agent printing one JSONL line every 400 ms for ~2.5 s then exiting 0, `idle_timeout_secs = 1` | Run turn | Turn succeeds, all text received without `Idle` timeout | `src/agent/custom.rs` `tc_1795_idle_timer_resets_on_each_line` |
| TC-1796 | FR-1796 | unit / integration | Fake custom agent with `idle_timeout_secs = 0` silent for 2 s then printing text line; and spec parsing tests | Run turn and parse specs | Turn succeeds with text; parsing gives unset -> `Some(300)`, 0 -> `None`, 45 -> `Some(45)` | `src/agent/custom.rs` `tc_1796_idle_timeout_zero_disables_watchdog`, `src/custom_agent.rs` `tc_1796_idle_timeout_secs_defaults` |
| TC-1797 | FR-1797 | integration (unix) | Fake custom agent starting `sleep 60 &` (inheriting stdout), writing PID to file, printing text line and exiting 0 without `done_when` | Run turn | Turn finishes within 10 s with the text, grandchild PID is gone | `src/agent/custom.rs` `tc_1797_grandchild_holding_stdout_still_finishes` |
| TC-1798 | FR-1798 | unit (unix) | Running child in its own group; grandchild ignoring SIGTERM | `group_alive`/`GroupGuard` registry; `finish` reaps ignoring grandchild | Guard registers/unregisters; group_alive true while running and false after stop; finish kills grandchild within 5 s | `src/process/supervisor.rs` `tc_1798_group_alive_and_guard_registry`, `tc_1798_finish_kills_grandchild_ignoring_term` |
| TC-1799 | FR-1799 | integration (unix) | Fake agy turn running | Run turn | While running pgid is in `live_groups()`; after turn ends/cancelled it is not and process is gone | `src/agent/agy.rs` `tc_1799_runner_groups_registered_while_running` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Signal safety | `libc::killpg` rejects 0, 1, and own process group; `pre_exec` uses async-signal-safe calls only | Backward compatible; no process or shell command regressions | TC-1779, TC-1789 |
| Process groups | Every child is isolated in its own process group (`process_group(0)`) | Groups clean up on stop or drop; registry tracks active groups | TC-1780, TC-1788 |
| Concurrency | Registry and stderr ring buffer use poison-safe lock recovery | Non-blocking stderr drain prevents pipe deadlocks | TC-1780, TC-1786 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Unix dependency and process supervisor API | `Cargo.toml`, `src/process/mod.rs`, `src/process/supervisor.rs` | FR-1779, FR-1780, FR-1789 | `tc_1779`, `tc_1780`, `tc_1789` |
| 2 | Output streaming, watchdogs, and stderr tail | `src/process/supervisor.rs` | FR-1781, FR-1782, FR-1783, FR-1784, FR-1786 | `tc_1781`, `tc_1782`, `tc_1783`, `tc_1784`, `tc_1786` |
| 3 | Group termination, cleanup, and drop safety | `src/process/supervisor.rs` | FR-1785, FR-1787, FR-1788 | `tc_1785`, `tc_1787`, `tc_1788` |
| 4 | Custom agent runner port and idle watchdog | `src/custom_agent.rs`, `src/agent/custom.rs`, `docs/agents/custom-agents.md`, `src/init.rs` | FR-1794, FR-1795, FR-1796, FR-1797 | `tc_1794`, `tc_1795`, `tc_1796`, `tc_1797` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-1779..FR-1799) map to passing test evidence (TC-1779..TC-1799).
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [x] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
