# REVIEW: the headless plan-only marker needs an explicit approval request

Status: complete  
Spec: [../specs/SPEC-plan-gate.md](../specs/SPEC-plan-gate.md) (FR-2121)  
Issue: GitHub #164 (SQA follow-up on PR #197)  
Reviewed: 2026-10-25  
Target: Linux (`x86_64-unknown-linux-gnu`)  
Follows: [REVIEW-headless-agy-plan-only.md](REVIEW-headless-agy-plan-only.md)

## Problem found by QA

The headless exit-7 marker of PR #197 used `is_plan_review_response`
(`src/agent/implement_outcome.rs`), which is far too loose: 7 of 10 ordinary
answers were classified as plan-only, and a Korean plan review was missed.
The marker therefore turned ordinary review answers into an "approval
required" exit, while a `계획 작성 + 검토 후 승인` reply was not detected at
all.

## Spec ID renumbering (after the rebase)

After the rebase onto `main`, the two FR-2121 test cases of this branch
collided with the FR-2125 cases of
[../specs/SPEC-custom-agent-provider.md](../specs/SPEC-custom-agent-provider.md),
which already own `TC-2125` and `TC-2126` (a spec ID may be defined in only
one SPEC). The FR-2121 cases are therefore renumbered `TC-2125` -> `TC-2145`
and `TC-2126` -> `TC-2146`; both numbers are unused on `main`. Only the
FR-2121 IDs changed, together with their test functions
(`tc_2145_plan_approval_request_requires_plan_and_approval`,
`tc_2146_agy_retry_separates_the_two_answers`). The FR-2125 cases keep their
IDs, their functions and their spec rows.

## Spec compliance

- FR-2121, exit-7 trigger: `is_plan_approval_request`
  (`src/agent/implement_outcome.rs`) requires **both** halves. The plan
  statement side matches an agent-written or agent-presented plan
  (`created|wrote|written|drafted|saved|added|prepared|produced … plan`,
  `here is/are … plan`, `proposed … plan`, `plan … below`,
  `implementation plan … created|below|here`, and the Korean
  `계획 … 작성|수립|만들|아래`). The approval side matches an explicit request
  addressed to the user (`please review|approve|confirm`, `approve it`,
  `shall I`, `should I proceed`, `let me know if I should`, `before I edit`,
  `once you approve`, `waiting for (your) approval`, and the Korean `승인`,
  `검토 후`, `진행할까요`, `진행해도 될까요`, `확인해 주세요`). A bare
  `plan.md`, `review`, `proceed` or `confirm` is not enough, so a review
  report, a missing `plan.md`, an instruction to the user (`to proceed, run
  cargo test`) and a finished edit stay ordinary answers. Evidence: TC-2145
  (ten ordinary answers, six plan-approval replies), TC-2123 (adapter level:
  an ordinary answer emits no marker).
- FR-2121, marker wiring: only the headless exit-7 marker uses the new
  classifier. `run_turn` (`src/agent/agy.rs`) now has two separate paths: the
  marker path (`!request.require_file_edits`) uses
  `is_plan_approval_request` plus "no observed writes" plus a `Completed`
  terminal, and the stricter-edit retry path (turns that require file edits)
  keeps `is_plan_only_implement_outcome` / `is_plan_review_response`
  unchanged (`implement_plan_only_retries_once_then_fails` and
  `agy_question_plan_review_text_does_not_force_edit_retry` still pass).
- FR-2121, additional signal: `agy_message_indicates_plan_artifact` reads the
  AGY `step_update` shapes already parsed in `src/agent/agy.rs` (a plan or
  artifact step type, or a plan/artifact tool step) and can stand in for the
  textual plan statement. It is never required: the text classifier alone
  decides, and the structural signal only adds a positive when the reply asks
  for an approval.
