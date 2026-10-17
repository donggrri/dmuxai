# PLAN: AGY idle timeout for silent headless turns

Status: complete  
Spec: [SPEC-agy-idle-timeout.md](../specs/SPEC-agy-idle-timeout.md)

1. Add an idle-timeout resolver (`DMUXAI_AGY_IDLE_TIMEOUT_SECS`, default 300 s,
   `0` disables, invalid values fall back to the default) and expose it to the
   AGY stream loop. (FR-442, TC-442)
2. Wrap the `stream_child` select loop with an idle deadline: every stdout line
   re-arms the deadline, and when it fires the child is killed and the turn
   fails with `no events for <N>s` plus guidance about background commands.
   (FR-440, FR-441, TC-440, TC-441)
3. Prepend an instruction to the Implement prompt that forbids background or
   detached processes and requires foreground commands with a bounded timeout.
   (FR-443, TC-443)
4. Update the existing Implement-prompt assertion, write the review artifact,
   and run the Rust gates. (all requirements)

## Risks and mitigations

- The deadline must not fire while the child streams normally: the deadline is
  absolute (`Instant::now() + secs`) and is re-armed on every stdout line, so a
  slow but continuous stream is never aborted.
- Environment variables are process-global: the two spawn-based tests share a
  tokio mutex and save/restore the affected variables so tests stay isolated.
- Killing an already-exited child must not mask the timeout error: the kill
  result is deliberately ignored before returning the timeout error.
- The TUI itself is untouched; only the AGY adapter changes, so other
  providers and the persistence layer are unaffected.

## Rollback

Code-only change with no persistence or provider protocol migration. Revert the
adapter changes to restore the previous unbounded wait behavior; no database or
state rollback is required.
