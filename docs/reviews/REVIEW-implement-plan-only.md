# REVIEW: Implement plan-only and AGY terminal lifecycle

Spec: [SPEC-implement-plan-only.md](../specs/SPEC-implement-plan-only.md)
Date: 2026-10-17

## Spec compliance

| Requirement | Status | Notes |
| --- | --- | --- |
| FR-500 | Met | Implement `agy_prompt` requires direct edits |
| FR-501 | Met | `observe_agy_message` on write tool names |
| FR-502 | Met | Single retry via `run_turn` loop + strict prompt |
| FR-503 | Met | `pending_terminal` + grace kill in `stream_child` |
| FR-504 | Met | Codex/Cursor/AGY/custom concurrent `Send` → `Error` |
| FR-505 | Met | `cancel_selected` clears tracking and calls adapter cancel |
| FR-506 | Met | Terminal `Error` fails turn/graph via existing `finish_turn_outcome` |
| FR-507 | Met | Plan mode notice preserved; terminal deferred |
| FR-508 | Met | `finish(..., require_writes)` unchanged for non-plan-only |

## Regression risk

- Medium: AGY turn lifecycle timing changed; mitigated by grace + idle tests.
- Low: concurrent send now finishes turns on `Error` (more visible than Notice).

## Test evidence

```text
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```

All tests passed on Linux CI agent environment (2026-10-17).

## Residual risks

- Live AGY plan-only phrasing outside heuristics may fail without retry value; manual dogfood on Implement edits recommended.

## Review round 2 (2026-10-17)

Re-review after round 1; each finding was verified against the current code and
its fix is covered by the named tests (existing FR/TC ids only).

| # | Finding | Fix | Test evidence |
| --- | --- | --- | --- |
| 1 | Grace kill after a SUCCESS `result` failed an Implement mode turn | `finish()` now accepts `exit_forced_after_result` once the terminal-deferring `result` was parsed | `agy_implement_grace_kill_after_success_result_completes`, `finish_accepts_forced_kill_after_result_in_implement_mode` (TC-505) |
| 2 | `edit_file` / `write_file` / accept-edits needles for the Cursor/custom text scan were dropped during the write-detection change | Needles restored for both providers | `provider_line_write_needles_cover_cursor_and_custom` (TC-501) |
| 3 | Grace branch spun on `child.try_wait()` in a busy loop | Grace branch now blocks on `child.wait()` instead of polling | Covered by the grace/kill tests under TC-505 |
| 4 | stderr could stall behind `stdout` after `result` | stderr keeps draining while the terminal outcome is deferred | Covered by the grace/kill tests under TC-505 |
| 5 | The app-level already-running test sent the `Error` with the first request's ids, which never happens against real adapters | Test rewritten: the `Error` arrives with the second request's conversation/turn ids; the tracked (second) entry is finished and `active_turns` holds neither request | `already_running_adapter_error_clears_busy_state` (FR-504) |
| 6 | After a normal exit, a grandchild holding stderr could hang `stream_child` (grace arm was guarded by `exit_status.is_none()`) | Grace arm fires whenever the result deadline passes; kills/reaps only if the child is still alive; always closes both pipes | `agy_grace_closes_when_grandchild_holds_stderr_after_exit` (TC-505) |

Notes:
- Grace-test timing is now measured inside the env lock to avoid
  suite-parallelism flakes (elapsed no longer includes other tests' lock wait).
- Who did what: AGY (gemini-3.8-flash-high) fixed findings 1/3/4 plus their
  tests; opencode deepseek-v4.1-flash-free fixed finding 2 and performed the
  cross-review that found row 6; opencode glm-5.3-flash-free fixed finding 5
  and the docs, and performed the final cross-check.

Fmt drift in the touched files was also fixed.
