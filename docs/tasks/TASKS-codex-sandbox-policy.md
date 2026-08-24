# TASKS: Codex sandbox-policy object compatibility

Status: complete
Spec: [SPEC-codex-sandbox-policy.md](../specs/SPEC-codex-sandbox-policy.md)

- [x] T1 — Make TC-046 fail against the legacy string request shape. Requirements:
  INT-040. Verify: `cargo test codex_execution_modes_set_per_turn_sandbox_and_approval_policy`.
- [x] T2 — Emit the tagged sandbox-policy object and preserve other turn fields.
  Requirements: INT-040, NFR-041. Verify: `cargo test codex_execution_modes_set_per_turn_sandbox_and_approval_policy`.
- [x] T3 — Update baseline/review and run required gates. Requirements: all.
  Verify: spec checks, `cargo fmt --check`, `cargo test --all-targets`, and clippy.
