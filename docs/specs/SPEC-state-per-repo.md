# SPEC-state-per-repo

Status: Implemented · Dogfood: #12

## 1. Decision summary
The conversation DB was one global file, `$XDG_STATE_HOME/dmuxai/conversations.sqlite3`, shared by every worktree and binary version. A newer schema in one worktree broke older builds in other worktrees with `UnsupportedSchema(2)`. The DB is now keyed by the workspace root, there is an explicit override, and old data is migrated by copy.

## 3. Terms and state model
- State base: `$DMUXAI_STATE_DIR` if set. Otherwise `$XDG_STATE_HOME/dmuxai` or `~/.local/state/dmuxai`.
- Repo key: `<sanitized basename>-<16 hex FNV-1a 64 of the canonical workspace root path>`.
- Per-repo DB: `<base>/repos/<repo key>/conversations.sqlite3`.
- Legacy DB: `<base>/conversations.sqlite3`.

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-450 | Must | The DB path is derived from the canonical workspace root, so two worktrees get different DB files. |
| FR-451 | Must | `DMUXAI_STATE_DIR` overrides the state base. |
| FR-452 | Must | Migration: if the per-repo DB does not exist and the legacy DB does, the legacy DB is copied (not moved) to the per-repo path on first open, and a one-line notice is shown. The legacy file is left untouched. |
| FR-453 | Must | The repo key is stable across runs and builds (FNV-1a, not `DefaultHasher`). |
| FR-454 | Should | `UnsupportedSchema(n)` renders as a message that names the DB path, says it was written by a newer dmuxai, and suggests `DMUXAI_STATE_DIR`. |
| FR-1837 | Must | State base resolution: `DMUXAI_STATE_DIR` overrides; otherwise `XDG_STATE_HOME/dmuxai`; otherwise `~/.local/state/dmuxai`; an empty `DMUXAI_STATE_DIR` falls through. Missing HOME is an error. |
| FR-1838 | Must | Per-repo DB naming: `<base>/repos/<sanitized basename>-<16 hex FNV-1a 64 of canonical workspace>/conversations.sqlite3`, stable across runs and builds. |
| FR-1839 | Must | Legacy migration: if the per-repo DB is missing and the legacy DB exists, copy (not move) it once with a one-line notice; the legacy file is left untouched and a second open does not re-copy. |
| FR-1840 | Must | Two processes on the same workspace use the same per-repo DB path; appends take a cross-process lock so seqs stay unique and increasing (stale lock cleared after 5 s). |

### Code-vs-spec mismatches

| ID | Spec says | Code does (file:function) | Proposed resolution |
| --- | --- | --- | --- |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-450 | FR-450 | `different_workspaces_get_different_db_paths` |
| TC-451 | FR-451 | `state_dir_env_overrides_base` |
| TC-452 | FR-452 | `legacy_db_is_copied_once_and_left_in_place` |
| TC-453 | FR-453 | `repo_key_is_stable_fnv1a` (fixed expected value) |
| TC-454 | FR-454 | `unsupported_schema_message_names_path` |
| TC-1837 | FR-1837 | `state_dir_env_overrides_base` |
| TC-1838 | FR-1838 | `repo_key_is_stable_fnv1a` |
| TC-1839 | FR-1839 | `legacy_db_is_copied_once_and_left_in_place` |
| TC-1840 | FR-1840 | `event_log_seq_is_monotonic_across_handles` |

## 6. Exit criteria
TCs pass in CI. Dogfood #12 is closed.
