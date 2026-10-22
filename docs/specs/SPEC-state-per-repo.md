# SPEC-state-per-repo

Status: Implemented · Dogfood: #12

## Decision summary
The conversation DB was one global file, `$XDG_STATE_HOME/dmuxai/conversations.sqlite3`, shared by every worktree and binary version. A newer schema in one worktree broke older builds in other worktrees with `UnsupportedSchema(2)`. The DB is now keyed by the workspace root, there is an explicit override, and old data is migrated by copy.

## Terms / state
- State base: `$DMUXAI_STATE_DIR` if set. Otherwise `$XDG_STATE_HOME/dmuxai` or `~/.local/state/dmuxai`.
- Repo key: `<sanitized basename>-<16 hex FNV-1a 64 of the canonical workspace root path>`.
- Per-repo DB: `<base>/repos/<repo key>/conversations.sqlite3`.
- Legacy DB: `<base>/conversations.sqlite3`.

## Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-450 | Must | The DB path is derived from the canonical workspace root, so two worktrees get different DB files. |
| FR-451 | Must | `DMUXAI_STATE_DIR` overrides the state base. |
| FR-452 | Must | Migration: if the per-repo DB does not exist and the legacy DB does, the legacy DB is copied (not moved) to the per-repo path on first open, and a one-line notice is shown. The legacy file is left untouched. |
| FR-453 | Must | The repo key is stable across runs and builds (FNV-1a, not `DefaultHasher`). |
| FR-454 | Should | `UnsupportedSchema(n)` renders as a message that names the DB path, says it was written by a newer dmuxai, and suggests `DMUXAI_STATE_DIR`. |

## Acceptance
| TC | Covers | Test |
|---|---|---|
| TC-450 | FR-450 | `different_workspaces_get_different_db_paths` |
| TC-451 | FR-451 | `state_dir_env_overrides_base` |
| TC-452 | FR-452 | `legacy_db_is_copied_once_and_left_in_place` |
| TC-453 | FR-453 | `repo_key_is_stable_fnv1a` (fixed expected value) |
| TC-454 | FR-454 | `unsupported_schema_message_names_path` |

## Exit criteria
TCs pass in CI. Dogfood #12 is closed.
