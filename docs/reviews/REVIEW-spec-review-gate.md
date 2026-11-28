# REVIEW: spec-review-gate

Status: part 1 and part 2 verified
Owner: DmuxAI
Created: 2026-11-28
Spec: [SPEC-spec-review-gate](../specs/SPEC-spec-review-gate.md)
Plan: [PLAN-spec-review-gate](../plans/PLAN-spec-review-gate.md)
Tasks: [TASKS-spec-review-gate](../tasks/TASKS-spec-review-gate.md)

## 1. Summary

A headless run with spec-first `full` now pauses after the spec step passes the
FR-3508 gate. It writes `<state>/spec-reviews/<graph-id>.json` (0600, atomic,
flock), shows `spec review: waiting for a human · dmuxai spec-review approve <id>`
once on the progress stream, and polls the record at most twice per second. No
step with a spec ancestor starts until `approved`; independent read-only steps
keep running. Request changes re-runs the spec step with a nonce DATA block;
reject, timeout and cancel end the run with exit 1 (130 for Ctrl-C) and the
documented messages. The board child uses the same headless path.

## 2. Requirement review (part 1 scope)

| Requirement | Result | Evidence |
| --- | --- | --- |
| FR-3700 | Met | `tc_3700_*` in `src/settings.rs` (defaults follow `spec_first`, parse/override, invalid values warn, untrusted workspace cannot turn the gate off). |
| FR-3701 | Met | `tc_3701_*` in `src/cli.rs`, `src/mcp_server.rs`; `tc_3706_timeout_expires_and_tc_3701_no_spec_review_skips` runs the binary with `--no-spec-review`. `--approve-plan` still pauses (every integration run passes it). |
| FR-3702 | Met | `tc_3702_*` in `src/graph/spec_review.rs`; `tc_3717_run_pauses_for_review_then_approves` shows exactly one spec call while pending. Failed spec step creates no review (`tc_3702_failed_spec_step_creates_no_review`). |
| FR-3703 | Met | `tc_3703_*`: fields, 0600, digest, masking, spec paths only under `spec_dir`, id validation. |
| FR-3704 | Met | `tc_3704_*` unit tests and integration `tc_3704_rejected_review_fails_run_without_implementation`, `tc_3704_interrupt_while_waiting_cancels_record` (exit 130, record `cancelled`). |
| FR-3705 | Met | `tc_3705_*` unit and `tc_3705_request_changes_reruns_spec_step` (round 2, new digest, feedback in the re-run spec). Empty feedback refused, max rounds refused, nonce block cannot be closed by feedback. |
| FR-3706 | Met | `tc_3706_timeout_expires_review_and_stops_run`; binary test with `DMUXAI_SPEC_REVIEW_TIMEOUT_MS`. |
| FR-3707 | Met | `tc_3707_concurrent_decisions_digest_and_not_pending`. |
| FR-3708 | Met (CLI, MCP; dashboard in part 2) | `tc_3708_spec_review_decide_needs_a_tty` (cli), `tc_3708_mcp_has_no_spec_review_tool`, `tc_3709_decide_without_tty_is_forbidden`, `tc_3709_decide_on_tty_conflict_and_run_id` (pseudo-TTY, `DMUXAI_RUN_ID` → 5). Actor is always `owner`. |
| FR-3709 | Met | `tests/spec_review_cli.rs` `tc_3709_*`: list/show JSON, exit codes 2/3/4/5, invalid id creates no store directory. |
| FR-3710 | Met | `tc_3710_journal_events_and_graph_file_field`: events carry no feedback text; graph file has `spec_review`. In the CLI the events go to the run journal progress copy only. |
| FR-3717 | Met | `tc_3717_run_pauses_for_review_then_approves`: JSON `spec_review` approved round 1, hint once on stderr. |
| NFR-3700 | Met | `tc_3718_waiting_polls_at_most_twice_per_second_and_uses_no_cpu`; no extra agent turn (call counts in integration tests). |
| NFR-3701 | Met | Feedback and prompt summary pass `mask_sensitive_tokens`; record 0600, dir 0700; journal lines omit feedback. |
| FR-3711 .. FR-3716 | See part 2 | The part 1 placeholders in `tests/spec_review_part2.rs` were replaced by the real tests and the file deleted. |

## 2a. Requirement review (part 2 scope)

