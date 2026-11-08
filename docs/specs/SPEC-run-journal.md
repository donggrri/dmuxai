# SPEC-run-journal: Daily execution journal schema, storage, and CLI inspection (PR1a)

Status: implemented
Owner: DmuxAI Team
Created: 2026-11-08
Baseline: main 1a1dc8b; existing highest FR-2555 / TC-2554; PR1a adds FR/TC-2560–2567
Related: SPEC-state-per-repo, SPEC-turn-record, SPEC-agent-api, SPEC-process-supervisor

## 1. Decision summary

PR1a provides storage and inspection for durable, private local execution records under `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/<16hexid>/` (fallback `~/.local/share/dmuxai/repos/<repo-key>/runs/<16hexid>/`), completely isolated from the SQLite conversation database and state directory. Automatic creation for CLI/web/TUI executions is deferred to PR1b; PR1a does not yet generate records during real runs. Stored run records contain schema_version 1 manifests with explicit null values for unknown fields, bounded UTF-8 safe write/read diagnostics, pre-persistence credential/secret masking, POSIX 0700 directory and 0600 file permissions AT CREATION, and 90-day / 1 GiB configurable retention. Operators inspect run history using the CLI commands `dmuxai runs [--limit N] [--failed] [--json]` and `dmuxai runs show <id> [--json]`.

## 2. Scope

### In scope (PR1a)

- Run journal schema version 1 and filesystem layout rooted strictly at XDG Data Home: `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/<16hexid>/` (fallback `$HOME/.local/share/dmuxai`).
- Stable manifest schema (`manifest.json`) with explicit null values for unknown fields: `run_id`, `origin`, `status` (`active`, `completed`, `failed`, `cancelled`), timestamps (`created_at_ms`, `completed_at_ms`, `duration_ms`), requested/resolved runner and model, version metadata (`dmuxai_version`, `build_commit`), child process execution (`child_exit_code`, `child_signal`), sanitized error (`error_kind`, `error_summary`), correlation links (`conversation_id`, `turn_id`, `graph_id`, `node_id`, `attempt_index`), diagnostics refs, and nullable `end_summary` reserved for PR1b.
- Atomic durable storage using temporary files and rename within the same filesystem directory; atomic directory creation with 0700 permissions and file creation with 0600 permissions on Unix.
- Security and isolation: strict validation of 16-hex lowercase run ID, rejection of path traversal (`..`, `/`), rejection of symlinks anywhere in the path hierarchy (root, repo, run directory, manifest, diagnostics) without touching or deleting external targets; fixture outside markers survive.
- Bounded I/O: manifest read bounded (128 KiB); diagnostics tail bounded on write (64 KiB tail buffer) and read; UTF-8 safe boundary truncation.
- Secret and credential masking BEFORE disk write: pattern masking for `API_KEY`, `TOKEN`, `SECRET`, `PASSWORD` assignments (including JSON quoted keys and values, unquoted assignments, quoted strings with spaces), `Bearer <token>`, `Authorization: <scheme> <token>`, and query tokens (`?token=...`, `&token=...`); ANSI and terminal control sequence stripping.
- Retention enforcement: configurable 90-day age and 1 GiB total size quota per repository ([runs] settings); prunes terminal runs (`completed`, `failed`, `cancelled`) oldest first; active runs preserved even when exceeding quota; malformed or unreadable records skipped with warning and preserved; symlinks skipped; unremovable files not deducted from calculated usage.
- CLI subcommands: `dmuxai runs [--limit N] [--failed] [--json]` (default limit 20, noninteractive machine array) and `dmuxai runs show <id> [--json]` (manifest object); exit 0 on success, exit 2 on invalid arguments or path traversal, exit 3 on unknown ID, exit 6 on I/O failure; stdout contains only JSON when `--json` is supplied; stderr contains diagnostics and warnings; missing root directory produces an empty list. No live runners, network, configs, or hooks invoked during list/show.

### Out of scope / non-goals

- PR1b live execution wiring, process supervisor hooks, live stderr streaming/journal capture, and `end_summary` population.
- Altering `src/app.rs`, live turn dispatch, or SQLite conversation database schemas.
- Remote telemetry, uploads, cloud synchronization, or network diagnostics.
- Automatic crash recovery, watchdog taxonomy, or PR2 heartbeat gap triage.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2560 | Storage failure must emit a visible warning on stderr once without altering the exit status of the running process. | DmuxAI Team | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Run Journal Root | Storage directory at `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/` (fallback `$HOME/.local/share/dmuxai/...`). | SQLite database directory (`$XDG_STATE_HOME/dmuxai`) or `.dmuxai/events.jsonl`. |
| Run ID | Exactly 16 lowercase hexadecimal ASCII characters (`[0-9a-f]{16}`) identifying an execution attempt. | Conversation ID, Turn ID, or Graph Node ID. |
| Masking / Redaction | Sanitization of API keys, bearer tokens, passwords, and sensitive URLs before disk write. | User privacy filtering of workspace source files. |

Run lifecycle state transitions:

