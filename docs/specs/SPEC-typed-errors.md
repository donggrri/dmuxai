# SPEC: typed-errors

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `4841710`
Related: #133, #134; SPEC-plan-gate FR-977 (TC-1680, TC-1681); SPEC-cursor-stream-dedupe FR-800 (TC-1684, TC-1685, TC-1686)

## 1. Decision summary

Runner failures are no longer reported as bare strings or NUL-prefixed sentinels: every `AgentEvent::Error` carries a structured `AgentError { kind, message, hint }` whose `kind` is one of seven stable categories. Each runner maps its concrete failure condition to the matching kind, so the UI, headless JSON, and event log can present and persist the category without parsing prose. Headless output and the event log add `error_kind` next to the existing `error` field without changing any other field. Custom-agent `error_path` accepts one JSON pointer or an ordered list, with a compact-JSON fallback for an `/error` object, and the opencode preset, docs, and init template agree on `["/error/data/message", "/error/message"]`. Cancellation no longer uses NUL-prefixed string sentinels.

## 2. Scope

### In scope

- `AgentError` / `AgentErrorKind` in `src/agent/mod.rs` and runner mappings in `src/agent/{custom,codex,cursor,agy,acp}.rs`.
- `error_kind` in `HeadlessOutcome`, `outcome_json`, and the event log draft.
- Pointer-or-list `error_path` in `src/custom_agent.rs`, including the `/error` object compact-JSON fallback.
- Preset, docs, and init-template agreement plus the drift guard test.

### Out of scope / non-goals

- Redesigning provider protocols or adding new error categories.
- Changing exit codes, cancellation semantics, or the text of existing error messages.
- Redefining TC-1680, TC-1681, TC-1684, TC-1685, or TC-1686, which belong to SPEC-plan-gate and SPEC-cursor-stream-dedupe.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1678 | `error_kind` is `null`/absent when a run has no agent error. | dmuxai | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| AgentError | Structured runner error with `kind`, `message`, and optional `hint`. | A plain `String` error. |
| AgentErrorKind | One of `provider`, `model_unavailable`, `process_exit`, `timeout`, `idle`, `cancelled`, `policy`. | Exit code or HTTP status. |
| error_path | JSON pointer or ordered list of pointers used to extract an error message from a custom agent JSONL line. | `text_path`. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1678 | Must | Given a runner emits a turn failure | `AgentEvent::Error` carries `AgentError { kind, message, hint }`; the seven kinds are `provider`, `model_unavailable`, `process_exit`, `timeout`, `idle`, `cancelled`, `policy`; Display text is `message`, plus `hint` when present; the kind serializes as a stable snake_case string | If `hint` is absent, Display is `message` only | TC-1687, TC-1688 |
| FR-1679 | Must | Given a runner condition: non-zero exit or spawn failure; total timeout; idle watchdog; permission, sandbox, or model-policy refusal; unknown model; provider-reported error; cancellation | Map respectively to `process_exit`, `timeout`, `idle`, `policy`, `model_unavailable`, `provider`, `cancelled`; cancellation uses `AgentError::cancelled` and no NUL-prefixed sentinel strings are sent | Conditions not otherwise classified default to `provider` where applicable | TC-1689, TC-1690, TC-1691, TC-1692, TC-1693 |
| FR-1682 | Must | Given a headless `dmuxai run --json` or an event-log emission on an agent error | The JSON object and event record include `error_kind` next to the existing `error`; all other fields are unchanged | `error_kind` is null or absent when there is no error | TC-1694, TC-1695 |
| FR-1683 | Must | Given a custom-agent JSONL line carrying an `/error` object | `error_path` accepts one pointer or an ordered list tried in sequence, with a compact-JSON fallback for an `/error` object; the opencode preset, `docs/agents/presets.toml`, `docs/agents/custom-agents.md`, and the init template use `["/error/data/message", "/error/message"]`; a test guards drift | Pointers that do not resolve are skipped | TC-1696, TC-1697, TC-1698, TC-1699 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1687 | FR-1678 | unit | `AgentErrorKind` variants | serialize and display each kind | stable snake_case strings | `src/agent/mod.rs` `agent_error_kind_strings_and_display` |
| TC-1688 | FR-1678 | unit | `AgentError` constructors | build and display each error | `kind`, `message`, `hint`, and Display text | `src/agent/mod.rs` `agent_error_constructors_and_display` |
| TC-1689 | FR-1679 | integration | custom runner fixtures | run failing turns | errors map to provider, policy, and process_exit | `src/agent/custom.rs` `failures_are_detected` |
| TC-1690 | FR-1679 | integration | sleeping child | total timeout elapses | `Timeout` kind | `src/agent/custom.rs` `timeout_kills_the_process_group` |
| TC-1691 | FR-1679 | integration | silent agy child | idle watchdog elapses | `Idle` kind | `src/agent/agy.rs` `agy_idle_timeout_kills_silent_child` |
| TC-1692 | FR-1679 | unit | ACP cancel result | map to event | `Cancelled` event without NUL sentinels | `src/agent/custom.rs` `acp_cancel_emits_cancelled_event_without_sentinel` |
| TC-1693 | FR-1679 | unit | graph stop notice | format a failed step | agent error text propagates | `src/app.rs` `tc_1421_cancelled_graph_step_not_reported_as_failed` |
| TC-1694 | FR-1682 | integration | headless run | agent emits error | `outcome.error_kind` is set | `src/app.rs` `headless_run_reports_agent_error` |
| TC-1695 | FR-1682 | unit | `HeadlessOutcome` | serialize via `outcome_json` | `error_kind` appears next to `error` | `src/cli.rs` `tc_1695_outcome_json_includes_error_kind` |
| TC-1696 | FR-1683 | unit | `docs/agents/presets.toml` | deserialize opencode preset | `error_path` matches builtin | `src/custom_agent.rs` `presets_opencode_error_path_matches_builtin` |
| TC-1697 | FR-1683 | unit | opencode error JSONL line | parse line | provider message surfaces | `src/custom_agent.rs` `opencode_error_jsonl_surfaces_provider_message` |
| TC-1698 | FR-1683 | unit | TOML with single `error_path` | parse line | single pointer still works | `src/custom_agent.rs` `single_string_error_path_still_works` |
| TC-1699 | FR-1683 | unit | `/error` object without message | parse line | compact JSON fallback | `src/custom_agent.rs` `error_object_without_message_surfaces_compact_json` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Event log adds optional `error_kind` field; existing records remain valid | Field is optional with serde default | TC-1694 |
| Provider protocol | No protocol change; JSON pointers only affect custom agents | Unknown fields ignored | TC-1697, TC-1699 |
| Security / permissions | Policy refusals remain `policy` kind; no weakening | Existing gate behavior unchanged | TC-1689 |
| Platform | Process-exit and timeout behavior unchanged | Platform-specific process handling preserved | TC-1690 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Structured errors and runner mappings | `src/agent/mod.rs`, `src/agent/*.rs` | FR-1678, FR-1679 | TC-1687 to TC-1693 |
| 2 | `error_kind` in headless JSON and events | `src/cli.rs`, `src/app.rs`, `src/events.rs` | FR-1682 | TC-1694, TC-1695 |
| 3 | Pointer-or-list `error_path` and preset/docs/init | `src/custom_agent.rs`, docs, init | FR-1683 | TC-1696 to TC-1699 |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] `../reviews/REVIEW-typed-errors.md` records commands, result, residual risk, and rollback notes.
