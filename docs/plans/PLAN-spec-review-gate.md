# PLAN: spec-review-gate

Status: part 1 (core, CLI, MCP flag) and part 2 (board, dashboard, TUI, #374) complete
Owner: DmuxAI
Created: 2026-11-28
Spec: [SPEC-spec-review-gate](../specs/SPEC-spec-review-gate.md)

## 1. Goal

Part 1 delivers the human spec review gate for headless runs: FR-3700 .. FR-3710,
FR-3717, NFR-3700, NFR-3701. The board child uses the same headless path, so it
pauses without extra code. Board lanes, dashboard endpoints and the TUI panel
(FR-3711 .. FR-3716) are part 2.

## 2. Approach

1. `src/settings.rs`: `[workflow] spec_review`, `spec_review_timeout_secs`,
   `spec_review_max_rounds`; the workspace may not turn the gate off unless the
   config is trusted (restricted key).
2. `src/graph/spec_first.rs`: snapshot only the spec dir
   (`spec_snapshot_in`) and report the files a spec step changed
   (`changed_spec_paths`) so the record lists spec paths only.
3. `src/graph/executor.rs`: a pure hold in `GraphRunSession`. Steps with a spec
   ancestor are not started while the hold is armed; `AwaitingSpecReview` run
   phase; `release_spec_review`, `reopen_spec_steps` (re-run with a DATA block)
   and `skip_unstarted_steps`.
4. `src/graph/spec_review.rs`: record store (`<state>/spec-reviews/<id>.json`,
   0600 atomic writes under a flock), digest, `decide()` rules, and
   `SpecReviewGate`, a controller that requests the review, polls the record at
   most twice per second and reacts to the decision. No agent turn, no busy loop.
5. `src/app.rs`: arm the hold in `start_graph_run_record` for headless runs, add
   spec paths after the spec gate passes, call the gate after a step finishes,
   poll in the headless loop, end the run on reject / timeout / cancel, and put
   `spec_review` in `HeadlessOutcome`.
6. `src/cli.rs`, `src/main.rs`, `src/mcp_server.rs`: `--no-spec-review`,
   MCP `no_spec_review`, and `dmuxai spec-review list|show|approve|request-changes|reject`
   (human-only via `board::resolve_actor`).

### Part 2 (FR-3708 dashboard part, FR-3711 .. FR-3716, #374)

7. `src/graph/spec_first.rs`, `src/board/plan.rs`, `src/board/store.rs` (#374):
   `expand_plan_steps` applies the same `apply_spec_first` as the run
   (FR-3503) to a pending plan. `board plan --edit`, dashboard Save plan and
   `board add --plan-hash` / attach store the expanded plan under its own hash
   and record the inserted step ids as `plan.spec_first_added`. A plan that
   does not validate after expansion is stored unexpanded (the run expands it
   as before).
8. `src/graph/types.rs`, `src/app.rs`: the executed graph records
   `plan_hash`, computed from the steps that actually run, so the approved hash
   can be compared with the executed one.
9. `src/board/runner.rs`, `src/graph/spec_review.rs`: the board runner passes
   `DMUXAI_BOARD_ITEM_ID`; the gate mirrors each record change to the item
   (`set_spec_review`, thread message, `spec_review` notify event) and never
   touches lane or decision; the child timeout counts only time while the item's
   review is not pending.
10. `src/web/board_api.rs`: `POST /api/board/{id}/spec-review` (human headers,
    digest, error codes) calls the same `decide()` and updates the item status.
11. `src/web/assets/board.js`, `board.css`: badges, My decisions entry, spec
    review section with the Markdown viewer link, plan review title, inserted
    step note.
12. `src/app.rs`: TUI spec review panel inside the approval panel area; the gate
    is armed for interactive runs too.

## 3. Dependency order

settings → spec_first helpers → executor hold → spec_review module (unit tests)
→ App wiring → CLI / MCP → integration tests → docs.

Part 2: plan expansion (#374) → executed `plan_hash` → board item mirror and
child timeout → dashboard endpoint → board UI → TUI panel → browser e2e → docs.

## 4. Risks

| Risk | Mitigation |
| --- | --- |
| A run killed with SIGKILL leaves a `pending` record | Records carry `created_ms`; `list --all` shows them; a later part may add a sweeper. Residual risk recorded in the review. |
| Existing harnesses running spec-first graphs would wait for a human | Those tests set `spec_review = Some(false)`; `DmuxSettings::test_default` already has spec-first off. |
| Polling cost | Poll interval 500 ms, covered by TC-3718. |
| Stored plan hash changes when spec-first expands it (#374) | `board add --plan-hash` returns the stored (expanded) hash; dashboards read the hash from the item. Unexpanded legacy plans still run and record their executed hash honestly. |
| TUI runs now pause for a human | Only with spec-first `full` and `spec_review` on; Esc hides the panel, `--no-spec-review` / config `off` disable it. |
| Feedback injection into the spec prompt | Feedback is masked, capped at 4 KB and wrapped in a nonce DATA block that the text cannot close. |

## 5. Rollback

Set `[workflow] spec_review = "off"` or pass `--no-spec-review`. The record
directory is additive state; deleting `<state>/spec-reviews/` loses only
pending reviews (a waiting run then ends as `spec review cancelled`).
No database or provider protocol change. Part 2 adds only optional JSON fields
(`spec_review`, `plan.spec_first_added`, graph `plan_hash`) that older builds
ignore; reverting the code leaves stored items readable.
