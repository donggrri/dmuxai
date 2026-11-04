# REVIEW: progress tool events and heartbeats (#166)

Spec: [SPEC-agent-api](../specs/SPEC-agent-api.md) FR-2430..FR-2433.

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-2430 | Met | Custom JSONL mapping + opencode defaults; `tc_2430_opencode_tool_lines_emit_tool_progress`, `tc_2430_cli_progress_emits_tool_line_for_jsonl_runner` |
| FR-2431 | Met | Workspace-relative edit path; `tc_2431_edit_tool_emits_file_edit_with_relative_path` |
| FR-2432 | Met | `ProgressTracker` heartbeat, headless-only emission; `tc_2432_heartbeat_every_interval_with_last_output_age` |
| FR-2433 | Met | Safe input-field whitelist and 160-char sanitization; `tc_2433_progress_summaries_never_include_content` |

## Regression notes

- `AgentEvent::OutputLine` is metadata only: it is not stored in the transcript,
  not written to the event log, and does not reset adapter idle/stall timers.
- Heartbeats are emitted only through the `--progress` sink in `run_headless`;
  interactive sessions are unchanged.
- No persistent data or provider protocol changes.

## Residual risk

- Built-in Cursor/Codex/ACP runners do not expose a raw JSONL type, so their
  heartbeats omit the `(type=...)` suffix; ages still work from their events.
- Tool title/input heuristics are provider-specific; unknown shapes emit nothing.
