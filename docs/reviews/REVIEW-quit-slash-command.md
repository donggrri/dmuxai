# REVIEW: Slash-command quit

Status: complete  
Spec: [SPEC-quit-slash-command.md](../specs/SPEC-quit-slash-command.md)
Reviewed: 2026-08-24

## Result

The exit action now uses the `/quit` slash command. Exact `/quit` submission
and slash-preview selection through Enter or Tab enter the existing application
exit state, while `Esc` only dismisses an active overlay or preview and is a
no-op in the main prompt. No provider protocol, persistence schema, or
shutdown implementation was changed.

## Requirement review

- UX-011: `/quit` is listed in the slash catalog, appears for `/` and `/q`,
  executes without starting a provider turn, and treats `/quit ` as ordinary
  prompt text. Covered by TC-019, TC-020, and TC-022.
- UX-012: main-prompt `Esc` no longer exits; slash-preview and conversation
  picker dismissal remain intact. Covered by TC-021.
- Non-goals: `Ctrl+C`, child-process cleanup, terminal restoration, provider
  requests, and SQLite data behavior remain unchanged.

## Test evidence

All commands were run from the repository root. Cargo was invoked through
`%USERPROFILE%\.cargo\bin\cargo.exe` because Cargo was not on the
PowerShell `PATH`.

- `cargo fmt --check`: passed.
- `cargo test --bin dmuxai quit`: 4 passed.
- `cargo test --bin dmuxai slash_preview_can_execute_quit`: 2 passed.
- `cargo test --bin dmuxai escape_never_exits_the_application`: 1 passed.
- `cargo test --all-targets`: 102 passed, 6 ignored, 0 failed. The ignored
  tests require authenticated provider CLIs and were not counted as passes.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `scripts/check-spec.ps1` for `docs/specs/SPEC-quit-slash-command.md`: passed.
- `scripts/check-spec.ps1` for `docs/specs/DMUXAI-BASELINE.md`: passed.
- `git diff --check`: passed; Git reported only existing line-ending notices.

Authenticated Codex, Cursor, and AGY smoke tests were skipped because live CLI
installation and authentication were not established. An interactive terminal
smoke test was not run; the existing shutdown path is unchanged and the state
transition is covered deterministically.

## Residual risk and rollback

The remaining risk is limited to interactive terminal presentation of the new
status-line hint, which is covered by the existing rendering path but was not
manually exercised. Rollback requires reverting the `/quit` catalog/action and
submission branch, restoring the main-prompt `Esc` transition, and reverting
the associated documentation/tests; no data migration or recovery is needed.

The worktree contained unrelated pre-existing changes. They were preserved and
excluded from this feature review; no reset, checkout, or commit was performed.
