# PLAN: turn outcome without a final reply (#146) and narrow catalog-hint reclassification

Status: complete  
Specs: [SPEC-custom-agent-provider.md](../specs/SPEC-custom-agent-provider.md) (FR-2125),
[SPEC-model-resolver.md](../specs/SPEC-model-resolver.md) (FR-2126, FR-1729)

1. Extract the failure checks of `failure_reason` into a private
   `failure_signal` (JSONL error, `failure_stderr` match, non-zero exit, empty
   reply) and keep `failure_reason` as a thin wrapper so the existing tests and
   callers keep their meaning. (FR-2125, TC-2125)
2. Add the pure helper `classify_turn_outcome` plus the `TurnOutcome` enum in
   `src/custom_agent.rs`. It reuses the same signal order and only turns an
   empty reply into `SuccessWithoutReply` when the workspace changed; every
   other signal still fails. (FR-2125, TC-2125)
3. In `src/agent/custom.rs`, sample the workspace before the child starts and
   after it exits with `git status --porcelain=v1 -z --untracked-files=normal`
   (5 s timeout, `None` when git is missing or the workspace is not a repo) and
   feed the result into `classify_turn_outcome`. Only Implement (edit-allowed)
   turns sample the workspace; Ask/Plan keep the old behaviour. On
   `SuccessWithoutReply` emit the Notice `<label> finished without a final
   message; workspace files changed` and finish the turn as `Completed`.
   (FR-2125, TC-2126, TC-2127)
4. Add `dmuxai::runner_fallback::is_model_unavailable_message` (narrow matcher
   for "model not found", "unknown model", "invalid model", "not a valid
   model", "model X does not exist / is not available / not supported") and use
   it in `App::attach_model_catalog_hint`: the hint is still attached, but the
   kind is rewritten to `ModelUnavailable` only for those messages.
   (FR-2126, TC-2128, TC-2129)
5. Update the affected specs (`FR-1729` wording, new `FR-2125` / `FR-2126` rows
   and `TC-2125..TC-2129` rows), `docs/agents/custom-agents.md`, the
   CHANGELOG, and the review artifact. (all requirements)

## Risks and mitigations

- **A changed workspace is only weak evidence of work.** The fingerprint only
  tells that *something* in the working tree changed during the turn. It is
  therefore used only for the "empty reply" case, never to suppress a JSONL
  error, a `failure_stderr` match, or a non-zero exit.
- **The fingerprint must not be influenced by DmuxAI itself.** The temp config
  and system-prompt files are written to the system temp directory, outside the
  workspace, and they exist during both samples, so they cannot create a
  difference.
- **Cost.** Two `git status` calls per edit-allowed turn. They are cheap, run
  with a 5 s timeout, and are skipped entirely when the workspace is not a git
  repository (`None`) and for read-only turns.
- **Runner fallback must not be triggered by unrelated failures.** The kind
  rewrite is now gated on the error message, so `classify_failure` keeps seeing
  the original kind for provider/process failures.
- **The matcher must stay narrow.** `is_model_unavailable_message` is a
  dedicated regex; the generic `UNAVAIL_RE` (spawn failures) is untouched.

## Rollback

Code-only change with no persistence, schema, or provider-protocol change.
Reverting `src/custom_agent.rs`, `src/agent/custom.rs`, `src/runner_fallback.rs`
and `src/app.rs` restores the previous behaviour; no data migration or state
rollback is required. The docs/spec rows are reverted with the same commit.
