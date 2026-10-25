# PLAN: A reply that cannot be saved must not disappear

Status: complete
Issue: [#203](https://github.com/dmuxai/dmuxai/issues/203)
Spec: [SPEC-agent-api.md](../specs/SPEC-agent-api.md) (FR-2140)

1. Extract the busy-retry loop of the open phase into a shared
   `with_busy_retry(budget, attempt)` helper in `src/conversation.rs` so the
   open phase and the save phase use the same backoff (50 ms doubling to 1 s,
   with jitter) and the same `ConversationError::Busy` result. The open phase
   keeps its behaviour. (FR-2140, TC-2140)
2. Add `ConversationEngine::record_turn_outputs_with_retry(outputs, budget)`:
   one attempt uses a short busy timeout (250 ms) so the retry loop owns the
   waiting, a busy error is retried until the budget is spent, and a spent
   budget is reported as `ConversationError::Busy`. A spent budget (`ZERO`)
   still makes one attempt, and that attempt does not wait at all: the
   database may have become writable again, and the save has to recover
   instead of skipping the database for the rest of the turn. (FR-2140,
   TC-2140)
3. In `src/app.rs`, save every turn output through the retrying call and keep
   the busy-retry budget per agent and turn: the time a save really waited for
   a locked database counts against the budget (about 20 s), a save that
   follows a successful one gets the whole budget again, and a turn that
   really waited only gets what is left of it (a probe that does not wait).
   A locked database can therefore not block the save path once per event, a
   long turn is not punished by the time it already ran, and the reply is
   still stored when the lock is released after the budget. (FR-2140,
   TC-2141)
4. When a save still fails, keep the answer: the event is applied to the
   in-memory transcript of the visible conversation, so the streamed reply
   stays in `messages` and in the headless `text`. The "Could not save <agent>
   output" system note stays as it is. (FR-2140, TC-2142)
5. Record the failed save on the app and map it in `App::run_headless`: the run
   ends with `ok: false`, exit code 4 for a busy database and exit code 6 for
   every other database error, and the reported error says the agent reply
   could not be saved. The failure is recorded once per turn, so a locked
   database cannot grow the report. The interactive TUI only shows the notice.
   (FR-2140, TC-2141, TC-2143, TC-2144)
6. Cover the behaviour with deterministic tests (no provider, no second real
   process): a held write lock, a lock released inside the budget, the exit
   code mapping, and the headless run with a locked database. (FR-2140,
   TC-2140..TC-2144)
7. Document the behaviour: FR-2140 and TC-2140..TC-2144 in
   `docs/specs/SPEC-agent-api.md`, the exit-code table in `docs/API.md`, and a
   `Fixed` entry in `CHANGELOG.md`. (FR-2140)

## Risks and mitigations

- The retry must not turn a short conflict into a long hang: the budget is
  bounded (about 20 s of waiting per turn), the backoff is capped at 1 s, and
  the error stays a conflict (4) with a message that asks the caller to retry.
- A turn with many streamed events must not block once per event: the waiting
  of a failed save counts against the budget of the turn, and a spent budget
  only probes the database without waiting.
- The old behaviour (an immediate "database is locked" error) must not come
  back for the paths that do not save a turn output: only the turn-output save
  and the cancel record use the retrying call.
- The reply must not be lost twice: the failed save still applies the event to
  the transcript, so the text is returned even though the database refused it.
- Concurrent starts must stay safe: no exclusive or advisory file lock is
  added; SQLite's own locking remains the only mechanism, and the tests use a
  second SQLite connection that takes the same lock a second process would.

## Rollback

The change touches no schema and no persistent format: `user_version` stays at
3 and the message and turn tables are unchanged. Reverting the code restores
the former behaviour (an immediate "database is locked" internal error and a
lost reply). Databases written by the new code are readable by the old code.
