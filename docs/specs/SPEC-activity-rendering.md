# SPEC: Activity rendering for tool calls and thinking

Status: Implemented
Owner: DmuxAI
Created: 2026-10-17
Baseline: `73e3f4d`
Related: SPEC-task-progress-visibility, SPEC-speaker-colors, SPEC-custom-agent-provider

## 1. Decision summary

Tool calls and model thinking are rendered concisely in the chat stream instead of dumping raw JSON or creating noisy system messages. Tool calls render as single collapsed lines in muted gray (e.g. `▸ edit src/app.rs (+12 -3)`, `▸ run cargo test ✓ 4.2s`, `✗` on failure), grouping into a single summary line when more than 5 consecutive calls occur, and toggling expanded views via `Ctrl+O` or `/details` (truncated to 40 lines). Model thinking displays as an in-place streaming dim italic preview (`thinking… <last ~80 chars>`) and collapses to `▸ thought for <duration>` on turn completion. Thinking is strictly ephemeral: it is never saved to the conversation database or projected into cross-agent sharing context. UI behavior is configurable via `[ui]` with `show_tools` and `show_thinking`.

## 2. Scope

### In scope

- Rendering tool call progress and completion as a single collapsed line in system/gray color with symbol indicators (`▸`, `✓`, `✗`), diff stats, or execution duration.
- Consecutive tool call grouping when more than 5 calls occur in sequence within a turn (e.g. `▸ 7 tool calls (edit ×3, run ×4)`).
- Interactive expansion and collapse of tool details (arguments and output truncated to 40 lines) via `Ctrl+O` for the last tool group and `/details` slash command for all tool groups.
- Streaming thinking indicator updated in place as dim italic text (`thinking… <last ~80 chars>`), collapsing to `▸ thought for <duration>` upon completion.
- Expansion of collapsed thinking lines to view thought transcripts truncated to 40 lines.
- Configuration settings under `[ui]` in `dmuxai.toml`:
  - `show_tools = "collapsed" | "expanded" | "hidden"` (default: `"collapsed"`)
  - `show_thinking = "live" | "collapsed" | "hidden"` (default: `"live"`)
- Ephemeral lifecycle guarantee: thinking text is never written to SQLite conversation history or included in cross-agent context projection.
- Provider event normalization for Codex, Cursor (`stream-json`), AGY (`step_update` tool and thinking steps), and `[[agents.custom]]` via `tool_name_path`, `tool_status_path`, and `thinking_path` JSON pointers.
- Safe ignoring of unknown provider events without breaking conversation streaming.

### Out of scope / non-goals

- Syntax highlighting or pager navigation for tool outputs beyond 40 lines (output is truncated).
- Interactive tool execution approvals in the chat area (handled by permissions and CLI sandbox policies).
- Modifying SQLite schema or persistent message data structures to record thinking blocks.
- Parsing internal reasoning models for providers that do not emit distinct thinking events or tokens.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-360 | If both diff stats and execution durations are present for a tool call, both are displayed inline (e.g. `▸ edit src/app.rs (+12 -3) ✓ 0.4s`). | DmuxAI | Implementation |
| OQ-361 | Consecutive tool call grouping resets as soon as an agent produces response text or the turn finishes. | DmuxAI | Implementation |

## 3. Terms and state model

### Current state

- In `src/agent/mod.rs`, `AgentEvent` defines `ToolProgress { item_id: String, summary: String, state: String }`. There is no dedicated event for thinking streaming or thought completion.
- In `src/agent/agy.rs`, `normalize_event` only inspects `step_update` where `step_type == "agent_response"` (`text_delta`) and terminal `result` events; all other `step_update` events (such as tool invocations or thinking steps) are ignored in `_ => {}`.
- In `src/agent/cursor.rs`, `normalize_event` only consumes `assistant` text deltas and terminal `result`.
- In `src/app.rs`, `apply_tool_progress` converts incoming tool events into plain `Message` items with `Role::System` containing `{agent} tool [{state}]: {summary}`, cluttering chat history as regular system lines without collapse or grouping.
- `src/custom_agent.rs` only defines `text_path`, `text_when`, `error_path`, and `session_id_path`. Tool and thinking JSON paths are not supported.

### Terminology

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Tool Call | A single invocation of an external action (command execution, file edit, search) by the agent. | Task activity status line in header |
| Tool Group | A sequence of one or more consecutive tool calls within a single turn before text generation. | Multi-agent graph pipeline |
| Thinking Stream | Ephemeral reasoning tokens emitted by a model before or during response generation. | Agent response text (`TextDelta`) |
| Collapsed View | Single-line compact summary of tool progress or completed thoughts. | Hidden activity (`hidden` mode) |
| Expanded View | Detailed multiline block displaying tool input arguments and output truncated to 40 lines. | Full external log file |

### State model

