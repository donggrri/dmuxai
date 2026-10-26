# REVIEW: A reply that cannot be saved must not disappear

Spec: [SPEC-agent-api.md](../specs/SPEC-agent-api.md) (FR-2140)
Plan: [PLAN-reply-save-failure.md](../plans/PLAN-reply-save-failure.md)
Tasks: [TASKS-reply-save-failure.md](../tasks/TASKS-reply-save-failure.md)
Issue: [#203](https://github.com/dmuxai/dmuxai/issues/203)

## What changed

| Area | Change | Location |
| --- | --- | --- |
| Conversation engine | The busy-retry loop of the open phase is now the shared `with_busy_retry(budget, attempt)` helper | `src/conversation.rs` `with_busy_retry`, `open_with_retry` |
| Conversation engine | New `record_turn_outputs_with_retry(outputs, budget)`: one attempt uses a 250 ms busy timeout, a busy error is retried until the budget is spent, a spent budget is reported as `ConversationError::Busy`, and a zero budget probes the database without waiting so a released lock still gets the reply | `src/conversation.rs` `record_turn_outputs_with_retry` |
| App | Every turn-output save goes through the retrying call; the budget (about 20 s of waiting per turn) is charged only for the waiting that a failed save really spent, so a save after a successful one gets the whole budget again and a long turn is not punished by its own runtime | `src/app.rs` `record_turn_output_with_retry`, `conversation_save_budget_left` |
| App | A failed save still applies the event to the visible transcript, so the streamed reply stays in `messages` and in the headless `text` | `src/app.rs` `apply_agent_output` |
| App | A failed save is recorded and mapped in the headless outcome: `ok: false`, exit 4 (busy) or 6 (other), error "Could not save &lt;agent&gt; reply: …" | `src/app.rs` `conversation_save_exit_code`, `run_headless` |
| App | The "Could not save &lt;agent&gt; output" notice is shown once per turn (a locked database would repeat it for every event) | `src/app.rs` `apply_agent_output` |
| App | The cancel record is saved with the same retry | `src/app.rs` `cancel_selected` |
| Conversation engine | `close_session` marks the session closed after the first attempt whether it succeeded or not, so `Drop` never retries a busy close; new `close_session_with_timeout` lets shutdown try once with a short busy timeout | `src/conversation.rs` `close_session`, `close_session_with_timeout` |
| App | Once the save budget of the turn is spent, `shutdown` closes the session with a 250 ms busy timeout, so a headless run does not wait the default 5 s connection timeout twice | `src/app.rs` `conversation_save_budget_spent`, `SHORT_CLOSE_BUSY_TIMEOUT`, `shutdown` |
| App / API | A failed reply save and a busy open report `error_kind` `conflict` (exit 4) or `internal` (exit 6); `ApiError::to_json` and the headless outcome use `agent_api::error_kind_for_exit` | `src/agent_api.rs` `error_kind_for_exit`, `ApiError::to_json`; `src/app.rs` `run_headless` |
| Docs | FR-2140 and TC-2140..TC-2144, the exit-code table, `error_kind`, the changelog | `docs/specs/SPEC-agent-api.md`, `docs/API.md`, `CHANGELOG.md` |

## Spec compliance

| Requirement | Met by | Evidence |
| --- | --- | --- |
| FR-2140 (bounded busy backoff for the reply save) | `with_busy_retry` + `record_turn_outputs_with_retry`; about 20 s of waiting per turn, charged only to the saves that really waited for a locked database | `src/conversation.rs` `tc_2140_busy_save_is_retried_and_persisted_once_the_lock_is_released`, `tc_2140_a_locked_database_fails_the_save_with_a_busy_error`, `tc_2140_a_spent_budget_still_saves_once_the_lock_is_released`; `src/app.rs` `tc_2140_short_database_lock_is_retried_and_the_reply_is_persisted`, `tc_2140_a_spent_budget_stores_the_rest_of_the_reply_after_the_lock_is_released` (TC-2140) |
| FR-2140 (a still-failing save fails the headless run) | `run_headless` maps the recorded failure to `ok: false` with exit 4 (busy) or 6 (other database error) | `src/app.rs` `tc_2141_locked_database_fails_the_run_with_exit_4_and_keeps_the_text` (TC-2141), `tc_2143_non_busy_database_error_maps_to_the_internal_exit_code` (TC-2143), `tc_2144_busy_database_error_maps_to_the_conflict_exit_code` (TC-2144) |
| FR-2140 (the answer is never lost) | the failed save applies the event to the visible transcript; the headless `text` and the `messages` keep the reply | `src/app.rs` `tc_2141_…`, `tc_2142_failed_save_keeps_the_system_note_and_the_reply` (TC-2142) |
| FR-2140 (the TUI keeps the notice and does not crash) | the notice is unchanged, shown once per turn; no new panic path | `src/app.rs` `tc_2142_…` |
| FR-2140 (the retry never blocks longer than the budget) | one attempt waits at most 250 ms, the budget bounds the waiting of a turn (about 20 s, the order of the open phase), a spent budget only probes the database without waiting, and a save that follows a successful one gets the whole budget again | `src/conversation.rs` `SAVE_ATTEMPT_BUSY_TIMEOUT`, `SAVE_RETRY_BUDGET`; `src/app.rs` `conversation_save_budget_left` (TC-2140) |
| FR-2140 (exit 4/6 carry an error kind) | `error_kind_for_exit` maps exit 4 to `conflict` and exit 6 to `internal`; the headless outcome and `ApiError::to_json` include it, including the locked open of #196 | `src/app.rs` `tc_2141_…` (TC-2141), `tc_2143_non_busy_database_error_maps_to_the_internal_exit_code` (TC-2143), `tc_2144_busy_database_error_maps_to_the_conflict_exit_code` (TC-2144); `src/cli.rs` `tc_2120_busy_conversation_open_maps_to_exit_4` |
| FR-2140 (shutdown does not wait the default timeout twice) | the session is marked closed after the first attempt and shutdown closes once with a 250 ms busy timeout once the save budget is spent | `src/conversation.rs` `tc_2140_a_busy_close_is_attempted_once_and_does_not_wait_twice`; `src/app.rs` `tc_2140_shutdown_after_a_spent_budget_closes_once` (TC-2140) |

Non-goals kept: the activity-save path (`record_activity`) keeps its own
once-per-turn notice and is not retried; it stores auxiliary rows, not the
reply. No schema change, no new dependency, no change to the open phase or to
the exit codes of unrelated failures.

## Regression risk

| Risk | Mitigation |
|---|---|
| A locked database blocks the event loop | The waiting of a turn is bounded (about 20 s, the order of the open phase); after it is spent the database is only probed, without waiting |
| A save that used to fail fast now takes up to 20 s | Only busy databases are retried; every other error is returned unchanged |
| The turn outcome changes | The internal turn outcome is unchanged (the agent did answer); only the headless outcome reports the failure, so the graph and the dashboard keep their previous behaviour |
| The reply is applied twice | The failed-save branch returns before the success path, so an event is applied at most once |
| The transcript floods with notices | The notice is shown once per turn (`turn_save_failed_turns`) |
| A busy close is retried and delays the exit | The session is marked closed after the first attempt, so `Drop` never retries; shutdown uses a 250 ms busy timeout once the save budget is spent |
| Adding `error_kind` changes unrelated failure JSON | `error_kind_for_exit` returns `null` for every exit code other than 4 and 6, so usage/not-found/failed output keeps its shape plus a `null` field |

## Test evidence

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | pass |
| `cargo test --all-targets --no-fail-fast` | pass, except the pre-existing `web::server::tests::agent_prompt_api_get_lists_runners_unset_by_default` failure (fails on the unmodified checkout too) |
| `cargo test --lib tc_2140` | 4 passed |
| `cargo test --bin dmuxai tc_214` | 7 passed |
| `cargo test --lib conversation::` | 9 passed |
| `cargo test --bin dmuxai tc_2120` | 1 passed |
| `python3 scripts/check-spec.py --all` | 80 specs, 80 pass |
| `python3 scripts/check-tc-links.py` | 880 defined, 228 strong, 0 orphan, 0 undefined-test, 0 stale |

Manual end-to-end check with the built binary (`target/debug/dmuxai run "Say
hello" --agent slow --json`), a custom agent that prints `delayed-text` after
3 s, and a second SQLite connection holding `BEGIN EXCLUSIVE` during the turn:

- Lock held for the whole budget (40 s): exit code 4, `ok: false`,
  `error` = "Could not save slow reply: another dmuxai process is using this
  workspace's conversation database (database is locked); retry when it
  finishes", `text` = "delayed-text", one "Could not save slow output" system
  note next to the agent reply, and no assistant row in the conversation
  database.
- Lock released inside the budget (2 s): exit code 0, `ok: true`, `text` =
  "delayed-text", and the reply is stored in the conversation database.

## Residual risks and intentionally untested paths

- A reply of a *background* conversation (a second conversation of the same
  TUI process) is still lost when its save fails: the text of a background
  conversation lives in the database, not in the in-memory transcript. The
  notice is shown; the headless run (always foreground) is not affected.
- The `record_activity` path is not retried; an activity row can still be lost
  while the database is locked. The reply is not affected.
- The turn state of the TUI stays "completed" when only the save failed; the
  dashboard shows the notice instead of an error state. Marking the turn failed
  would change the graph and dashboard behaviour, which is outside this fix.
- No test runs a second real `dmuxai` process; the lock is taken by a second
  SQLite connection, which is the same lock a second process takes.
- The reply text of an event whose save failed is not stored later: the save
  of that event is lost for good, only the events that follow are stored once
  the database is writable again. Buffering the failed events and writing them
  later is a bigger change and is out of scope for this fix.
- The busy-retry budget of a turn is spent by the waiting of a failed save
  only. A save that finally succeeds after a long wait does not charge the
  budget, so a lock that is released just before every timeout can still block
  a save for the whole budget each time (the same behaviour as the open
  phase).
