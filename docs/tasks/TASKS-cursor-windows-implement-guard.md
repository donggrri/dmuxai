# TASKS: Disable Cursor Implement mode on Windows

Status: complete  
Spec: [SPEC-cursor-windows-implement-guard.md](../specs/SPEC-cursor-windows-implement-guard.md)

- [x] T1 - Add `UnsupportedMode` and the default adapter mode-validation hook.
  Requirements: INT-060, INT-061. Verify: `cargo test cursor_mode_validation_matches_platform`.
- [x] T2 - Reject Cursor Implement on Windows during mode selection and before
  turn process spawning; retain Ask/Plan and supported-platform flags.
  Requirements: NFR-060, INT-061. Verify: `cargo test windows_cursor_implement_turn_is_rejected_before_process_spawn`
  and `cargo test cursor_execution_modes_use_native_cli_flags`.
- [x] T3 - Make the UI preflight mode capability before binding retirement or
  mode persistence. Requirements: INT-060. Verify:
  `cargo test unsupported_mode_is_rejected_before_binding_retirement`.
- [x] T4 - Update README and baseline traceability, then run the spec check,
  format, all-target tests, and clippy. Requirements: all.
  Verify: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-cursor-windows-implement-guard.md`,
  `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/DMUXAI-BASELINE.md`,
  `cargo fmt --check`, `cargo test --all-targets`, and
  `cargo clippy --all-targets --all-features -- -D warnings`.
