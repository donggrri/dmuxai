# PLAN-run-journal: Daily execution journal schema, storage, and CLI inspection (PR1a)

## 1. Goal and architectural boundaries

Implement PR1a: run journal schema version 1, durable atomic storage under `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/<16hexid>/`, sensitive credential masking, POSIX 0700/0600 permissions, symlink and traversal rejection, bounded I/O, configurable 90d/1GiB retention, and `dmuxai runs` / `dmuxai runs show` CLI subcommands.
All live turn start/finish wiring and live stderr capturing are deferred to PR1b. `src/app.rs` and live process runner dispatch remain untouched.

## 2. Dependency order

1. **Test-first specification & integration harness (`tests/run_journal_cli.rs`)**:
   - Create black-box CLI tests calling the compiled `dmuxai` binary via `std::process::Command`.
   - Implement tests `tc_2560` through `tc_2566` mapped directly to `FR-2560` through `FR-2566`.
   - Verify that tests compile against current codebase and fail at runtime (RED) because the `runs` subcommand does not yet exist.
2. **Core storage and schema module (`src/run_journal.rs`)**:
   - Schema version 1 types: `RunManifest`, `RunOrigin`, `RunStatus` (`active`, `completed`, `failed`, `cancelled`), `RunLinks`, `RunDiagnosticsRef`, etc.
   - Persistence root resolution using `XDG_DATA_HOME` (fallback `$HOME/.local/share/dmuxai`).
   - Atomic write via temporary file in the target run directory and atomic replace.
   - Unix 0700 directory / 0600 file creation modes via `std::os::unix::fs::DirBuilderExt` and `OpenOptionsExt`.
   - Path traversal guard: 16-hex validation (`[0-9a-f]{16}`). Symlink rejection via `symlink_metadata()`.
   - Secret redaction before write (`mask_sensitive_tokens`): API keys, Bearer tokens, Authorization headers, query tokens, and ANSI stripping.
   - Bounded I/O: 128 KiB manifest read limit, 64 KiB diagnostics tail write limit.
   - Retention policy: prune terminal runs (`completed`, `failed`, `cancelled`) older than 90 days or exceeding 1 GiB total size, strictly preserving active runs.
3. **Settings configuration (`src/settings.rs`)**:
   - Add `[runs]` table: `retention_days` (default 90) and `retention_bytes` (default 1073741824).
4. **CLI subcommand wiring (`src/cli.rs`, `src/main.rs`)**:
   - Add `AgentCommand::Runs` and `AgentCommand::RunsShow`.
   - Handle `--limit`, `--failed`, and `--json`.
   - Return exit code 0 on success, exit code 2 on invalid arguments or traversal, exit code 3 on unknown ID, exit code 6 on I/O error.
5. **Review and validation**:
   - Generate `docs/reviews/REVIEW-run-journal.md`.
   - Verify that all gates pass.

## 3. Risks and rollback

- Risk: Accidental modification of existing conversation databases.
  Mitigation: Storage path is strictly separated under `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/` instead of the state database directory (`$XDG_STATE_HOME/dmuxai`).
- Risk: Symlink attacks traversing outside the runs directory during read or retention pruning.
  Mitigation: Strict `symlink_metadata()` check at every directory level, rejecting any symlink and never unlinking or chmodding outside targets.
- Rollback: Delete the `runs/` directory; SQLite conversation history and event logs are completely independent.
