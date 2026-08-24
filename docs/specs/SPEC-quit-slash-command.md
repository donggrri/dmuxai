# SPEC: Slash-command quit

Status: approved  
Owner: DmuxAI  
Created: 2026-08-24  
Baseline: `52dde59`  
Related: baseline UX-011, UX-012

## 1. Decision summary

Replace the application's keyboard-only quit action with the `/quit` slash
command. Submitting exactly `/quit`, or selecting it from the slash preview,
sets the existing application exit state and therefore uses the normal child
process shutdown and terminal restoration path. `Esc` remains available for
dismissing an open overlay or slash preview, but no longer exits the
application when the main prompt is active.

## 2. Scope

### In scope

- Registering `/quit` in the supported slash-command catalog and preview.
- Executing `/quit` through Enter or Tab from the preview and through exact
  prompt submission.
- Removing the main-prompt `Esc` exit action while preserving overlay and
  preview dismissal.
- Updating user-facing controls and the product baseline.

### Out of scope / non-goals

- Changing `Ctrl+C` cancellation of the selected agent's active turn.
- Changing how provider child processes are stopped or how the terminal is
  restored during normal shutdown.
- Adding command arguments, aliases, confirmation prompts, or a new shutdown
  mechanism.
- Changing historical completed review artifacts.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | `Esc` should continue closing an open picker or slash preview because that behavior is independent of quitting. | DmuxAI | Execute |

No open questions block implementation.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| `/quit` | The exact supported slash command that requests application exit. | A prompt beginning with `/quit `, which remains ordinary prompt text. |
| Slash preview | The list of commands matching the current no-whitespace `/...` input. | A provider prompt submitted after whitespace is entered. |
| Exit state | The existing application state that ends the event loop and allows normal shutdown to run. | Immediate process termination that could skip cleanup. |

The application remains running until `/quit` is executed. An open overlay or
slash preview may still transition to its closed state on `Esc`; this does not
change the application exit state.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| UX-011 | Must | The main prompt is active and the user submits exactly `/quit`, or selects `/quit` in the slash preview and presses Enter or Tab. | The input is cleared, the application enters its existing exit state, and the normal shutdown path is used. No provider turn is started for the command. | `/quit` followed by whitespace is ordinary prompt text and is not treated as a quit command. | TC-019, TC-020, TC-022 |
| UX-012 | Must | The user presses `Esc` while the main prompt, a slash preview, or an overlay is active. | `Esc` closes the active slash preview or overlay; with no dismissible UI it leaves the application running and preserves the prompt input. | `Esc` never changes the exit state. Existing picker-specific dismissal behavior remains intact. | TC-021 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-019 | UX-011 | unit | A test app has an idle main prompt and no overlay. | Set input to `/quit` and submit it. | Input is empty, `exit` is true, and no adapter request was sent. | `cargo test exact_quit_command_exits_without_starting_a_turn` |
| TC-020 | UX-011 | unit | A test app has slash preview active after typing `/q`. | Press Enter or Tab on the highlighted `/quit` command. | The preview closes, input is empty, and `exit` is true without an adapter request. | `cargo test slash_preview_can_execute_quit_with_enter`; `cargo test slash_preview_can_execute_quit_with_tab` |
| TC-021 | UX-012 | unit | A test app is running; repeat with a slash preview and a conversation picker. | Press `Esc` in each state. | Preview/picker dismisses where applicable, but `exit` remains false; a bare `Esc` preserves the prompt and also leaves `exit` false. | `cargo test escape_never_exits_the_application` |
| TC-022 | UX-011 | boundary | A test app has an idle selected adapter. | Set input to `/quit ` and submit it. | The exact text is sent as an ordinary provider prompt and `exit` remains false. | `cargo test quit_with_whitespace_is_regular_prompt_text` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema or canonical transcript change. | Reverting the feature restores `Esc` as the main-prompt exit action; existing stored conversations are unaffected. | TC-019, TC-021 |
| Provider protocol | `/quit` never becomes a provider request; whitespace-bearing `/quit ` retains existing prompt routing. | No provider protocol changes or live credentials are required. | TC-019, TC-022 |
| Security / permissions | Existing shutdown cleanup and provider ownership boundaries remain unchanged. | The command only selects the existing exit path and cannot grant provider capabilities. | TC-019 |
| Platform | Normal terminal restoration remains owned by the existing shutdown/drop path on Windows and Linux. | No platform-specific command or fallback is introduced. | Manual smoke check in review |

## 7. Implementation plan

See [PLAN-quit-slash-command.md](../plans/PLAN-quit-slash-command.md) and
[TASKS-quit-slash-command.md](../tasks/TASKS-quit-slash-command.md).

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] The diff is reviewed against the `/quit` scope and non-goals.
- [x] The spec traceability check and required Rust format, test, and clippy
  gates pass.
- [x] External-provider smoke tests are marked skipped with a reason unless
  explicitly run with authenticated CLIs.
- [x] `../reviews/REVIEW-quit-slash-command.md` records commands, results, residual risk,
  and rollback notes.
