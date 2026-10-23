# SPEC-agy-idle-timeout

Status: Implemented · Dogfood: #8

## 1. Decision summary
Long AGY Implement turns could start a background command, such as `cargo build &`, and then wait on it with no output, hanging for 5+ minutes. dmuxai now (1) tells AGY in Implement mode to run commands in the foreground with a bounded time, and (2) aborts the turn when AGY produces no stdout event for an idle timeout. The child process is killed and a clear error is shown.

## 2. Scope

In scope: AGY adapter (`src/agent/agy.rs`). Out of scope: the other providers, which already stream continuously.

## 3. Terms and state model

- Idle timeout: the time since the last stdout line from the AGY child. Default 300 s. Override with the `DMUXAI_AGY_IDLE_TIMEOUT_SECS` environment variable (integer seconds; 0 disables it).

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-440 | Must | If no stdout line arrives for the idle timeout, the AGY child is killed and the turn fails with an error that contains `no events for <N>s` and mentions background commands. |
| FR-441 | Must | Each stdout line resets the idle timer. |
| FR-442 | Must | `DMUXAI_AGY_IDLE_TIMEOUT_SECS` overrides the default. `0` disables the timeout. Invalid values fall back to the default. |
| FR-443 | Should | The Implement prompt tells AGY not to start background or detached processes and to run commands in the foreground with a timeout. |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-440 | FR-440 | `agy_idle_timeout_kills_silent_child` (fake AGY script that prints one init event, then sleeps) |
| TC-441 | FR-441 | `agy_idle_timer_resets_on_each_event` |
| TC-442 | FR-442 | `agy_idle_timeout_env_parsing` |
| TC-443 | FR-443 | `implement_prompt_forbids_background_commands` |

## 6. Exit criteria
TCs pass in CI. Dogfood #8 is closed.
