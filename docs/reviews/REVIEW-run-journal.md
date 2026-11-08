# REVIEW-run-journal: Daily execution journal schema, storage, and CLI inspection (PR1a)

## 1. Executive Summary

PR1a establishes the foundation for DmuxAI execution run journaling:
- Clean module API in `src/run_journal.rs` exported via `src/lib.rs`.
- Strict schema version 1 manifests with explicit null values for optional/unpopulated fields and forward-compatible serde deserialization (`dmuxai_exit_code` supported).
- Durable atomic persistence under `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/<16hexid>/` (fallback `~/.local/share/dmuxai/repos/<repo-key>/runs/<16hexid>/`).
- POSIX 0700 directory and 0600 file permissions at creation on Unix platforms using `O_NOFOLLOW` and secure random temp files.
- Robust security defenses rejecting symlink components at root, run directory, and leaf files without touching or following external symlink targets.
- Pre-disk redaction of sensitive credentials (API keys, Bearer headers, Authorization schemes, URL query tokens, JSON quoted secret fields, and quoted assignments with spaces) along with ANSI/control escape sequence stripping before write and prior to tail truncation.
- Bounded I/O: manifest read bounded at 128 KiB; diagnostics snapshot write bounded at 64 KiB tail buffer with UTF-8 boundary safety.
- Retention policy with default 90 days age and 1 GiB total size limits configurable via `[runs]` table in `dmuxai.toml`; preserves active, corrupt, and future schema version runs.
- CLI subcommands `dmuxai runs [--limit N] [--failed] [--json]` and `dmuxai runs show <id> [--json]` with exit codes: 0 ok, 2 invalid arguments/traversal, 3 unknown run id, 6 I/O failure.

Note: PR1a implements the storage engine, schema, and inspection CLI only; live turn capture, process supervisor hooks, summary generation, and dashboard integration are deferred to PR1b and PR2.

## 2. Requirement Traceability

| Requirement ID | Test ID | Test Status | Evidence |
| --- | --- | --- | --- |
| FR-2560 | TC-2560 | PASS | `tests/run_journal_cli.rs`: `tc_2560_durable_atomic_storage_root_and_permissions`<br>`tests/run_journal_store.rs`: `tc_2560_store_atomic_save_load_permissions_and_overwrite` |
| FR-2561 | TC-2561 | PASS | `tests/run_journal_cli.rs`: `tc_2561_manifest_schema_version_1_nullable_metadata`<br>`tests/run_journal_store.rs`: `tc_2561_store_pre_disk_masking_secrets_and_urls` |
| FR-2562 | TC-2562 | PASS | `tests/run_journal_cli.rs`: `tc_2562_cli_runs_list_and_show_contract` |
| FR-2563 | TC-2563 | PASS | `tests/run_journal_cli.rs`: `tc_2563_symlink_rejection_and_traversal_guards`<br>`tests/run_journal_store.rs`: `tc_2563_store_symlink_defense_and_outside_file_protection` |
| FR-2564 | TC-2564 | PASS | `tests/run_journal_cli.rs`: `tc_2564_pre_disk_masking_and_bounded_io`<br>`tests/run_journal_store.rs`: `tc_2564_store_bounded_manifest_read_and_bounded_diagnostics_write` |
| FR-2565 | TC-2565 | PASS | `tests/run_journal_cli.rs`: `tc_2565_retention_pruning_protects_active_and_corrupt_runs`<br>`tests/run_journal_store.rs`: `tc_2565_store_retention_prune_preserves_active_corrupt_and_new_schemas` |
| FR-2566 | TC-2566 | PASS | `tests/run_journal_cli.rs`: `tc_2566_io_failure_handling_and_exit_codes`<br>`tests/run_journal_store.rs`: `tc_2566_store_file_root_returns_error_without_panic` |

## 3. Verification Commands and Results

1. Unit and storage integration tests:
   ```bash
   cargo test --test run_journal_store
   ```
   Result: **6 passed; 0 failed; 0 ignored**

2. CLI contract tests:
   ```bash
   cargo test --test run_journal_cli
   ```
   Result: **7 passed; 0 failed; 0 ignored**

3. Codebase checks:
   ```bash
   cargo check
   ```
   Result: Finished with 0 errors, 0 warnings.

4. Spec checks:
   ```bash
   bash scripts/check-spec.sh docs/specs/SPEC-run-journal.md
   python3 scripts/check-spec.py --all
   python3 scripts/check-tc-links.py
   ```
   Result: 95/95 specs pass; 0 stale, 0 failures; all 1104 TC links verified.

## 4. Residual Risks and Next Steps

- PR1a deliberately contains no live execution hooks (`src/app.rs` untouched).
- PR1b will wire process execution lifecycle events, live stderr capture into `diagnostics.txt`, and generate `end_summary`.
