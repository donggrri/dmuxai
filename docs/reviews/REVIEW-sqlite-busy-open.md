# REVIEW: Busy conversation database on open

Status: complete  
Issue: [#167](https://github.com/dmuxai/dmuxai/issues/167)  
Spec: [SPEC-agent-api.md](../specs/SPEC-agent-api.md) (FR-2120)  
Plan: [PLAN-sqlite-busy-open.md](../plans/PLAN-sqlite-busy-open.md)  
Tasks: [TASKS-sqlite-busy-open.md](../tasks/TASKS-sqlite-busy-open.md)  
Reviewed: 2026-10-25

## 1. Result

A second `dmuxai run` in a busy workspace no longer fails with an internal
error. The open phase of the conversation database waits for the other process:
it retries SQLITE_BUSY/SQLITE_LOCKED with a bounded backoff (about 20 s, 50 ms
doubling to 1 s, with a small jitter), it no longer takes a write transaction
while the schema is current, and it no longer switches the journal mode of a
database that is already in WAL mode. A database that stays locked ends the
headless run with exit code 4 and a message that asks the caller to retry. The
same mapping covers the session registration and turn recovery that follow the
open, because they write to the same database.

No exclusive or advisory file lock was added; SQLite's own locking stays the
only mechanism.

## 2. Requirement review

| Requirement | Evidence | Result |
| --- | --- | --- |
| FR-2120 (retry with bounded backoff) | `open_with_retry` in `src/conversation.rs`; `tc_2120_open_succeeds_once_the_lock_is_released` | Met |
| FR-2120 (no write transaction while the schema is current) | `runtime_session_schema_is_current` + `ensure_runtime_session_schema`; `tc_2120_open_of_current_schema_takes_no_write_lock` | Met |
| FR-2120 (still-busy open ends as exit 4 with a retry message) | `conversation_open_error` in `src/cli.rs`, used for the engine open and for `App::new`; `tc_2120_busy_conversation_open_maps_to_exit_4` | Met |
| FR-2120 (no exclusive lock) | No lock file, no `flock`, no new dependency; only SQLite locking | Met |
| FR-1810 (exit 4 = busy/conflict) | The busy open now uses the documented conflict code | Met |

Non-goals were respected: the agent protocol, the provider adapters, the
conversation schema (`user_version` stays 3) and the runtime-session lifecycle
are unchanged, and the TUI startup path keeps its previous error handling.

## 3. Test evidence

| Command | Result |
| --- | --- |
| `cargo check --all-targets` | Pass |
| `cargo fmt --all` / `cargo fmt --check` | Pass (no diff) |
| `cargo test --lib tc_2120` | Pass: 4 tests (no write lock, released lock, `is_busy`, real busy error) |
| `cargo test --bin dmuxai tc_2120` | Pass: 1 test (exit 4 for busy, exit 6 for other errors) |
| `cargo test --lib conversation` | Pass: 7 tests |
| `cargo test --test conversation` | Pass: 27 tests |
| `cargo test --test agent_api_cli` | Pass: 15 tests |
| `cargo test --bin dmuxai cli::` | Pass: 9 tests |
| `python3 scripts/check-spec.py --all` | Pass: 80 specs, 80 pass, 0 fail |
| `python3 scripts/check-tc-links.py` | Pass: 863 defined, 0 orphan, 0 undefined-test, 0 stale |

Manual end-to-end check (temporary directory, `DMUXAI_STATE_DIR` pointed at it):

- a second connection holding `BEGIN IMMEDIATE` for 5 s: `dmuxai run` waited
  ~4.9 s and then continued normally;
- the same lock held for 30 s: `dmuxai run` exited 4 after ~19.8 s with
  `error: another dmuxai process is using this workspace's conversation
  database (database is locked); retry when it finishes`.

## 4. Regression risk

Low. The change only affects the open phase of the conversation database:

- `ensure_runtime_session_schema` still creates every runtime-session object
  when one is missing, still inside one transaction, and still re-checks the
  `turns.owner_session_id` column inside it;
- the v2/v3 migrations keep their own transactions and their `user_version`
  handling;
- the connection keeps the previous 5 s busy timeout once the database is open;
- the to-do store, the event log and the adapters are untouched.

## 5. Residual risk

- A database that stays locked for longer than the budget ends in exit 4 even
  when the holder would have finished later; the message tells the caller to
  retry.
- The retry uses wall-clock time, so a heavily loaded machine may spend a few
  attempts faster than the nominal backoff.
- The TUI startup path (`src/main.rs`) keeps its previous error handling: it
  now waits for the same budget and then reports the busy message, but exits
  with its generic error code instead of the headless conflict code 4.

## 6. Rollback

Revert `src/conversation.rs` and `src/cli.rs`: no schema, no persistent format
and no stored state changed, so no database migration or cleanup is needed.
