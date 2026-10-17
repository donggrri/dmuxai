# SPEC: Input slash-command handling and redraw liveness

Status: Implemented (PR #18)
Owner: DmuxAI
Created: 2026-10-17
Baseline: `f0a73e0`
Related: dogfood #15, #17, #19; baseline `FR-002`, `UX-002`

> Note: PR #18 accidentally committed this file empty, losing the original
> text. Reconstructed on 2026-10-17 (SPEC-spec-index work) from
> `docs/TEST-PLAN.md` §1 rows FR-420..425 / TC-420..426, `docs/dogfood`
> #15/#17/#19, and the actual `src/app.rs` test names. Requirement wording is
> faithful to the recorded fix summaries and tests; IDs are unchanged.

## 1. Decision summary

Fast-typed slash input and stale redraws made the TUI feel hung or sent
commands to agents as prompts. Slash parsing now happens at submission time
instead of relying on popup state, unknown commands are rejected instead of
being forwarded, and completion/busy activity forces terminal repaints so
status ages keep advancing while agents work.

## 2. Scope

### In scope

- `//` escape for literal `/` prompts and argument-bearing slash commands.
- Unknown slash commands stay in the input with an error, never sent.
- Fast-typed `/agent <name>` and `/mode <ask|plan|implement>` execute on submit.
- Immediate redraw when event draining observes turn completion.
- A periodic busy tick keeps elapsed/last-event ages advancing.

### Out of scope / non-goals

- New slash commands or changes to existing command semantics.
- Popup-only interactions (typing speed must not matter).
- Provider protocol or persistence changes.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-420 | Must | Given a prompt starting with `//`, when it is submitted. | The prompt is sent to the active agent as a regular prompt with the leading `/` handling escaped, not treated as a slash command. | A literal prompt is never swallowed by command parsing. | TC-420 |
| FR-421 | Must | Given a prompt starting with `/` that is not a known slash command, when it is submitted. | The input is rejected with `Unknown command: /x (type / for the list)` and kept in the input buffer. | Nothing is sent to the agent; the user can edit and resubmit. | TC-421 |
| FR-422 | Must | Given `/agent <name>` or `/mode <ask|plan|implement>` typed faster than the popup opens, when Enter is submitted. | The command executes directly on submission: the agent or mode changes and no prompt is sent to any agent. | Fast typing cannot leak the command text as an agent turn. | TC-422, TC-423 |
| FR-423 | Must | Given a turn completes while event draining is throttled by the render interval, when `drain_agent_events` observes completion. | The terminal redraws immediately so IDLE state and the answer replace the RUNNING status without waiting for another key press. | Completion is never left unpainted behind the throttle. | TC-424 |
| FR-424 | Must | Given any agent is busy without keyboard events, when the timer tick fires. | The tick marks the UI dirty so `working · Ns · last event Ns ago` keeps advancing on screen. | Busy agents do not appear frozen between key presses. | TC-425 |
| FR-425 | Must | Given an agent emits any event, including ephemeral `Tool` or `Thinking` activity items, when the event is applied. | `last event` age resets for every observed agent event, not only stored ones. | Long tool-running turns are not misreported as hung. | TC-426 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-420 | FR-420 | unit | App with an active agent and `//` prompt in input. | Submit the prompt. | The prompt is sent to the agent as a regular prompt. | `src/app.rs`: `escaped_slash_prompt_is_sent_as_regular_prompt` |
| TC-421 | FR-421 | unit | App with an unknown `/x` command in input. | Submit it. | An unknown-command error is shown and the input is kept; nothing is sent. | `src/app.rs`: `unknown_slash_command_is_not_sent_and_keeps_input` |
| TC-422 | FR-422 | unit | Fast-typed `/agent agy` without the popup open. | Submit with Enter. | The selected agent switches without sending a prompt. | `src/app.rs`: `fast_typed_agent_command_switches_agent_without_sending` |
| TC-423 | FR-422 | unit | Fast-typed `/mode ask` without the popup open. | Submit with Enter. | The mode is set without sending a prompt. | `src/app.rs`: `fast_typed_mode_command_sets_mode_without_sending` |
| TC-424 | FR-423 | unit | App draining events that include turn completion. | Drain agent events. | Completion forces an immediate draw. | `src/app.rs`: `drain_agent_events_completion_triggers_immediate_draw` |
| TC-425 | FR-424 | unit | App with a busy agent. | Call the busy tick. | The UI is marked dirty so status ages advance. | `src/app.rs`: `tick_marks_dirty_while_busy` |
| TC-426 | FR-425 | unit | Agent emits a `Tool`/`Thinking` activity event. | Apply the event. | The last-event age resets. | `src/app.rs`: `activity_event_resets_last_event_age` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change. | Revert is code-only. | TC-421 |
| Provider protocol | Unknown or escaped slash input must never reach a provider as a turn. | Rejection is local to the UI. | TC-420, TC-421 |
| Platform | Redraw tick applies to all platforms; observed under tmux (Linux). | Falls back to keypress-driven repaint if the tick is removed. | TC-424, TC-425 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Submission-time slash parsing with `//` escape, unknown-command rejection, argument commands. | `src/app.rs` | FR-420, FR-421, FR-422 | TC-420, TC-421, TC-422, TC-423 |
| 2 | Immediate draw on drained completion and busy-tick dirty marking. | `src/app.rs` | FR-423, FR-424 | TC-424, TC-425 |
| 3 | `observe_activity` before `stored_event` so every event resets the age. | `src/app.rs` | FR-425 | TC-426 |
