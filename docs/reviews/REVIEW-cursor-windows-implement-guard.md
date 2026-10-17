# REVIEW: Disable Cursor Implement mode on Windows

Status: complete  
Spec: [SPEC-cursor-windows-implement-guard.md](../specs/SPEC-cursor-windows-implement-guard.md)  
Reviewed: 2026-08-24  
Target: Windows (`x86_64-pc-windows-msvc`)

## Spec compliance

- INT-480: `AgentAdapter::validate_mode` is called by the UI before mode
  binding retirement. Cursor returns an actionable `UnsupportedMode` error for
  Windows Implement, leaving the picker, selected mode, and active binding
  unchanged.
- NFR-480: Cursor validates again in the runtime loop and at the beginning of
  `run_turn`, before `cursor_command()`, argument construction, or child
  spawning. No Windows Cursor Implement process can be started through these
  paths.
- INT-481: Ask and Plan remain available on every target. The existing
  supported-platform command mapping, including Implement's
  `--force --sandbox enabled`, remains unchanged.

## Regression and safety review

- The change is limited to the provider capability contract, Cursor adapter,
  mode-picker preflight, README, baseline traceability, and deterministic
  tests. It does not alter Codex, AGY, persistence schema, authentication, or
  the Cursor Ask/Plan protocol.
- The UI validates before calling `retire_active_binding`, so an unavailable
  mode cannot stale a valid native session. The adapter and pre-spawn guards
  provide defense in depth for non-UI callers.
- There is no unsandboxed or allowlist fallback. The user can choose Cursor Ask
  or Plan instead.
- The worktree contained pre-existing multi-instance and role-separated-chat
  changes. They were preserved and are not part of this feature's intended
  commit scope.

## Test evidence

- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-cursor-windows-implement-guard.md`: passed.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/DMUXAI-BASELINE.md`: passed.
- `%USERPROFILE%\.cargo\bin\cargo.exe fmt --check`: passed.
- `%USERPROFILE%\.cargo\bin\cargo.exe test --all-targets`: passed; 8 library tests, 93 binary tests, and 31 conversation integration tests passed; 5 authenticated provider smoke tests were ignored as intended.
- Windows-specific tests passed: `windows_cursor_implement_mode_is_rejected_before_queueing`, `windows_cursor_implement_turn_is_rejected_before_process_spawn`, and `unsupported_mode_is_rejected_before_binding_retirement`.
- `%USERPROFILE%\.cargo\bin\cargo.exe clippy --all-targets --all-features -- -D warnings`: passed.
- Live Cursor smoke test: skipped; it requires a locally installed and authenticated Cursor Agent CLI and would make a real provider request.

## Residual risks and rollback

Cursor may add Windows sandbox support in a future release. Until then, the
platform guard intentionally keeps Implement unavailable on Windows. A future
support change should update the capability test and this spec rather than
silently removing the guard. Rollback is code/docs-only; no database migration
or data repair is required.
