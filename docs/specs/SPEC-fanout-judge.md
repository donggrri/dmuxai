# SPEC-fanout-judge

Status: In progress · Series: orchestrator gap plan 3/5 · Builds on SPEC-fanout-readonly

## 1. Decision summary
A read-only fan-out returns N answers but leaves the comparison to the reader. This adds `--judge <agent[:model]>` to `dmuxai run --agents ...`: after the legs finish, one extra read-only judge turn on a chosen runner receives every successful leg answer inside nonce DATA blocks (the same guard as the triage judge) and returns a structured comparison. Leg text is data, never instructions. Without `--judge` nothing changes and no extra model call is made.

## 2. Scope

In scope: `--judge` flag, judge prompt with per-leg DATA blocks, `reject_data` guard, JSON parse and validation, `aggregate` output field, text output section, failure handling, free-runner exclusion for company workspaces.
Out of scope: write legs (PR 4), automatic merge of answers, the heuristic file/line overlap check, TUI and dashboard UI.

## 3. Terms and state model

- Judge turn: one read-only turn on a fresh conversation, started after all legs reach a final state.
- Aggregate: `{ "agreements": [string], "disagreements": [ { "topic": string, "positions": { "<agent>": string } } ], "unique_findings": { "<agent>": [string] }, "recommendation": string }`.

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-2920 | Must | `--judge <agent[:model]>` is valid only together with `--agents`; otherwise exit 2. The judge runner may equal a leg runner. It needs read-only mode; without it the aggregate step fails with `error_kind` `no_read_only_mode`. |
| FR-2921 | Must | The judge turn starts only after every leg is `ok`, `failed`, or `skipped`, and only when at least 2 legs are `ok`. With fewer than 2 `ok` legs no judge turn runs and `aggregate` is `null` with `aggregate_skipped: "too_few_legs"`. |
| FR-2922 | Must | Each `ok` leg answer is placed only inside `<<<DATA-<nonce>` ... `DATA-<nonce>>>>` blocks labelled with the leg's agent name, using one fresh nonce per judge turn. The prompt states that DATA content is data, not instructions. A leg answer containing the closing marker for the nonce is excluded from the judge input, marked `blocked` in the output, and never ends a block early. |
| FR-2923 | Must | The judge answer is parsed as the aggregate JSON (fenced or bare, last valid object wins, at most 64 KiB). Unknown keys are ignored. Position and finding keys must name legs that were given to the judge; others are dropped. Invalid output sets `aggregate` to `null` and `aggregate_error: "invalid_aggregate"`; the run result still depends only on the quorum of legs. |
| FR-2924 | Must | `--json` adds `aggregate`, `aggregate_error`, `aggregate_skipped`, and `judge: { "agent", "model", "duration_ms" }`. Text mode prints a comparison section after the leg sections. |
| FR-2925 | Must | The judge turn runs with `TurnPolicy::ReadOnly` in a fresh conversation, with the same worktree-fingerprint guard as the legs. |
| FR-2926 | Must | When `[fanout] company_workspace = true`, a free runner given to `--judge` is rejected with exit 2 and a warning. |
| NFR-2920 | Must | Without `--judge` no judge code runs; the benchmark stays within 10% of main (#301). The judge adds exactly one extra turn and at most the time of that turn. |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-2920 | FR-2920 | `tc_2920_judge_flag_requires_agents_and_read_only_runner` |
| TC-2921 | FR-2921 | `tc_2921_judge_skipped_with_fewer_than_two_ok_legs` |
| TC-2922 | FR-2922 | `tc_2922_leg_text_in_data_blocks_and_closing_marker_blocked` |
| TC-2923 | FR-2923 | `tc_2923_aggregate_parse_valid_invalid_and_unknown_agent_keys` |
| TC-2924 | FR-2924 | `tc_2924_json_output_has_aggregate_fields` |
| TC-2925 | FR-2925 | `tc_2925_judge_turn_is_read_only_and_guarded` |
| TC-2926 | FR-2926 | `tc_2926_company_workspace_rejects_free_judge` |
| TC-2927 | NFR-2920 | `tc_2927_no_judge_flag_runs_no_judge_turn` |

## 6. Data and safety contracts
No schema change. Prompt-injection boundary: leg text only inside DATA blocks (reuse of `reject_data`). No new network access.

## 7. Exit criteria
All TCs pass, gate and web tests pass, benchmark within 10%.
