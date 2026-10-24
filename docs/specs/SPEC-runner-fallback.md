# SPEC-runner-fallback

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `7e00ff8`
Related: #151, PR-10

## 1. Decision summary

When a runner fails for provider reasons (quota, auth, transient network error, a runner that could not start) or stalls, DmuxAI may hand the same turn to the next runner/model in an ordered per-runner chain configured under `[runners.<id>]` / `[runners.default]`. A failed turn is recorded with a Notice and Failed state, a `fallback` event is emitted, and the next target receives the original prompt with a preface telling it to check `git status` and `git diff` first. Empty or missing chains preserve the old behaviour. Stall detection runs inside the agy and custom command adapters only when a chain exists. AGY also keeps results after a transient provider error reported after the final answer step (#151), no longer passes `--sandbox` on agent turns, and adds a short efficiency rule block to its agent prompt.

## 2. Scope

### In scope

- Per-runner `fallback`, `stall_timeout_secs` and `stall_api_retries` config keys.
- `classify_failure` trigger mapping and non-triggers.
- Stall detection in agy and custom command adapters.
- Handover across chat turns, graph worker nodes, and headless `dmuxai run` turns.
- Headless `run --json` `fallbacks` array.
- AGY #151 recovery, `--sandbox` removal on agent turns, efficiency rules.
- Custom agent #151 recovery and `stall_keepalive`.

### Out of scope / non-goals

- Fallback for internal turns (planner, judge, triage, replan, summary).
- Stall detection for ACP runners (codex/cursor/grok acp).
- Treating a silent/idle process as a fallback trigger.
- Using a fallback runner's own chain.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2100 | Internal turns do not fall back; only chat, graph worker nodes, and headless run turns do. | dmuxai | PR-10 |
| OQ-2101 | ACP runners have no stall detection; only agy and custom command adapters implement it. | dmuxai | PR-10 |
| OQ-2102 | Idle (silent process) is not a fallback trigger; it remains a normal failure. | dmuxai | PR-10 |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Fallback target | An ordered `"<runner>"` or `"<runner>:<model>"` entry in a chain. | The current runner. |
| Chain | The ordered list of fallback targets for one runner, after validation and self-exclusion. | A runner's own chain used when it is the fallback. |
| Stall | No model progress for `stall_timeout_secs`, or `stall_api_retries` consecutive API retry lines. | Idle (no stdout at all). |
| Handover | Sending the same prompt to the next target with a preface. | Retry of the same runner. |
| Exhausted | No usable target remains in the chain. | A cancelled turn. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2100 | Must | Given a fallback target string | Parse into runner id (lowercased, trimmed) and optional model (trimmed); reject empty runner or model-only strings; Display round-trips | Empty or `:model` strings error | TC-2100 |
| FR-2101 | Must | Given `[runners.<id>]` / `[runners.default]` tables | Load `fallback`, `stall_timeout_secs` (default 240), `stall_api_retries` (default 3); empty `fallback = []` disables the default chain for that runner; `config_for` prefers a runner entry over default; workspace tables replace the user entry per runner id (and default when present) | Missing chain keeps old behaviour | TC-2101 |
| FR-2102 | Must | Given invalid fallback targets | Drop unknown runners, disallowed models, unparseable entries and duplicates with a config warning naming runner and reason; keep valid targets | Empty chain results when all targets invalid | TC-2102 |
| FR-2103 | Must | Given a runner's chain and its current model | Exclude any target with the same runner and same model; a bare runner target is always excluded for that runner; another model of the same runner is kept | Self-fallback with same model never occurs | TC-2103 |
| FR-2104 | Must | Given an error kind and message | `classify_failure` returns a fallback reason for `stalled`; `model_unavailable` (quota vs unavailable); provider/process_exit messages matching quota/429/rate limit, auth expired/401/403/not logged in, transient provider/network errors, or could-not-start | Unmatched messages return None | TC-2104 |
| FR-2105 | Must | Given a non-trigger error | `classify_failure` returns None for cancelled, policy, timeout, idle, and normal task failures (tests failing, empty reply, plan-only, refusal) | No fallback attempted | TC-2105 |
| FR-2106 | Must | Given a runner with a non-empty chain | Stall detection runs in agy and custom adapters: no model progress for `stall_timeout_secs` (0 disables) triggers; keepalive lines do not reset the timer; a running tool pauses the timer until it finishes; `stall_api_retries` consecutive API retry lines (0 disables) trigger | No detection without a chain | TC-2106 |
| FR-2107 | Must | Given a runner's stall settings | `stall_policy_for` returns None when the runner has no chain or both stall settings are 0 | Disabled timers are None | TC-2107 |
| FR-2108 | Must | Given a FallbackChain | Yield usable targets in configured order exactly once per user turn; the fallback runner's own chain is not used | Exhausted chain yields None | TC-2108 |
| FR-2109 | Must | Given a handover | Emit the exact preface with from runner, reason, detail (truncated to 200 chars) and the instruction to check `git status` and `git diff` first | Detail trimmed | TC-2109 |
| FR-2110 | Must | Given a FallbackRecord | `summary()` formats the system line with from runner/model, reason, detail and either the next target or "fallback chain exhausted" | Matches expected text | TC-2110 |
| FR-2111 | Must | Given an AGY turn ending in a transient provider error after the final answer step | Complete with a Notice "...the result was kept (#151)" instead of failing; mid-turn or non-transient errors still fail | Result kept | TC-2111 |
| FR-2112 | Must | Given a custom agent turn with a transient provider error after final text | Complete with a Notice "...(#151)"; mid-turn or non-transient errors still fail | Warning only at final | TC-2112 |
| FR-2113 | Must | Given an AGY agent turn | Do not pass `--sandbox`; read-only turns keep `--mode plan --sandbox` | Agent turns can run cargo fmt/test | TC-2113 |
| FR-2114 | Must | Given an AGY agent turn | Include the "Work efficiently" rule block (no manage_task, read 200+ lines at once, batch rg, run only cargo check and changed tests; leave fmt/clippy/full suite to caller) | Prompt contains rules | TC-2114 |
| FR-2115 | Must | Given stall timers running | Supervisor `next_line_with_tick` keeps partial lines across ticks | No lost output | TC-2115 |
| FR-2116 | Must | Given a custom agent with `stall_keepalive` | Lines matching the regex are keepalives only; stall triggers on no progress or API retry lines | Keepalive does not reset timer | TC-2116 |
| FR-2117 | Must | Given an AGY turn with a tool running | Stall timer pauses while the tool is ACTIVE and fires after it finishes; no stall detection without a chain | Tool pause observed | TC-2117 |
| FR-2118 | Must | Given a failed chat, graph worker, or headless run turn | Record Notice + Failed ("handed over to ..."), show the system line in TUI/web/headless messages, emit an events.jsonl `kind: "fallback"` event, send the same prompt to the next usable target with the preface; skip busy or disallowed targets; one pass through the original chain; exhausted chain fails with "fallback chain exhausted"; internal turns do not fall back | Cancel and normal failures do not hand over | TC-2118 |
| FR-2119 | Must | Given handover in headless or graph runs | `run --json` includes an optional `fallbacks` array with `{from_runner, from_model, reason, detail, to_runner, to_model}` (`to_*` null when exhausted); `ok`/`exit_code` reflect the final runner; graph nodes rebind to the fallback runner; same-runner different-model fallback works | Omitted when empty | TC-2119 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2100 | FR-2100 | unit | Target strings including runner, runner:model, whitespace | Parse each | Parsed runner/model and errors as specified | `src/runner_fallback.rs` `tc_2100_target_parse` |
| TC-2101 | FR-2101 | unit | User and workspace tables with default and runner entries | Load settings and resolve chains | Runner entry overrides default; workspace replaces user entry; empty chain disables default | `src/runner_fallback.rs` `tc_2101_config_for_prefers_runner_entry_over_default`; `src/settings.rs` `tc_2101_settings_parse_fallback_tables` |
| TC-2102 | FR-2102 | unit | Chain with invalid model, unknown runner, duplicate | Load settings | Invalid targets dropped with warnings; valid target remains | `src/runner_fallback.rs` `tc_2102_invalid_targets_warn_and_skip`; `src/settings.rs` `tc_2102_settings_fallback_rejects_disallowed_model` |
| TC-2103 | FR-2103 | unit | Chain containing self and another model | Resolve chain for current model | Self same model excluded; other model kept | `src/runner_fallback.rs` `tc_2103_chain_excludes_self` |
| TC-2104 | FR-2104 | unit | Error kinds and messages | Call classify_failure | Expected reasons for triggers | `src/runner_fallback.rs` `tc_2104_classify_triggers` |
| TC-2105 | FR-2105 | unit | Non-trigger errors | Call classify_failure | Returns None | `src/runner_fallback.rs` `tc_2105_no_fallback_on_normal_failure` |
| TC-2106 | FR-2106 | unit | StallPolicy with timeout/retries | Drive StallDetector with keepalives, progress, tools, retries | Stall fires only as specified | `src/runner_fallback.rs` `tc_2106_stall_detection_with_keepalive` |
| TC-2107 | FR-2107 | unit | Chains empty/present with zero settings | Call stall_policy_for | None only when no chain or both zero | `src/runner_fallback.rs` `tc_2107_stall_policy_only_with_chain` |
| TC-2108 | FR-2108 | unit | Three-target chain | next_target skipping one | Order and exhaustion as specified | `src/runner_fallback.rs` `tc_2108_chain_order_single_pass` |
| TC-2109 | FR-2109 | unit | Handover inputs | Call handover_preface | Exact preface text | `src/runner_fallback.rs` `tc_2109_preface_text` |
| TC-2110 | FR-2110 | unit | FallbackRecord variants | Call summary | Expected system lines | `src/runner_fallback.rs` `tc_2110_record_summary` |
| TC-2111 | FR-2111 | integration | AGY fixture NDJSON with final answer then transient error | Run turn | Completes with #151 Notice; mid/non-transient errors fail | `src/agent/agy.rs` `tc_2111_agy_recovered_api_error_keeps_result`, `tc_2111_agy_non_transient_error_still_fails`, `tc_2111_agy_mid_turn_api_error_still_fails` |
| TC-2112 | FR-2112 | integration | Custom agent output with final text then transient error | Run turn | Completes with #151 Notice; mid/non-transient errors fail | `src/agent/custom.rs` `tc_2112_custom_final_text_then_transient_error_is_warning`, `tc_2112_custom_non_transient_error_still_fails`, `tc_2112_custom_mid_turn_transient_error_still_fails` |
| TC-2113 | FR-2113 | unit | AGY command config | configure_command for Agent/ReadOnly | Agent has no --sandbox; ReadOnly keeps it | `src/agent/agy.rs` `tc_2113_agy_agent_mode_has_no_sandbox` |
| TC-2114 | FR-2114 | unit | AGY prompt builder | Build agent prompt | Contains efficiency rules | `src/agent/agy.rs` `tc_2114_agy_agent_prompt_has_efficiency_rules` |
| TC-2115 | FR-2115 | unit | Partial line across ticks | next_line_with_tick | Partial line preserved | `src/process/supervisor.rs` `tc_2115_next_line_with_tick_keeps_partial_line` |
| TC-2116 | FR-2116 | integration | Custom agent with keepalive regex and retry lines | Run turn | Stall fires as specified | `src/agent/custom.rs` `tc_2116_custom_stall_with_keepalive_output`, `tc_2116_custom_api_retry_lines_stall` |
| TC-2117 | FR-2117 | integration | AGY turn with tool running and no chain | Run turn | Tool pauses stall; no detection without chain | `src/agent/agy.rs` `tc_2117_agy_running_tool_pauses_stall`, `tc_2117_agy_stall_fires_after_tool_finished`, `tc_2117_no_stall_without_fallback_chain` |
| TC-2118 | FR-2118 | integration | App with fallback chain and failing runner | Submit prompt | Handover, events, system line, cancel/normal no handover | `src/app.rs` `tc_2118_provider_error_hands_turn_to_next_runner`, `tc_2118_stall_error_hands_over`, `tc_2118_normal_failure_does_not_fall_back`, `tc_2118_cancel_does_not_fall_back`, `tc_2118_drain_does_not_mark_handed_over_error` (App wiring in progress) |
| TC-2119 | FR-2119 | integration | Headless/graph runs with fallback | Run turn | fallbacks array, graph rebind, same-runner model | `src/app.rs` `tc_2119_chain_single_pass`, `tc_2119_same_runner_model_fallback`, `tc_2119_graph_node_rebinds_to_fallback_runner`, `tc_2119_headless_outcome_serializes_fallbacks` (App wiring in progress) |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Config | New optional keys under `[runners.<id>]`/`[runners.default]` | Empty/missing chain keeps old behaviour | TC-2101, TC-2102 |
| Fallback chain | Original runner's chain only, one pass | Fallback runner's own chain ignored | TC-2108, TC-2119 |
| Stall detection | Only agy/custom command adapters, only with chain | ACP runners unaffected | TC-2106, TC-2107, TC-2116, TC-2117 |
| Events | New `kind: "fallback"` event with to_state runner or exhausted | Readers ignore unknown kinds | TC-2118 |
| Headless JSON | Optional `fallbacks` array | Omitted when empty; ok/exit_code final | TC-2119 |
| AGY | No `--sandbox` on agent turns; efficiency rules; #151 recovery | Read-only turns unchanged | TC-2111, TC-2113, TC-2114 |
| Custom | #151 recovery; `stall_keepalive` | Mid-turn errors still fail | TC-2112, TC-2116 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Fallback target parsing and settings | `src/runner_fallback.rs`, `src/settings.rs` | FR-2100..FR-2103 | TC-2100..TC-2103 |
| 2 | Trigger classification | `src/runner_fallback.rs` | FR-2104, FR-2105 | TC-2104, TC-2105 |
| 3 | Stall detection | `src/runner_fallback.rs`, `src/agent/agy.rs`, `src/agent/custom.rs`, `src/process/supervisor.rs` | FR-2106, FR-2107, FR-2115..FR-2117 | TC-2106, TC-2107, TC-2115..TC-2117 |
| 4 | Handover and headless output | `src/app.rs`, `src/cli.rs` | FR-2108..FR-2110, FR-2118, FR-2119 | TC-2108..TC-2110, TC-2118, TC-2119 |
| 5 | AGY and custom fixes | `src/agent/agy.rs`, `src/agent/custom.rs` | FR-2111..FR-2114 | TC-2111..TC-2114 |
| 6 | Specification document | `docs/specs/SPEC-runner-fallback.md` | FR-2100..FR-2119 | `python3 scripts/check-spec.py --all` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-2100..FR-2119) map to passing test evidence (TC-2100..TC-2119), except TC-2118/TC-2119 whose App-wiring tests are in progress.
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [ ] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
