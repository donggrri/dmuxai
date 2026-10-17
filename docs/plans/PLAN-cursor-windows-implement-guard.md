# PLAN: Disable Cursor Implement mode on Windows

Status: complete  
Spec: [SPEC-cursor-windows-implement-guard.md](../specs/SPEC-cursor-windows-implement-guard.md)

1. Extend the provider adapter contract with a side-effect-free mode
   validation hook and an explicit unsupported-mode error. (INT-480, TC-480)
2. Reject Cursor Implement on Windows during capability validation and again at
   the runtime boundary before command construction/spawning; keep Ask, Plan,
   and supported-platform command flags unchanged. (NFR-480, INT-481, TC-481..TC-483)
3. Make the mode picker call validation before retiring a provider binding or
   accepting the new mode, so an unavailable Windows mode cannot alter durable
   state. (INT-480, TC-480)
4. Update README and baseline traceability, run the spec check and Rust gates,
   and record review evidence. (all requirements)

## Risks and mitigations

- A future code path could bypass the UI mode picker; keep the runtime guard
  before `cursor_command()` as defense in depth.
- A generic provider capability hook could be ignored by a caller; keep
  `select_mode` validation and the pre-spawn check in the Cursor adapter.
- Platform-specific tests can be skipped accidentally; run the full suite on
  Windows and record the target platform in the review.

## Rollback

This is a code and documentation change with no persistence migration. Revert
the adapter validation, UI preflight, runtime guard, and documentation to
restore the prior behavior; no database rollback is required.
