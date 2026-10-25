# PLAN: Busy conversation database on open

Status: complete  
Issue: [#167](https://github.com/dmuxai/dmuxai/issues/167)  
Spec: [SPEC-agent-api.md](../specs/SPEC-agent-api.md) (FR-2120)

1. Inspect the runtime-session schema read-only before writing to it: report
   whether `runtime_sessions`, its workspace/status index, the
   `turns.owner_session_id` column (only when `turns` exists) and the
   `turns_by_owner_status` index are already present, and skip the immediate
   write transaction when everything is. Keep the `IF NOT EXISTS` clauses and
   the column re-check inside the transaction so a racing process stays
   harmless. (FR-2120, TC-2120)
2. Skip `PRAGMA journal_mode = WAL` when `PRAGMA journal_mode` already reports
   `wal`: switching the journal mode needs a brief exclusive lock that SQLite
   does not run the busy handler for. (FR-2120, TC-2120)
3. Add `ConversationError::Busy(String)` and an `is_busy` helper for
   SQLITE_BUSY/SQLITE_LOCKED, and wrap the open phase (pragmas through the
   `runtime_sessions` insert) in a bounded retry: about 20 s in total, 50 ms
   doubling to a 1 s maximum, with a small jitter from the existing `uuid`
   dependency. Non-busy errors are returned unchanged. (FR-2120, TC-2120)
4. Map a busy open to the headless conflict exit code (4) with the actionable
   "retry when it finishes" message; every other open failure keeps the
   internal exit code (6). (FR-2120, TC-2120)
5. Cover the behavior with deterministic tests (no provider, no second real
   process): a held write lock, a lock released inside the budget, the
   `is_busy` mapping and the CLI exit-code mapping. (FR-2120, TC-2120)

## Risks and mitigations

- The retry budget must not turn a short conflict into a long hang: the budget
  is bounded (about 20 s), the backoff is capped at 1 s and the error stays a
  conflict (4) with a retry message.
- A database that is genuinely busy must still be usable afterwards: the
  failing attempt rolls back its transaction and the connection is dropped
  before the next attempt.
- Skipping the write transaction must not skip a needed migration: the
  read-only check covers every object the transaction used to create, and the
  v2/v3 migrations keep their own transactions.
- Concurrent starts must stay safe: no exclusive or advisory file lock is
  added; SQLite's own locking remains the only mechanism.

## Rollback

The change touches no schema and no persistent format: `user_version` stays at
3 and the runtime-session objects are unchanged. Reverting the code restores the
former behavior (an immediate "database is locked" internal error). Databases
opened by the new code are readable by the old code.
