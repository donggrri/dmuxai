# TASKS: AGY idle timeout for silent headless turns

Status: complete  
Spec: [SPEC-agy-idle-timeout.md](../specs/SPEC-agy-idle-timeout.md)

- [x] T1 - Resolve the idle timeout from `DMUXAI_AGY_IDLE_TIMEOUT_SECS`
  (default 300 s, `0` disables, invalid/non-positive values use the default).
  Requirements: FR-442. Verify: `cargo test agy_idle_timeout_env_parsing`.
- [x] T2 - Arm an absolute idle deadline in `stream_child`, re-arm it on every
  stdout line, and on expiry kill the child and fail the turn with
  `no events for <N>s` plus background-command guidance. Requirements: FR-440,
  FR-441. Verify: `cargo test agy_idle_timeout_kills_silent_child` and
  `cargo test agy_idle_timer_resets_on_each_event`.
- [x] T3 - Forbid background/detached processes in the Implement prompt and
  require foreground commands with a bounded timeout. Requirements: FR-443.
  Verify: `cargo test implement_prompt_forbids_background_commands`.
- [x] T4 - Update the existing Implement-prompt assertion for the new prefix,
  then run the full gates. Requirements: all. Verify: `cargo fmt --check`,
  `cargo test --all-targets`, and
  `cargo clippy --all-targets --all-features -- -D warnings`.