| Current state | Trigger | Resulting state |
| --- | --- | --- |
| None | Execution attempt created | `active` (manifest written atomically with created_at_ms) |
| `active` | Process/turn succeeds | `completed` (outcome ok, completed_at_ms, duration_ms) |
| `active` | Process/turn fails | `failed` (error_kind, error_summary, exit code) |
| `active` | User/system cancel | `cancelled` (cancellation marked) |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2560 | Must | When a run journal is stored or loaded on disk, | Storage root is `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/<16hexid>/` (fallback `~/.local/share/dmuxai/`); manifest writes are atomic via temp-file and rename; directories are created with 0700 and files with 0600 on Unix AT CREATION; ID inside manifest must match directory name. | Traversal or malformed IDs are rejected before any filesystem call; manifest write errors emit a stderr warning and do not panic. | TC-2560 |
| FR-2561 | Must | When a manifest is serialized or deserialized (schema_version 1), | All stable fields are present with explicit null for unknown values: run_id, origin, status (active/completed/failed/cancelled), created_at_ms, completed_at_ms, duration_ms, requested/resolved runner and model, version metadata, child_exit_code, child_signal, error_kind, error_summary, correlation links (conversation_id, turn_id, graph_id, node_id, attempt_index), diagnostics refs, end_summary. | Unknown or future schema versions emit a warning and are skipped without pruning. | TC-2561 |
| FR-2562 | Must | When `dmuxai runs [--limit N] [--failed] [--json]` or `dmuxai runs show <id> [--json]` is executed, | `runs` lists runs sorted newest first up to limit (default 20, machine JSON array); `--failed` filters for failed runs; `show <id>` displays manifest (machine JSON object); exit 0 on success; exit 2 on invalid limit/flags/traversal; exit 3 on unknown ID; exit 6 on I/O error; stdout contains only JSON when `--json` is set; empty root outputs empty array/list without starting runners or network. | Missing root directory exits 0 with empty list; malformed manifests warn and are skipped in list. | TC-2562 |
| FR-2563 | Must | When reading or writing journal paths, manifests, and diagnostics, | Symlinks are strictly rejected at runs root, run dir, manifest, and diagnostics; traversal paths (`..`, `/`) are rejected; symlinks are never dereferenced, chmodded, read, or deleted; external targets remain untouched. | Symlink encountered returns error or is skipped; marker files outside the runs hierarchy survive untouched. | TC-2563 |
| FR-2564 | Must | Before writing any metadata string or diagnostic tail to disk, | Secrets matching API_KEY/TOKEN/SECRET/PASSWORD assignments, Bearer/Authorization headers, and URL query tokens are masked with `[REDACTED]`; ANSI escape and control characters are stripped; diagnostics writes are bounded to 64 KiB tail buffer; manifest reads are bounded to 128 KiB; UTF-8 boundary slices are safe. | Boundary splits and complete records do not leak partial keys to disk; no network or credential store reads. | TC-2564 |
| FR-2565 | Must | When retention pruning is executed with configured age (default 90 days) and total size (default 1 GiB), | Terminal runs (completed, failed, cancelled) exceeding age or size budget are deleted oldest first; active runs are preserved even when exceeding quota; corrupt or unknown schema records are preserved; symlinks are skipped; failed deletions are not deducted from disk usage. | Non-terminal (active) runs are never deleted by retention. | TC-2565 |
| FR-2566 | Must | When the storage filesystem is read-only or an I/O error occurs during journal write or read, | A diagnostic warning is emitted to stderr once, and the error returns a typed I/O failure (CLI exit 6) without crashing or masking execution outcomes. | Unwritable storage does not panic or corrupt existing records. | TC-2566 |
| FR-2567 | Must | Boundary storage operations with oversized or mismatched records, nested secret fields, or unsafe children | Reads and writes stay bounded; invalid manifests remain unpruned; nested secret-key values are masked; retention-save returns its report/errors; unsafe child entries preserve the run | No partial replacement on oversized writes; failed cleanup is not counted | TC-2567 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2560 | FR-2560 | integration | CLI test runner with isolated XDG_DATA_HOME sandbox | Execute `dmuxai runs` and inspect XDG data path layout and permissions | Directory is `$XDG_DATA_HOME/dmuxai/repos/<key>/runs/<id>` with 0700/0600 permissions; atomic replace prevents partial reads | `tests/run_journal_cli.rs`: `tc_2560_durable_atomic_storage_root_and_permissions`, `tests/run_journal_store.rs`: `tc_2560_store_atomic_save_load_permissions_and_overwrite` |
| TC-2561 | FR-2561 | integration | Stored run fixture with schema_version 1 and nullable fields | Invoke `dmuxai runs show <id> --json` | Manifest JSON includes explicit nulls for unpopulated fields and accurate correlation links | `tests/run_journal_cli.rs`: `tc_2561_manifest_schema_version_1_nullable_metadata`, `tests/run_journal_store.rs`: `tc_2561_store_pre_disk_masking_secrets_and_urls` |
| TC-2562 | FR-2562 | integration | Sandboxed environment with multiple run records | Execute `dmuxai runs` with `--limit`, `--failed`, `--json` and invalid args | CLI returns machine JSON array for list, object for show; exit 2 on bad args, exit 3 on unknown ID, exit 0 for empty root | `tests/run_journal_cli.rs`: `tc_2562_cli_runs_list_and_show_contract` |
| TC-2563 | FR-2563 | integration | Filesystem containing symlinks inside and outside the runs hierarchy | Read or prune runs via CLI / storage functions | Symlinks are rejected and not followed; external files are never read, modified, or unlinked | `tests/run_journal_cli.rs`: `tc_2563_symlink_rejection_and_traversal_guards`, `tests/run_journal_store.rs`: `tc_2563_store_symlink_defense_and_outside_file_protection` |
| TC-2564 | FR-2564 | integration | Stored diagnostic evidence containing API keys, bearer tokens, and ANSI color codes | Inspect written diagnostics and manifest strings | All secrets are replaced by `[REDACTED]`, ANSI escapes stripped, writes bounded within 64 KiB tail | `tests/run_journal_cli.rs`: `tc_2564_pre_disk_masking_and_bounded_io`, `tests/run_journal_store.rs`: `tc_2564_store_bounded_manifest_read_and_bounded_diagnostics_write` |
| TC-2565 | FR-2565 | integration | Repository with active runs and old terminal runs exceeding age/size quota | Run retention policy | Terminal runs older than 90 days or exceeding 1 GiB pruned oldest first; active runs preserved intact | `tests/run_journal_cli.rs`: `tc_2565_retention_pruning_protects_active_and_corrupt_runs`, `tests/run_journal_store.rs`: `tc_2565_store_retention_prune_preserves_active_corrupt_and_new_schemas` |
| TC-2566 | FR-2566 | integration | Read-only XDG_DATA_HOME or storage failure | Attempt journal operations or CLI show | Emits warning on stderr, returns exit code 6 for I/O errors without process crash | `tests/run_journal_cli.rs`: `tc_2566_io_failure_handling_and_exit_codes` |
| TC-2567 | FR-2563, FR-2564, FR-2565, FR-2567 | integration | Oversized/mismatched manifests, nested credentials, symlink diagnostic leaf, active and terminal records | Call storage save/load/prune and retention-save | Raw persisted secrets absent, prior manifest survives rejected replacement, invalid/unsafe records preserved, successful deletion report accurate | `tests/run_journal_store.rs`: `tc_2567_storage_hardening_regressions` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/<16hexid>/manifest.json` and `diagnostics.log`. Completely separate from SQLite DB (`conversations.sqlite3`). | Directory can be deleted to roll back with zero data loss to conversation history. | TC-2560, TC-2563 |
| CLI protocol | `dmuxai runs [--limit N] [--failed] [--json]` and `dmuxai runs show <id> [--json]`. | Additive CLI subcommands; existing subcommands (`run`, `status`, `events`, `todo`) unchanged. | TC-2562 |
| Security | Enforces 0700 dir and 0600 file modes at creation. Pre-disk redaction for credentials. Symlink and traversal rejection. | Does not contact network, does not dump system environment variables or auth tokens. | TC-2563, TC-2564 |
| Retention | 90 days / 1 GiB default limits configurable in settings `[runs]`. | Pruning only removes terminal runs inside the repo's `runs/` directory; never deletes active runs or conversation databases. | TC-2565 |

## 7. Implementation plan

Detailed plan: [PLAN-run-journal.md](../plans/PLAN-run-journal.md).
Task checklist: [TASKS-run-journal.md](../tasks/TASKS-run-journal.md).

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Create test-first standalone integration test harness proving expected RED behavior | `tests/run_journal_cli.rs` | FR-2560..FR-2566 | `cargo test --test run_journal_cli` |
| 2 | Implement `src/run_journal.rs` domain types, schema_version 1, masking, and retention | `src/run_journal.rs`, `src/lib.rs` | FR-2560, FR-2561, FR-2563, FR-2564, FR-2565, FR-2566 | TC-2560, TC-2561, TC-2563, TC-2564, TC-2565, TC-2566 |
| 3 | Add `[runs]` configuration table to settings | `src/settings.rs` | FR-2565 | `cargo test --lib settings` |
| 4 | Add CLI `runs` and `runs show <id>` subcommands | `src/cli.rs`, `src/main.rs` | FR-2562, FR-2566 | TC-2562, TC-2566 |
| 5 | Verify review and gates | `docs/reviews/REVIEW-run-journal.md` | All | `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` |

## 8. Review and test exit criteria

- [ ] All Must requirements (FR-2560..FR-2566) map to strong `tc_2560`..`tc_2566` test functions in `tests/run_journal_cli.rs`.
- [ ] Spec check passes `bash scripts/check-spec.sh docs/specs/SPEC-run-journal.md`.
- [ ] Repository spec check `python3 scripts/check-spec.py --all` passes with 0 failures and 0 stale entries.
- [ ] Link check `python3 scripts/check-tc-links.py` passes cleanly.
