# PLAN: Runner fallback handover correctness

Status: complete  
Spec: [SPEC-runner-fallback.md](../specs/SPEC-runner-fallback.md)

1. Keep the handover state in one place (`PendingFallback`: chain plus the
   original prompt), consume it at the top of `try_submit_to_agent`, and clear
   it on every exit path of `try_runner_fallback`, so a stale chain can never
   reach a later turn. (FR-2130, TC-2130)
2. Carry the original user prompt in `ActiveTurn` (`fallback_original_prompt`)
   and build every hop's prompt as `preface(this hop) + original prompt`; the
   transcript always shows the original user text. (FR-2131, TC-2131)
3. Record the handover (Notice + Failed "handed over to ...", `fallback`
   event, graph-node rebind, turn retirement) only after the target turn
   started. A target that fails to start restores runner, model, selection and
   graph-node mapping and the next target is tried; when every target fails the
   original error is recorded exactly once and the turn is finished once.
   (FR-2132, TC-2132)
4. Apply the target's runner and model before submitting; a target whose
   runner or model cannot be applied is skipped with a system note.
   (FR-2133, TC-2133)
5. Build the headless answer text from the final runner's messages only; the
   failed runner's output stays in `messages`, and the CLI JSON prints the
   `fallbacks` array of the run. (FR-2134, FR-2119, TC-2134, TC-2136)
6. Merge the user and workspace `[runners.<id>]` tables field-wise: a field the
   workspace table does not set keeps the user value. (FR-2101, TC-2135)

## Risks and mitigations

- The handover now happens after the target turn starts. A target that is busy
  or fails to start is skipped instead of being recorded as a handover, so the
  "handed over to ..." line can no longer precede a turn that never ran. The
  exhausted chain still records exactly one `FallbackRecord`.
- The failed turn is retired only after a successful handover. While the target
  is being tried, the failed turn stays active, so the same-runner handover
  still finishes it explicitly (the engine refuses two running turns of one
  runner).
- `try_submit_to_agent` consumes the pending display text and the pending
  fallback state before its first early return, so neither can leak into a
  later turn.
- The workspace/user config merge changes precedence: a workspace table that
  sets only the stall timers keeps the user's chain. An explicit workspace
  `fallback = []` still disables the chain, and an explicit chain still
  replaces the user chain.

## Rollback

No persistence migration and no provider protocol change. The turn records keep
their existing shape; only the order of the handover bookkeeping changes, so
reverting `src/app.rs`, `src/cli.rs` and `src/runner_fallback.rs` restores the
old behavior without a state rollback.
