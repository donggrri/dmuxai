# REVIEW: turn outcome without a final reply (#146) and narrow catalog-hint reclassification

Status: complete  
Specs: [SPEC-custom-agent-provider.md](../specs/SPEC-custom-agent-provider.md) (FR-2125),
[SPEC-model-resolver.md](../specs/SPEC-model-resolver.md) (FR-2126, FR-1729)  
Reviewed: 2026-10-25  
Target: Linux (`x86_64-unknown-linux-gnu`); the change is platform-neutral

## Spec compliance

- FR-2125 (custom agent, empty reply + workspace change = success with notice):
  `src/custom_agent.rs` now has `TurnOutcome` and the pure
  `classify_turn_outcome(spec, exit_ok, reply_text, stderr, jsonl_error,
  workspace_changed)`. It reuses the failure checks of `failure_reason` in the
  same order (JSONL error -> `failure_stderr` match -> non-zero exit -> empty
  reply) through the shared private `failure_signal` helper, and
  `failure_reason` stays a thin wrapper with identical reasons. Only the empty
  reply differs: with `workspace_changed` it yields `SuccessWithoutReply`,
  without it `Failed("empty reply text")`.
  `src/agent/custom.rs` `run_turn` samples
  `git status --porcelain=v1 -z --untracked-files=normal` in the turn's cwd
  before the child starts and after it exits (5 s timeout; `None` when git is
  missing, the directory is not a repository, or the command fails), and only
  for Implement (edit-allowed) turns. On `SuccessWithoutReply` it emits
  `AgentEvent::Notice("<label> finished without a final message; workspace files
  changed")` and returns `Ok(())`, so the adapter finishes the turn with
  `AgentEvent::Completed` (not `Error`); headless runs report `ok: true` and
  the notice is the only user-visible text of that turn (no garbage text is
  produced).
- FR-1849 (unchanged failure rules): `failure_reason` keeps its order and its
  reasons, and the existing test `test_failure_reason` still passes.
- FR-2126 (catalog hint reclassification): `App::attach_model_catalog_hint`
  still attaches the "model <m> is not offered by ..." hint to the error, but
  the kind is rewritten to `AgentErrorKind::ModelUnavailable` only when
  `dmuxai::runner_fallback::is_model_unavailable_message(&err.message)` is true
  and the kind is `Provider` or `ProcessExit`. The matcher covers "model not
  found", "unknown model", "invalid model", "not a valid model", "model X does
  not exist", "model X is not available" and "model X not supported", and
  rejects generic provider, quota, auth and policy messages. The check runs on
  the error message before the hint is attached, so the hint text itself can
  never trigger the rewrite.
- FR-1729 (reworded): the catalog warning is still pushed before the turn and
  the hint is still attached to a failing turn; only the kind rewrite is now
  conditional.

## Regression and safety review

- The change is confined to `src/custom_agent.rs` (pure classification),
  `src/agent/custom.rs` (fingerprint + notice), `src/runner_fallback.rs` (new
  matcher), `src/app.rs` (hint attachment) plus tests, the fake-agent fixture
  and docs. No persistence, schema, or provider-protocol change.
- No unrelated refactor: `failure_reason` keeps its signature and semantics,
  and the only change to `src/app.rs` is inside `attach_model_catalog_hint`.
- Failure precedence is preserved: a JSONL error, a `failure_stderr` match and
  a non-zero exit still fail the turn even when the workspace changed; only a
  clean exit 0 with an empty reply can become a success.
- Read-only (Ask/Plan) turns never sample the workspace and keep failing on an
  empty reply.
- DmuxAI's own temp config and system-prompt files are written to the system
  temp directory (outside the workspace) and exist during both fingerprint
  samples, so they cannot create a false "workspace changed" signal.
- The fallback chain is protected: an unrelated provider failure now keeps the
  `provider` kind, so `classify_failure` does not treat it as a fallback
  trigger and no other runner is handed the turn.
- Platform behaviour: the fingerprint shells out to `git`; on a machine
  without git, or outside a repository, the fingerprint is `None` and the
  empty reply stays a failure (the old behaviour).

## Test evidence

- `cargo test --all-targets tc_212` (new tests): 5 passed, 0 failed --
  `tc_2125_classify_turn_outcome_table`,
  `tc_2126_empty_reply_with_workspace_change_succeeds`,
  `tc_2127_empty_reply_without_workspace_change_fails`,
  `tc_2128_model_unavailable_message_matcher`,
  `tc_2129_generic_provider_error_keeps_provider_kind_with_hint`.
- `cargo test --all-targets tc_1748` (adjusted existing tests): 2 passed --
  `tc_1748_notlisted_error_gets_hint` and
  `tc_1748_drain_error_kind_is_model_unavailable` now use a "model not found"
  message; the hint is attached and the kind is `model_unavailable`.
- `cargo test --all-targets agent::custom::tests`: 35 passed, 1 ignored (live
  grok smoke), 0 failed.
- `cargo test --all-targets custom_agent::tests`: 45 passed, 0 failed.
- `cargo test --all-targets runner_fallback::tests`: 12 passed, 0 failed.
- `cargo test --all-targets catalog`: 6 passed, 0 failed
  (`tc_1738`, `tc_1739`, `tc_1746`, `tc_1747`, per-agent catalog test).
- `cargo test --all-targets fallback`: 26 passed, 0 failed (runner-fallback
  classification, settings and app fallback tests).
- `cargo check --all-targets`: passed (no warnings).
- `cargo fmt --all -- --check`: passed.
- `python3 scripts/check-spec.py --all`: 80 specs, 80 pass, 0 fail, 0
  allowlisted, 0 stale.
- `python3 scripts/check-tc-links.py`: 867 defined, 215 strong, 424
  comment-only, 228 allowlisted, 0 orphan, 0 undefined-test, 0 stale.
- Not run (per the task scope): the full test suite and clippy. Live provider
  smoke tests stay `#[ignore]` and need no credentials for the deterministic
  tests above.

## Residual risks and rollback

- A changed working tree is weak evidence: a turn that changes nothing but
  still produces no reply stays a failure, and a turn that changes the
  workspace for an unrelated reason (for example a background process touching
  the tree) is reported as "finished without a final message" instead of an
  error. The notice names the situation, so the user can inspect `git status`.
- The workspace fingerprint costs two `git status` calls per edit-allowed
  turn; on very large repositories this adds a small delay (bounded by the 5 s
  timeout, after which no evidence is available and the turn keeps the old
  behaviour).
- The model-unavailable matcher is intentionally narrow: a provider that
  reports an unknown model in an unexpected wording keeps its original error
  kind and is therefore not reclassified; the catalog hint is still shown.
- Rollback is code-only: revert the four source files (plus the fixture and
  docs). No database, schema, or provider-protocol migration is involved.