| Requirement | Result | Evidence |
| --- | --- | --- |
| FR-3708 (dashboard) | Met | `tc_3708_spec_review_post_requires_human_headers` (`src/web/board_api.rs`): POST without `Sec-Fetch-*` / `X-Dmuxai-Human` is 403 and the record stays pending. Actor is `owner`. |
| FR-3711 | Met | `tc_3711_board_item_mirrors_spec_review` (`src/board/runner.rs`): `DMUXAI_BOARD_ITEM_ID` reaches the child, request/approve/request-changes/reject update `spec_review`, thread messages from `planner` / `owner`, `spec_review` notify event, lane and decision unchanged, invalid or unknown item ids ignored with a warning. `tc_3714_*` e2e checks the same messages after a real run. |
| FR-3712 | Met | `tc_3712_child_timeout_suspended_while_spec_review_pending`: a 4 s child with a 2 s timeout survives while pending; after the review leaves `pending` the timeout resumes and the child is killed (exit 124). |
| FR-3713 | Met | `tc_3713_spec_review_endpoint_codes_and_payloads`: each verdict, 400 bad id / verdict / empty feedback, 404 unknown item / no review, 409 not pending / stale digest, `spec_review` in GET item and in GET pending (reason `spec_review`). |
| FR-3714 | Met | `tc_3714_spec_review_in_board_ui_with_real_child` (browser, real `dmuxai --web`, real board child, fake cursor-agent in a temp git repo): `Spec review` badge, My decisions entry, `Spec review (round N)` title, spec link opens the safe viewer (no script, modal stays open), empty feedback notice, round 2 after Request changes, digest conflict notice "reload", verdict shown and buttons gone, item Done; second item Reject spec ends `unknown` and `impl-reject.txt` is never written. Screenshots mask the token and go to the test temp dir unless `DMUXAI_E2E_SHOTS` names a directory. |
| FR-3715 | Met | `tc_3715_board_plan_and_spec_review_assets` (assets), the `Plan review` badge/title checks in `tc_3714_*`, and `tc_3715_plan_review_runs_spec_first_expanded_plan` (browser). |
| FR-3715 + FR-3503 (#374) | Met | `tc_3715_plan_edit_and_attach_store_spec_first_expanded_plan` (`src/board/plan.rs`: edit, idempotent re-save, `board add --plan-hash`, spec_first off), `tc_3715_save_plan_stores_spec_first_expanded_plan` (dashboard Save plan), `tc_3715_approved_expanded_plan_hash_equals_executed_plan_hash` (`src/app.rs`: 1-step plan, spec_first full → spec-1, step-1, test-1 run and `run.plan_hash` equals the approved hash), and the browser test: 3 steps with two `added by spec-first` notes shown before approval, executed graph `plan_hash` equals the approved hash. Spec section 7 records the amendment. |
| FR-3716 | Met | `tc_3716_tui_spec_review_panel_keys` (`src/app.rs`): panel lists spec paths; `a` approves, `c` + feedback + Enter requests changes, `x` rejects, Esc hides without deciding; keys act only with an empty input line. |

## 3. Non-goals and boundaries checked

- No MCP or agent-API decide tool; no `--allow-sensitive` or `--trust-workspace-config` added anywhere; `.github/` untouched.
- `--approve-plan` / `--approve-plan-id` never decide a review.
- No database or provider protocol change. Process launch changes only in the
  board child wait loop (pause-aware timeout) and one added env var.
- The dashboard POST updates only the item's review status; the thread messages
  come from the child, so a decision is never recorded twice.

## 4. Test evidence

Part 2, run in this worktree after the final edit.

| Command | Result |
| --- | --- |
| `cargo fmt --check` | pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | pass |
| `cargo test --all-targets --no-fail-fast` | all binaries pass except `term_cli` `tc_2622_wait_match_prompt_across_writes_with_ansi_and_cr`, which timed out under full parallel load in both full runs and passes when `cargo test --test term_cli` runs alone (35 passed); the term code is not touched by this change |
| `cargo test --test web_board_e2e -- --ignored` | 13 passed (twice; tc_3600 .. tc_3610, tc_3714, tc_3715) |
| `bash scripts/check-spec.sh docs/specs/SPEC-spec-review-gate.md` | pass |
| `python3 scripts/check-spec.py --all` | 117 specs, 117 pass |
| `python3 scripts/check-tc-links.py` | 0 orphan, 0 undefined-test, 0 stale |

`tc_3605_auto_run_opt_out_and_stale_plan_hash` failed once in an intermediate
full browser run and passed in two later full runs and five runs alone.

Part 1 evidence:

| Command | Result |
| --- | --- |
| `cargo fmt --check` | pass |
| `cargo clippy --all-targets -- -D warnings` | pass |
| `cargo test --all-targets --no-fail-fast` | pass (49 test binaries ok, 0 failed) |
| `cargo test --test spec_first_headless` / `--test spec_review_cli` | 5 / 4 passed |
| `cargo test --lib graph::spec_review` | 16 passed |

## 5. Residual risks and untested paths

- Interactive TUI runs are now gated when spec-first is `full` and the review
  is on. Gate warnings (store or board mirror failures) use `eprintln!`, which
  can smear the TUI screen; they are rare error paths.
- #374: a plan that no longer validates after expansion (for example over the
  node cap) is stored unexpanded; the run expands it as before, so its executed
  `plan_hash` differs from the approved one. Plans stored before this change
  behave the same way. `board add --plan-hash` and attach may store a hash
  different from the one passed in; callers must read `plan.hash` from the item.
- Board mirror failures (item deleted, store unwritable) only warn; the run and
  the review record continue.
- A run killed with SIGKILL leaves a `pending` record; `spec-review list --all` shows it and a later decision is accepted but nothing waits. No sweeper yet.
- A step failure that triggers a replan while a review is pending keeps the gate; replan plus gate interplay is not covered by a test.
- A store that cannot be opened or written fails the run closed (`spec review unavailable: ...`); only the write-failure path in the gate is unit tested, not the App message ordering for it.
- The browser tests use a fake cursor-agent; no live provider run was done.
