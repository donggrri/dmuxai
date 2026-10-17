# REVIEW-state-per-repo

Spec: [SPEC-state-per-repo.md](../specs/SPEC-state-per-repo.md) ·
Plan: [PLAN-state-per-repo.md](../plans/PLAN-state-per-repo.md) ·
Tasks: [TASKS-state-per-repo.md](../tasks/TASKS-state-per-repo.md)

## Spec compliance

| Requirement | Status | Evidence |
|---|---|---|
| FR-450 | Met | `repo_db_path` derives the DB from the canonical root (`workspace::normalize_workspace_path`) plus the repo key; `different_workspaces_get_different_db_paths` passes (TC-450). |
| FR-451 | Met | `state_base()` honors `DMUXAI_STATE_DIR` first; `state_dir_env_overrides_base` passes (TC-451). |
| FR-452 | Met | `prepare_repo_db` copies the legacy DB once, leaves it in place, and returns the notice that `main.rs` pushes as a startup notice; `legacy_db_is_copied_once_and_left_in_place` passes (TC-452). |
| FR-453 | Met | `fnv1a64` matches published FNV-1a 64 test vectors; `repo_key_is_stable_fnv1a` passes (TC-453). |
| FR-454 | Met | `UnsupportedSchema` carries the DB path and renders a message naming the path, a newer dmuxai, and `DMUXAI_STATE_DIR`; `unsupported_schema_message_names_path` passes (TC-454). |

Non-goals respected: the AGY model-cache path and launcher behavior are
untouched; the conversation schema itself is unchanged (no `user_version`
bump).

## Regression risk

- `UnsupportedSchema` changed shape from a tuple to a struct variant; both
  construction sites and the renderer were updated and `cargo check
  --all-targets` is clean, so no callers remain.
- Per-repo DBs are additive files; older binaries still read the untouched
  legacy DB, so rollback is safe.
- Tests used a shared `ENV_LOCK` mutex so env-var mutations cannot race.

## Test evidence

- `cargo fmt --check` — pass.
- `cargo test --all-targets` — 270 deterministic tests pass (81 lib, 143 app,
  and the remaining integration targets), including the five new TC tests.
- `cargo clippy --all-targets --all-features -- -D warnings` — pass.

## Rule exceptions

- `scripts/check-spec.ps1` could not run on this Linux host (no PowerShell
  or pwsh is installed). The underlying checks were replicated with
  equivalent regex logic against `docs/specs/SPEC-state-per-repo.md`: unique
  FR-450..FR-454 rows, unique TC-450..TC-454 rows, and every `Must`
  requirement mapped to an acceptance scenario — pass.

## Residual risks

- TC-454 unit-tests the rendered message rather than an end-to-end open of a
  schema-3 database; the `user_version > 2` branch in `ConversationEngine::open`
  is exercised only by construction review.
- The migration notice surfaces only in the TUI startup notices; headless
  consumers have no other rendering of it (spec asks only for "a one-line
  notice … shown").
