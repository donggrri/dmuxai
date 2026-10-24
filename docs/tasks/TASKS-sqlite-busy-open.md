# TASKS: Busy conversation database on open

Spec: [SPEC-agent-api.md](../specs/SPEC-agent-api.md) (FR-2120)  
Plan: [PLAN-sqlite-busy-open.md](../plans/PLAN-sqlite-busy-open.md)

## Part A (Code implementation)

| Done | Task | Requirement IDs | Test command |
| --- | --- | --- | --- |
| [x] | `ensure_runtime_session_schema` checks the runtime-session objects read-only and only takes the immediate transaction when something is missing | FR-2120 | `cargo test --lib tc_2120_open_of_current_schema_takes_no_write_lock` |
| [x] | The open skips `PRAGMA journal_mode = WAL` when the database already reports `wal` | FR-2120 | `cargo test --lib tc_2120_open_of_current_schema_takes_no_write_lock` |
| [x] | `ConversationError::Busy` plus an `is_busy` helper for SQLITE_BUSY/SQLITE_LOCKED | FR-2120 | `cargo test --lib tc_2120_is_busy_matches_busy_and_locked_codes`, `cargo test --lib tc_2120_a_locked_database_produces_a_busy_sqlite_error` |
| [x] | The open phase retries a busy database with a bounded backoff through `open_with_retry(db, ws, budget)` | FR-2120 | `cargo test --lib tc_2120_open_succeeds_once_the_lock_is_released` |
| [x] | `src/cli.rs` maps a busy open to exit 4 (conflict) with a retry message, other failures stay exit 6 | FR-2120 | `cargo test --bin dmuxai tc_2120_busy_conversation_open_maps_to_exit_4` |

## Part B (Documentation and gates)

| Done | Task | Requirement IDs | Test command |
| --- | --- | --- | --- |
| [x] | `docs/specs/SPEC-agent-api.md` documents FR-2120 and TC-2120 | FR-2120 | `python3 scripts/check-spec.py docs/specs/SPEC-agent-api.md` |
| [x] | Plan, tasks and review artifacts for this change | FR-2120 | manual review |
| [x] | Spec and TC link gates stay green | FR-2120 | `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` |
