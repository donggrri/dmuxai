# PLAN-state-per-repo

Spec: [SPEC-state-per-repo.md](../specs/SPEC-state-per-repo.md)

## Modules and changes (in dependency order)

1. `src/state_path.rs` (new) — pure path derivation: `state_base()`
   (`DMUXAI_STATE_DIR` > `XDG_STATE_HOME` > `HOME`, Windows `%LOCALAPPDATA%\DmuxAI`),
   FNV-1a 64 helper, `repo_key()` (sanitized basename + 16-hex FNV-1a of the
   canonical workspace root via `workspace::normalize_workspace_path`),
   `repo_db_path()`, `legacy_db_path()`, and `prepare_repo_db()` which copies
   the legacy DB once and returns an optional one-line notice (FR-450..FR-453).
   Unit tests live here (TC-450, TC-451, TC-452, TC-453).
2. `src/lib.rs` — `pub mod state_path;`
3. `src/main.rs` — replace `conversation_db_path()` with `state_path::prepare_repo_db(&workspace_root)`;
   push the migration notice as a startup notice.
4. `src/conversation.rs` — `UnsupportedSchema` carries the DB path; `Display`
   names the path, says it was written by a newer dmuxai, and suggests
   `DMUXAI_STATE_DIR` (FR-454, TC-454).

## Risks

- Env-var-dependent tests can race; non-env-mutating helpers are tested
  directly and the one env-mutating test is guarded by a mutex.
- Rollback: revert the commits; the legacy DB is never moved, so old builds
  keep working (per-repo DBs are simply ignored by them).

## Rollback

Revert; no persistent schema change is introduced, only file locations.
