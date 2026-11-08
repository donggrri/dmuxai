# SPEC: JSONL custom-agent text folding and lifecycle-based turn completion (#184)

Status: in progress (PR #265, reworked 2026-11-08)
Owner: dmuxai
Created: 2026-11-07
Baseline: `2b1f4f6`
Related: GitHub #184, SPEC-custom-agent-provider (FR-2442), SPEC-grok-runner (FR-916)

## 1. Decision summary

Custom JSONL agents glued every text chunk into the final reply with no
separator, and could report `ok: true` when the agent stopped mid-work after
tool calls. The first draft of this PR judged "unfinished" from a short reply
after several tools; that rejects legitimate short answers and is removed.

Instead DmuxAI decides whether a turn finished from the runner's own
lifecycle:

- the child process exit status (non-zero is already a failure), and
- the runner's terminal event: an optional `finish_when` matcher selects the
  runner's end-of-step lines, `finish_reason_path` reads the reason on those
  lines, and `finish_ok` lists the reasons that mean "turn complete". When
  `finish_when` is configured and the process exits 0, the turn is finished
  only if the LAST matching line's reason is in `finish_ok`; if no matching
  line was seen, or the last reason is something else (e.g. opencode
  `tool-calls`, `length`, `error`), the turn fails with
  `error_kind = "unfinished"` and a message naming the last reason.
- When `finish_when` is not set, `done_when` (if set) plays the same role:
  the process exiting 0 without ever emitting the `done_when` line is
  `unfinished`. Agents with neither keep today's behaviour.
- Reply length and edit count are never used to decide completion.

`text_mode` (`last` | `concat`) controls folding only. Default `last` keeps
the text block after the last `tool_when` / `text_break_when` boundary;
`concat` keeps all parts joined by a newline. The opencode preset uses
`concat`, `text_break_when` on `tool_use`, and
`finish_when = {path="/type", equals="step_finish"}`,
`finish_reason_path = "/part/reason"`, `finish_ok = ["stop"]` (observed live:
a tool step ends `tool-calls`, the final step ends `stop`). The grok preset
relies on its existing `done_when` (`type=end`).

## 2. Scope

In: `text_mode`, `finish_when` / `finish_reason_path` / `finish_ok`, the
`done_when`-missing rule, `unfinished` error kind, opencode preset, docs.
Out: wording heuristics, reply-length checks, non-JSONL agents, ACP runners
(already use the ACP `stopReason`), runner fallback on `unfinished`.

## 4. Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| FR-2530 | `text_mode` unset/`last`: only text after the last `tool_when`/`text_break_when` boundary is the reply. | TC-2531 |
| FR-2531 | `text_mode = "concat"`: every text part kept, parts joined by `\n` unless a paragraph break is pending. | TC-2530 |
| FR-2532 | With `finish_when` set and exit 0, the turn is finished iff the last matching line's `finish_reason_path` value is in `finish_ok`; otherwise `error_kind = "unfinished"`, headless `ok:false`. A short reply after many tools with reason `stop` succeeds. | TC-2532, TC-2534, TC-2535 |
| FR-2533 | Opencode preset: `concat`, `text_break_when` tool_use, finish settings above. | TC-2533 |
| FR-2534 | Without `finish_when`, a configured `done_when` that never matched before a clean exit gives `unfinished`; agents with neither are unchanged. | TC-2536 |
| FR-2535 | No reply-length / tool-count heuristic decides completion. | TC-2535 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Oracle | Automated location |
| --- | --- | --- | --- | --- |
| TC-2530 | FR-2531 | unit | concat joins with `\n` | `tc_2530_concat_mode_joins_text_parts_with_newlines` |
| TC-2531 | FR-2530 | unit | last keeps final block | `tc_2531_last_mode_keeps_final_text_block_after_tools` |
| TC-2532 | FR-2532 | integration (fake agent) | last step_finish reason `tool-calls` → `unfinished` | `tc_2532_last_finish_reason_not_ok_is_unfinished` |
| TC-2533 | FR-2533 | unit | preset fields | `tc_2533_opencode_preset_finish_settings` |
| TC-2534 | FR-2532 | integration | no finish line at all → `unfinished` | `tc_2534_missing_finish_event_is_unfinished` |
| TC-2535 | FR-2532, FR-2535 | integration | 3 tools then short "OK" with `stop` → ok | `tc_2535_short_final_answer_with_stop_succeeds` |
| TC-2536 | FR-2534 | integration | done_when never seen → `unfinished`; neither set → ok | `tc_2536_done_when_missing_is_unfinished` |
