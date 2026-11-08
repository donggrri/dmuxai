# TASKS: runner auth error kind (#183)

Spec: [SPEC-runner-auth-error-kind.md](../specs/SPEC-runner-auth-error-kind.md)

- [x] T1 - Add `AgentErrorKind::Auth` and extend kind-string test. Requirements: FR-2280 (TC-1687 in SPEC-typed-errors). Verify: `cargo test -- agent_error_kind_strings_and_display` (`src/agent/mod.rs`).
- [x] T2 - `builtin_probe_agent_error` and grok worker wiring. Requirements: FR-2281. Verify: `cargo test tc_2280_builtin_probe_agent_error_maps_grok_login_to_auth grok_unauthenticated_send_explains_login`.
- [x] T3 - Fallback exclusion and app integration test. Requirements: FR-2282. Verify: `cargo test tc_2282_auth_error_kind_never_falls_back tc_2283_auth_error_does_not_fall_back`.
- [x] T4 - Spec/plan/review docs and spec gates. Requirements: all. Verify: `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py`.
