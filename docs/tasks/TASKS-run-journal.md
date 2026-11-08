# TASKS-run-journal: Daily execution journal schema, storage, and CLI inspection (PR1a)

- [x] Task 1: Standalone integration test harness in `tests/run_journal_cli.rs` covering TC-2560..TC-2566. Satisfies FR-2560, FR-2561, FR-2562, FR-2563, FR-2564, FR-2565, FR-2566. Verified by `cargo test --test run_journal_cli` (fails RED at runtime on current main).
- [x] Task 2: Create `src/run_journal.rs` with schema_version 1, atomic persistence under `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/<16hexid>/`, 0700/0600 modes, symlink guards, bounded I/O, secret masking, and retention. Satisfies FR-2560, FR-2561, FR-2563, FR-2564, FR-2565.
- [x] Task 3: Add `[runs]` configuration to `src/settings.rs` and export `run_journal` in `src/lib.rs`. Satisfies FR-2565.
- [x] Task 4: Implement CLI commands `runs` and `runs show <id>` with JSON support in `src/cli.rs` and `src/main.rs`. Satisfies FR-2562, FR-2566.
- [x] Task 5: Verify spec and link gates (`python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py`, `cargo check`).
- [x] Task 6: Write `docs/reviews/REVIEW-run-journal.md` with verification evidence.
