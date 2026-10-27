# REVIEW: headless AGY plan-only runs must not report success

Status: complete  
Spec: [../specs/SPEC-plan-gate.md](../specs/SPEC-plan-gate.md) (FR-2121)  
Issue: GitHub #164  
Reviewed: 2026-10-25  
Target: Linux (`x86_64-unknown-linux-gnu`)

## Problem

`dmuxai run --agent agy --approve-plan` ended with `ok` / exit 0 when AGY
answered with its own plan review ("created plan.md, please review/approve")
and applied no file edit:

- the headless direct run sent `require_file_edits = false`, so AGY's existing
  one-shot stricter-edit retry never ran;
- `--approve-plan` only approves DmuxAI's own graph/sensitive gates and never
  reaches AGY, whose stdin is closed after the prompt;
- the plan-only reply therefore ended as a normal `Completed` turn and the
  headless run reported success.

## Spec compliance

- FR-2121, `--approve-plan` on a headless direct run with the write-capable
  policy: `run_headless` sets `headless_require_edits`, and
  `graph_node_expects_file_edits` returns true for it, so the agent request
  carries `require_file_edits = true`. AGY then applies its existing stricter
  edit instruction, retries once, and fails the turn with the policy error
  `AGY returned plan-only responses twice without applying file edits.`
  (non-zero exit). Evidence: TC-2121, TC-2124.
- FR-2121, without `--approve-plan`: the AGY adapter reports a plan-only
  completion (no observed writes, plan-like text, `Completed` terminal) with the
  `PLAN_ONLY_APPROVAL_NOTICE` marker notice. The headless run maps it to exit 7
  (`APPROVAL_REQUIRED`) with the error `AGY stopped at its own plan review
  without editing files; re-run with --approve-plan to let DmuxAI tell AGY to
  apply the plan`, and the plan text stays in `text`/`messages`. Evidence:
  TC-2122, TC-2123.
- FR-2121, boundaries: the marker notice is dropped before it becomes a
  transcript line, so the TUI and dashboard direct runs keep the FR-937 quiet
  behaviour (`direct_one_node_run_does_not_require_file_edits` still passes),
  read-only/question turns are unaffected
  (`agy_question_plan_review_text_does_not_force_edit_retry` still passes), and
  the headless flags are cleared when the run ends (asserted in TC-2121 and
  TC-2122).

## Regression and safety review

- The change touches `src/agent/mod.rs` (marker constant), `src/agent/agy.rs`
  (adapter notice + tests), `src/app.rs` (headless flags, notice handling, exit
  mapping, tests) and docs. Other runners (Codex, Cursor, custom), persistence,
  the graph scheduler, and the provider protocol are untouched.
- No persistence or provider protocol change: the marker is an in-process
  notice, so there is no database migration and no rollback path beyond
  reverting the code.
- The AGY retry logic itself is unchanged; the adapter only reports a plan-only
  completion that the app had no way to see before (observed writes and the raw
  result text live in the adapter).
- The stricter edit instruction is only added when the caller asked for edits
  (`--approve-plan`); a plain headless run still lets the agent answer with a
  plan and now reports "approval required" instead of a false success.
- Platform behaviour is unchanged: no process launch, sandbox or permission
  flag was modified.

## Test evidence

- `cargo check --all-targets`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo test --bin dmuxai -- tc_2121_headless_approve_plan_requires_file_edits
  tc_2122_headless_plan_only_reply_exits_seven
  tc_2123_agy_plan_only_reply_emits_approval_notice
  tc_2124_agy_plan_only_with_required_edits_fails_after_one_retry
  direct_one_node_run_does_not_require_file_edits
  agy_question_plan_review_text_does_not_force_edit_retry
  implement_plan_only_retries_once_then_fails
  implement_permission_denial_with_plan_like_response_is_not_a_plan_only_retry`:
  8 passed, 0 failed.
- `cargo test --bin dmuxai -- headless tc_212 plan_only`: 31 passed, 1 ignored
  (live grok smoke), 0 failed.
- `python3 scripts/check-spec.py --all`: 80 specs, 80 pass, 0 fail.
- `python3 scripts/check-tc-links.py`: 0 orphan, 0 undefined test, 0 stale.
- `bash scripts/check-spec.sh docs/specs/SPEC-plan-gate.md`: PASS.
- Not run for this change (out of the requested scope): the full
  `cargo test --all-targets` and `cargo clippy --all-targets --all-features
  -- -D warnings` gates. Only the narrow tests above and the affected
  headless/plan-only/notice subsets were executed.

## Residual risk and follow-ups

- Not manually verified against a live AGY account in this change: the
  end-to-end CLI path (`dmuxai run --agent agy --approve-plan` with a real AGY
  plan reply) is covered by unit tests with a fake AGY binary. A live smoke
  run is still worth doing before release.
- The plan-only text detection is the existing heuristic
  (`is_plan_review_response`); a reply that neither mentions a plan nor asks
  for a review is not classified as plan-only and keeps the old behaviour.
- Other runners do not emit the marker yet, so the exit-7 mapping only applies
  to AGY. A future runner can opt in by emitting the same marker notice.