```text
[Tool Call Lifecycle]
  Running (dim/gray "▸ <name> <target>")
    ├─► Success ("▸ <name> <target> ✓ <elapsed>")
    └─► Failed  ("▸ <name> <target> ✗ <elapsed>")

[Thinking Lifecycle]
  Streaming ("thinking… <last ~80 chars>" [dim italic, in place])
    └─► Finished ("▸ thought for <duration>" [collapsed, expandable])

[UI Toggle State]
  Collapsed (default) <──[Ctrl+O / /details]──> Expanded (args + output ≤40 lines)
```

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-360 | Must | Agent emits tool call progress or completion during a turn. | Renders as ONE collapsed line in chat in system/gray color (e.g. `▸ edit src/app.rs (+12 -3)`, `▸ run cargo test ✓ 4.2s`), never displaying raw JSON. Tool failures display `✗`. | If diff stats or duration are unavailable, renders tool name and target summary without blank gaps. | TC-360, TC-361 |
| FR-361 | Must | More than 5 consecutive tool calls occur in a single turn without intervening text. | Grouped into a single collapsed line summary, formatted as `▸ <N> tool calls (<tool1> ×<count1>, <tool2> ×<count2>)` (e.g. `▸ 7 tool calls (edit ×3, run ×4)`). | Exactly 5 or fewer consecutive tool calls remain displayed as individual single lines. | TC-362 |
| FR-362 | Must | Agent emits thinking chunks while generating a turn. | While streaming, displays a single dim italic line `thinking… <last ~80 chars>` updated in place; upon completion collapses to `▸ thought for <duration>` (e.g. `▸ thought for 12s`). | If thinking duration is less than 1 second, displays `▸ thought for 0s`. Empty thinking events produce no line. | TC-363, TC-364 |
| FR-363 | Must | Turn finishes, conversation is saved to database, or context is projected for another agent. | Thinking text and thinking markers are strictly ephemeral: they MUST NEVER be written to the SQLite conversation DB message text or included in cross-agent context sharing. | If provider emits thinking interleaved with response text, thinking tokens are filtered out from stored messages. | TC-364 |
| FR-364 | Must | Application loads `[ui]` configuration from `dmuxai.toml`. | Parses `show_tools = "collapsed" | "expanded" | "hidden"` (default `"collapsed"`) and `show_thinking = "live" | "collapsed" | "hidden"` (default `"live"`). `"hidden"` suppresses rendering; `"expanded"` renders tool details without manual toggle. | Invalid values log a warning and fall back to `"collapsed"` and `"live"`. Missing `[ui]` section uses defaults. | TC-365 |
| FR-365 | Must | Incoming NDJSON events received from Codex, Cursor `stream-json`, AGY `step_update`, or `[[agents.custom]]`. | Normalizes tool calls and thinking into standard activity events; custom agents map `tool_name_path`, `tool_status_path`, and `thinking_path`. Unrecognized events are silently ignored. | Malformed JSON lines or unmapped event types do not crash the adapter or abort the turn. | TC-366, TC-367 |
| UX-360 | Should | User presses `Ctrl+O` while viewing chat. | Toggles expanded/collapsed view of the last tool group in the active conversation, displaying tool arguments and output truncated to at most 40 lines. | If no tool group exists in the conversation, pressing `Ctrl+O` is a no-op. | TC-361 |
| UX-361 | Should | User inputs `/details` slash command. | Toggles expanded/collapsed state for all tool groups in the active conversation, displaying arguments and output truncated to 40 lines. | Repeated `/details` restores previous collapsed states; viewport preserves scroll position. | TC-361 |
| UX-362 | Should | User selects or toggles a collapsed `▸ thought for <duration>` line. | Expands to display the thought text transcript, formatted in dim/gray styling and truncated to 40 lines. | Collapsing returns to the single summary line. | TC-363 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-360 | FR-360 | unit | TestBackend with tool events (file edit with diff stats, command run with duration, failed command). | Render chat buffer. | Tool calls render as single lines in gray/system color with `▸`, `✓`, `✗` symbols; no raw JSON tokens visible. | `cargo test tool_call_collapsed_rendering` (`src/app.rs`) |
| TC-361 | FR-360, UX-360, UX-361 | unit | Rendered chat containing tool group with multi-line args and 60-line stdout output. | Trigger `Ctrl+O`, then `/details`. | Last tool group expands showing args and output truncated to 40 lines; `/details` expands all groups; toggling again collapses. | `cargo test tool_call_expand_collapse_and_truncation` (`src/app.rs`) |
| TC-362 | FR-361 | unit | Agent turn emitting 7 consecutive tool calls (3 edit calls, 4 run calls). | Render chat buffer. | Consecutive calls collapse to `▸ 7 tool calls (edit ×3, run ×4)`. | `cargo test tool_calls_grouped_when_exceeding_threshold` (`src/activity.rs`) |
| TC-363 | FR-362, UX-362 | unit | Agent emitting streaming thinking tokens, followed by completion event after 12 seconds. | Render chat buffer during stream, then after completion. | During stream, shows single dim italic `thinking… <last ~80 chars>` updated in place; upon completion collapses to `▸ thought for 12s`; toggling expands thought text. | `cargo test thinking_live_stream_and_collapsed_summary` (`src/activity.rs`) |
| TC-364 | FR-362, FR-363 | integration | Completed turn containing thinking stream, tool calls, and final response text. | Persist turn to SQLite DB and generate cross-agent context projection. | DB messages table and cross-agent context contain only final response text; thinking tokens are completely absent. | `cargo test thinking_never_persisted_to_db_or_shared_context` (`src/app.rs`) |
| TC-365 | FR-364 | unit | Configuration parser given TOML strings with valid, missing, and invalid `[ui]` settings. | Parse `dmuxai.toml` into settings struct. | Defaults to `show_tools = "collapsed"`, `show_thinking = "live"`; properly parses `"expanded"` and `"hidden"`; invalid values fall back to defaults. | `cargo test ui_config_activity_rendering_parsing` (`src/settings.rs`) |
| TC-366 | FR-365 | unit | NDJSON fixture lines from Codex tool items, Cursor `stream-json`, and AGY `step_update` tool/thinking steps. | Pass fixtures through adapter event normalization loops. | Emits normalized tool progress and thinking events; unrecognized event types are ignored without error. | `cargo test provider_activity_event_normalization` (`src/activity.rs`, `src/agent/agy.rs`) |
| TC-367 | FR-365 | unit | Custom agent spec with `tool_name_path`, `tool_status_path`, and `thinking_path` parsing sample JSONL lines. | Parse lines with `parse_jsonl_line`. | Extracts tool name, status, and thinking delta; unknown keys are ignored. | `cargo test custom_agent_activity_jsonl_paths` (`src/custom_agent.rs`) |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Thinking tokens are NEVER stored in SQLite `messages` or `turn_events` tables. Stored transcript and shared context projections remain strictly thinking-free. | Zero database schema change. Fully backward and forward compatible. | TC-364 |
| Provider protocol | Codex tool events, Cursor `stream-json` tool blocks, AGY `step_update` (`tool_call`, `thinking`), and custom agent JSONL paths (`tool_name_path`, `tool_status_path`, `thinking_path`) map into standard internal events. Unknown events are dropped. | No provider protocol breaking changes; existing adapters remain functional if new events are absent. | TC-366, TC-367 |
| Security / permissions | Rendered tool details (commands, file paths, diffs) must respect terminal escape sanitization. Raw ANSI escape injections from tool output are stripped before rendering. | Safe rendering in all standard terminal backends. | TC-361 |
| Platform | Keybinding `Ctrl+O` and `/details` slash command work across the Linux TUI and WSL environments. Line truncation handles standard UTF-8 and narrow terminal widths gracefully. | Text clipping uses terminal width without wrapping breaks. | TC-360, TC-361 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add `AgentEvent::ThinkingDelta`, `AgentEvent::ThinkingCompleted`, and tool detail fields to `AgentEvent::ToolProgress`. | `src/agent/mod.rs` | FR-360, FR-362 | `cargo test` |
| 2 | Add `show_tools` and `show_thinking` enums and parsing under `[ui]` in configuration. | `src/settings.rs` | FR-364 | TC-365 |
| 3 | Update `[[agents.custom]]` spec to parse `tool_name_path`, `tool_status_path`, and `thinking_path`. | `src/custom_agent.rs`, `src/agent/custom.rs` | FR-365 | TC-367 |
| 4 | Map tool calls and thinking in AGY (`step_update`), Cursor (`stream-json`), and Codex adapters. | `src/agent/agy.rs`, `src/agent/cursor.rs`, `src/agent/codex.rs` | FR-365 | TC-366 |
| 5 | Implement activity line rendering (collapsed tool lines, >5 grouping, in-place live thinking, expansion state) and truncation. | `src/app.rs` | FR-360, FR-361, FR-362, UX-362 | TC-360, TC-362, TC-363 |
| 6 | Implement `Ctrl+O` shortcut and `/details` slash command toggle. | `src/app.rs` | UX-360, UX-361 | TC-361 |
| 7 | Ensure thinking events bypass persistence and context sharing. | `src/app.rs`, `src/agent/mod.rs` | FR-363 | TC-364 |

## 8. Review and test exit criteria

- [ ] All Must requirements (FR-360 through FR-365) map to passing test evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] External-provider smoke tests are marked run, skipped, or blocked with a reason; skipped is never reported as passing.
- [ ] `docs/reviews/REVIEW-activity-rendering.md` records commands, result, residual risk, and rollback notes.