- FR-2121, headless note: `HEADLESS_PLAN_ONLY_NOTE` (`src/app.rs`) is now
  `AGY replied with a plan and made no file edits. If you wanted changes
  applied, re-run with --approve-plan; if this was a question, the reply is in
  "text".` The old wording ("AGY stopped at its own plan review without
  editing files; …") is only quoted in the historical review of PR #197,
  which stays untouched as past evidence.
- FR-2121, retry transcript: the stricter-edit retry emits a blank-line
  separator (`"\n\n"`) between the two answers, so the headless output is
  `…approve it.\n\nCreated plan.md…` instead of two glued answers. Evidence:
  TC-2146.
- FR-2121, help and docs: `dmuxai --help` now lists exit `7` as "plan
  approval, sensitive gate, or an agent that stopped at its own plan review";
  `docs/API.md` (run bullet and exit-code table), `SPEC-plan-gate.md`
  (FR-2121, TC-2123, TC-2145, TC-2146), `SPEC-agent-api.md` (FR-1810) and
  the changelog entry under Unreleased match the new behaviour.

## Regression and safety review

- Changed files: `src/agent/implement_outcome.rs` (classifier, artifact
  signal, tracker field), `src/agent/agy.rs` (marker condition, separator,
  tests), `src/app.rs` (note text), `src/main.rs` (help text), and docs. No
  persistence, no provider protocol and no other runner changed.
- The marker stays an in-process notice: the interactive TUI and dashboard
  direct runs keep the FR-937 quiet behaviour (`direct_one_node_run_does_not_require_file_edits` still passes), and read-only turns are unaffected.
- Platform behaviour is unchanged: no process launch, sandbox or permission
  flag was modified.
- Per-agent isolation is unchanged: the classifier sees only the agent reply
  of its own turn.
- Rollback: code-only, no migration. Reverting `src/agent/agy.rs` and
  `src/agent/implement_outcome.rs` restores the loose marker of PR #197.

## Test evidence

- `cargo check --all-targets`: passed (no warnings).
- `cargo fmt --all -- --check`: passed.
- `cargo test --bin dmuxai -- tc_2121_headless_approve_plan_requires_file_edits
  tc_2122_headless_plan_only_reply_exits_seven
  tc_2123_agy_plan_only_reply_emits_approval_notice
  tc_2123_agy_ordinary_plan_answer_does_not_emit_approval_notice
  tc_2124_agy_plan_only_with_required_edits_fails_after_one_retry
  tc_2145_plan_approval_request_requires_plan_and_approval
  tc_2146_agy_retry_separates_the_two_answers
  plan_approval_request_needs_both_halves
  agy_plan_artifact_steps_are_detected_from_step_updates
  direct_one_node_run_does_not_require_file_edits
  agy_question_plan_review_text_does_not_force_edit_retry
  implement_plan_only_retries_once_then_fails
  implement_permission_denial_with_plan_like_response_is_not_a_plan_only_retry`:
  13 passed, 0 failed.
- `cargo test --bin dmuxai -- tc_212 plan_only plan_approval agy_ headless`:
  62 passed, 2 ignored (live grok / live AGY smoke), 0 failed.
- `python3 scripts/check-spec.py --all`: 80 specs, 80 pass, 0 fail.
- `python3 scripts/check-tc-links.py`: 868 defined, 216 strong, 424
  comment-only, 228 allowlisted, 0 orphan, 0 undefined test, 0 stale.
- `bash scripts/check-spec.sh docs/specs/SPEC-plan-gate.md`: PASS.
- `bash scripts/check-spec.sh docs/specs/SPEC-agent-api.md`: PASS.
- Not run for this change (outside the requested scope): the full
  `cargo test --all-targets` and the clippy gate
  (`cargo clippy --all-targets --all-features -- -D warnings`). Only the
  narrow tests and the affected subsets above were executed.

## Residual risk and follow-ups

- The classifier is text-based. A plan review with wording outside the
  anchored patterns (for example "let me know if this looks right") is not
  detected and keeps the old behaviour (exit 0 with the reply in `text`).
  The structural plan-artifact signal covers part of that gap, but only when
  the AGY stream reports a plan or artifact step, which no live AGY session
  has been observed to emit yet.
- Not verified against a live AGY account in this change: the end-to-end CLI
  path (`dmuxai run --agent agy` with a real AGY plan reply) is covered by
  unit tests with a fake AGY binary. A live smoke run is still worth doing
  before release.
- The exit-7 wording for the plan-review case is stable per language
  (English and Korean); other languages would need their own anchors.
