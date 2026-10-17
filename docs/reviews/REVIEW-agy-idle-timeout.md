# REVIEW: AGY idle timeout for silent headless turns

Status: complete  
Spec: [SPEC-agy-idle-timeout.md](../specs/SPEC-agy-idle-timeout.md)  
Reviewed: 2026-10-17  
Target: Linux (`x86_64-unknown-linux-gnu`) tests; adapter is platform-neutral

## Spec compliance

- FR-440: `stream_child` arms an absolute idle deadline before the select loop.
  When it fires the AGY child is killed and the turn fails with
  `AGY produced no events for <N>s; it may be waiting on a background
  command...` (message contains `no events for <N>s` and background-command
  guidance).
- FR-441: every received stdout line re-arms the deadline
  (`Instant::now() + secs`). Stderr lines deliberately do not reset it; only
  stdout proves the agent is making headroom progress.
- FR-442: `parse_idle_timeout_secs` resolves
  `DMUXAI_AGY_IDLE_TIMEOUT_SECS` — unset → 300 s (default), `0` → disabled,
  valid positive integer → that value, invalid/non-positive → default.
- FR-443: `agy_prompt` prepends foreground/bounded-timeout and no-background /
  no-detached instructions to Implement prompts; Ask and Plan prompts are
  unchanged.

## Regression and safety review

- Change is confined to `src/agent/agy.rs` (deadline handling, env parsing,
  Implement prompt wording) plus tests and docs. Codex/Cursor/custom adapters,
  persistence, and TUI flows are untouched.
- Deadline is absolute and re-armed per stdout line, so a continuously
  streaming but slow turn is never aborted mid-flight; stderr-only noise keeps
  the deadline armed.
- Kill path: `child.kill()` result is deliberately ignored before returning the
  timeout error so a terminating child cannot mask the actionable message, and
  `kill_on_drop` remains as the backstop.
- Sandbox/permission policy is unchanged: only prompt wording for Implement
  mode changed, and it does not weaken any sandbox or approval boundary.
- The two spawn-based tests share an env lock and save/restore
  `DMUXAI_AGY_BIN` / `DMUXAI_AGY_IDLE_TIMEOUT_SECS` to avoid process-global env
  races between parallel tests.

## Test evidence

- `cargo test --all-targets`: passed; 147 binary + 76/19/10/3/4/4/0 library and
  integration tests, 0 failures; 5 authenticated provider smoke tests ignored
  as intended. Date: 2026-10-17.
- `agy_idle_timeout_kills_silent_child` (TC-440): passed — fake AGY prints one
  activity line and sleeps; turn errors with `no events for 1s` + background
  guidance and `kill -0` confirms the child PID is gone.
- `agy_idle_timer_resets_on_each_event` (TC-441): passed — cumulative runtime
  (≈2.1 s) exceeds the 2 s timeout while each stdout gap (0.7 s) does not.
- `agy_idle_timeout_env_parsing` (TC-442): passed — default, disable, valid,
  trim, invalid, negative, fractional, and empty inputs.
- `implement_prompt_forbids_background_commands` (TC-443): passed — Implement
  prompt requires foreground/bounded commands, forbids background/detached
  processes, keeps the original prompt, and Ask/Plan are untouched; the
  existing `agy_execution_modes_use_native_cli_flags_and_safe_prompts`
  assertion was updated for the new prefix (behavior change is intended).
- `cargo fmt --check`: passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-agy-idle-timeout.md`: not
  run; `pwsh` is unavailable on this Linux worktree (PowerShell is not
  installed). Spec text itself was not modified in this change.
- Live AGY smoke test (`real_agy_turn_streams_to_completion`): left ignored as
  intended; requires an installed, authenticated AGY CLI.

## Residual risks and rollback

- The default 300 s idle timeout keeps turning for long quiet stretches of a
  legitimately slow, silent provider (e.g., a long model-generation pause
  without stream events); `DMUXAI_AGY_IDLE_TIMEOUT_SECS=0` restores the old
  unbounded behavior per user if a false positive is ever observed in
  dogfooding.
- Timing-based TC-441 uses generous margins (0.7 s gaps vs 2 s timeout) and the
  timeout error path cannot fail before the child is silent for 1–2 s, so CI
  jitter is unlikely to flip results.
- Rollback is code-only: revert `src/agent/agy.rs`; no persistence, schema, or
  provider protocol changes are involved.
