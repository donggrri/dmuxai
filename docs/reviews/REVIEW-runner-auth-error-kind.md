# REVIEW: runner auth error kind (#183)

Spec: [SPEC-runner-auth-error-kind.md](../specs/SPEC-runner-auth-error-kind.md)

## Compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-2280 | Met | `agent_error_kind_strings_and_display` includes `auth` |
| FR-2281 | Met | `tc_2280_*`, `grok_unauthenticated_send_explains_login` assert `AgentErrorKind::Auth` and `grok login` text |
| FR-2282 | Met | `tc_2282_*`, `tc_2283_*`; FR-2104 provider+auth message test unchanged in `tc_2104_classify_triggers` |

## Residual risks

- Headless consumers must treat `error_kind: "auth"` as non-fallback; unknown-kind clients may mislabel until updated.
- Only grok probe login is mapped to `auth`; other runners still use `provider` for in-turn auth errors.

## Test evidence

Commands run during review (2026-11-08, worktree @2b610bf + staged WIP):

```bash
cargo fmt --check                                                        # clean
cargo test --all-targets -- agent_error_kind_strings_and_display         # 1 passed
cargo test --all-targets -- tc_2280_builtin_probe_agent_error_maps_grok_login_to_auth   # 1 passed
cargo test --all-targets -- grok_unauthenticated_send_explains_login     # 1 passed
cargo test --all-targets -- grok_auth_is_models_stdout_only              # 1 passed
cargo test --all-targets -- tc_2282_auth_error_kind_never_falls_back     # 1 passed
cargo test --all-targets -- tc_2283_auth_error_does_not_fall_back        # 1 passed
cargo test --all-targets -- tc_2104_classify_triggers                    # 1 passed
cargo test --all-targets -- tc_2105_no_fallback_on_normal_failure        # 1 passed
cargo test --all-targets -- tc_1695_outcome_json_includes_error_kind     # 1 passed
python3 scripts/check-spec.py --all                                      # 87/87 pass, 0 allowlisted
python3 scripts/check-tc-links.py                                        # 0 orphan / 0 undefined / 0 stale
cargo test --test spec_index                                             # 10 passed
```

During review the `spec_ids_are_unique_across_specs` gate (FR-471) failed because
TC-1687, TC-2281, TC-2282, and TC-2283 were first-cell table rows in two SPEC
files each. Fixed by keeping TC ownership in SPEC-typed-errors (TC-1687) and
SPEC-grok-runner (TC-2281), citing them from this spec, and owning TC-2282/TC-2283
in this spec while SPEC-runner-fallback cites them.
