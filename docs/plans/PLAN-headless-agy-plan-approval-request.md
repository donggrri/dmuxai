# PLAN: the headless plan-only marker needs an explicit approval request

Status: complete  
Spec: [../specs/SPEC-plan-gate.md](../specs/SPEC-plan-gate.md) (FR-2121)  
Issue: GitHub #164 (SQA follow-up on PR #197)  
Follows: [PLAN-headless-agy-plan-only.md](PLAN-headless-agy-plan-only.md)

1. Add a stricter classifier `is_plan_approval_request(response)` in
   `src/agent/implement_outcome.rs`: a plan statement (the agent wrote or
   presents a plan) **and** an explicit approval request addressed to the user,
   each matched by anchored regexes in English and Korean. A bare `plan.md`,
   `review`, `proceed` or `confirm` is not enough. (FR-2121)
2. Use the new classifier (plus "no observed writes") only for the headless
   exit-7 marker in `run_turn` (`src/agent/agy.rs`). The stricter-edit retry
   path for turns that require file edits keeps `is_plan_review_response` /
   `is_plan_only_implement_outcome` unchanged. (FR-2121)
3. Use a structural AGY plan-artifact signal (`step_update` plan/artifact step
   or a plan/artifact tool step) as an additional positive signal for the
   marker, never as a requirement. (FR-2121)
4. Replace the headless plan-only note in `src/app.rs` with the safer wording
   that points at `--approve-plan` and at the reply text. (FR-2121)
5. Separate the two answers of one stricter-edit retry with a blank line so the
   transcript and the headless output are not glued together. (FR-2121)
6. Add the tests, update [../specs/SPEC-plan-gate.md](../specs/SPEC-plan-gate.md)
   (FR-2121, TC-2145, TC-2146), [../API.md](../API.md), the `dmuxai --help`
   exit-code text in `src/main.rs`, and the changelog. (FR-2121)

## Risks and mitigations

- The stricter classifier can miss a real plan review that uses other wording.
  Mitigation: the marker is only a headless verdict, the reply text stays in the
  output, and the note tells the caller to re-run with `--approve-plan`.
  A structural plan-artifact event in the AGY stream is an additional positive
  signal for the wording the text classifier misses.
- The stricter classifier can also over-trigger. Mitigation: both halves are
  required and the anchors avoid bare `review`/`proceed`/`plan.md`;
  TC-2145 pins ten ordinary answers and six plan-approval replies.
- The retry separator changes the transcript shape. Mitigation: the separator is
  only a blank line between two answers of the same turn; TC-2146 pins it and
  the existing retry tests still pass.
- No persistence change and no provider protocol change: the classifier is a
  pure function over the reply text and the marker stays an in-process notice.

## Rollback

Code-only change without migration: revert the classifier, the marker
condition, the separator, and the note text. The exit-7 behaviour of the
previous PR (a loose plan-only marker) is restored by reverting
`src/agent/agy.rs` and `src/agent/implement_outcome.rs` only.
