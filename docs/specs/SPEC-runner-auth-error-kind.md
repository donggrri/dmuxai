# SPEC: runner-auth-error-kind

Status: Implemented

Owner: dmuxai
Created: 2026-11-07
Baseline: `2cf469e`
Related: #183, SPEC-runner-fallback, SPEC-grok-runner FR-896, SPEC-typed-errors FR-1678

## 1. Decision summary

A logged-out grok CLI is a user-action/auth condition, not a model availability problem. DmuxAI adds an `auth` agent error kind for detectable login/auth probe failures (at minimum the built-in grok preset's `grok_login_message()`), surfaces it in headless `error_kind`, and excludes it from runner fallback so a configured chain does not silently switch runners when the user must run `grok login`.

## 2. Scope

### In scope

- New `AgentErrorKind::Auth` / `error_kind: "auth"` in structured errors and headless JSON.
- Built-in grok model-probe failures that return `grok_login_message()` use `auth`, not `model_unavailable`.
- `classify_failure` treats the `auth` kind as a non-trigger (no handover).
- Runner-fallback and grok-runner specs updated; regression tests.

### Out of scope / non-goals

- Reclassifying mid-turn provider stderr that mentions auth (those stay `provider` and may still hand over per FR-2104).
- OAuth flows, credential storage, or changing the grok login message text.
- Fallback behaviour for quota or unknown-model failures (unchanged).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2280 | Other runners may adopt `auth` when a probe failure is unambiguous; grok is the first mapped case. | dmuxai | PR |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| `auth` error kind | Structured failure that requires the user to sign in or refresh credentials before retrying the same runner. | `FallbackReason::Auth` on a `provider` failure message during a turn. |
| Model probe | Built-in preset step that runs `models_command` before starting a turn. | In-turn API errors. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2280 | Must | Given `AgentErrorKind` serialization | The kind `auth` maps to the stable snake_case string `auth` in JSON and Display | Unknown kinds remain handled by serde on read | TC-1687, TC-2280 |
| FR-2281 | Must | Given the built-in grok preset and `grok models` probe failure that yields `grok_login_message()` | `ListModels` and `Send` emit `AgentError` with kind `auth` and a message that contains ``Run `grok login` ``; authenticated fast-only listings still use `model_unavailable` with `grok_no_allowed_model_message()` | Other built-in presets keep mapping probe failures to `model_unavailable` | TC-2280, TC-2281, TC-897 |
| FR-2282 | Must | Given `classify_failure` with error kind `auth` | Returns `None` (no runner fallback), regardless of message text | Provider/process failures whose message matches auth patterns but kind is not `auth` still classify per FR-2104 | TC-2282, TC-2283 |

## 5. Acceptance scenarios and test design

TC-2280, TC-2282, and TC-2283 are owned by this spec. TC-1687 in
[SPEC-typed-errors](SPEC-typed-errors.md) and TC-2281 in
[SPEC-grok-runner](SPEC-grok-runner.md) are not re-declared here (FR-471: a TC
row lives in exactly one SPEC); the tables and Evidence columns cite them.

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| FR-2280 (via TC-1687 in SPEC-typed-errors) | FR-2280, FR-1678 | unit | `AgentErrorKind::Auth` | serialize and display | string `auth` | `src/agent/mod.rs` `agent_error_kind_strings_and_display` |
| TC-2280 | FR-2281 | unit | grok preset | `builtin_probe_error_kind` with login vs no-models messages | login → `auth`; no allowed models → `model_unavailable` | `src/custom_agent.rs` `tc_2280_builtin_probe_agent_error_maps_grok_login_to_auth` |
| FR-2281 (via TC-2281 in SPEC-grok-runner) | FR-2281 | integration | mock grok with `GROK_FAKE_AUTH=fail` | list models and send prompt | both errors have kind `auth` and mention `grok login`; no `-p` argv | `src/agent/custom.rs` `grok_unauthenticated_send_explains_login` |
| TC-2282 | FR-2282 | unit | `auth` kind and login text | `classify_failure` | `None` | `src/runner_fallback.rs` `tc_2282_auth_error_kind_never_falls_back` |
| TC-2283 | FR-2282 | integration | App with agy→codex fallback chain | apply `AgentError::auth(grok_login_message())` | single submit; empty fallback history | `src/app.rs` `tc_2283_auth_error_does_not_fall_back` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Headless JSON | New optional `error_kind` value `auth` | Clients that ignore unknown kinds are unaffected | TC-2281, TC-2283 |
| Runner fallback | `auth` is in the non-trigger set with `cancelled` and `policy` | Revert removes the kind and restores grok `model_unavailable` mapping | TC-2282, TC-2283 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | `Auth` kind and grok probe mapping | `src/agent/mod.rs`, `src/custom_agent.rs`, `src/agent/custom.rs` | FR-2280, FR-2281 | TC-1687, TC-2280, TC-2281 |
| 2 | Fallback exclusion | `src/runner_fallback.rs`, `src/app.rs` | FR-2282 | TC-2282, TC-2283 |
| 3 | Specs and review | `docs/specs/*`, `docs/plans/*`, `docs/tasks/*`, `docs/reviews/*` | all | `python3 scripts/check-spec.py --all` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-2280..FR-2282) map to passing test evidence (TC-1687, TC-2280..TC-2283, TC-897) — see [REVIEW-runner-auth-error-kind.md](../reviews/REVIEW-runner-auth-error-kind.md).
- [x] `classify_failure` still hands over on `provider` + auth-shaped messages (FR-2104, TC-2104).
- [x] Required Rust checks pass for touched tests: `cargo fmt --check` and the nine focused tests listed in the review.
