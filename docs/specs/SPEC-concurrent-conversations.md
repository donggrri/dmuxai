# SPEC-concurrent-conversations

Status: Implemented

## Decision summary
In one `dmuxai --web` (or `serve`) process the user can start a turn in conversation A, open
conversation B from the Sessions sidebar and start another turn there while A keeps running.
They can switch back and forth; each conversation shows its own live progress and result, and
the Sessions list marks every conversation with a running turn.

Measured before this change (v0.2.2 + main `c263506`): while grok ran in A, a prompt sent in B
was **queued process-wide**, even to an idle agent (cursor). The queued B prompts never started,
even after A finished, and `run_active` stayed `true`; from then on every prompt in every
conversation was queued forever until restart.

## Scope
In scope: dashboard prompt submission (`/api/prompt`, chat box), the follow-up queue,
conversation switching (`load_conversation`, `new_conversation`), Sessions `running` flags,
background turn output.
Out of scope: TUI layout; graph runs (a graph run still blocks other sends, unchanged);
running the **same agent** in two conversations at once (one runner process per agent today —
see FR-1623).

## Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-1620 | Must | A prompt for agent X in conversation B starts immediately when X has no running turn, even while another agent runs a turn in conversation A. It is not queued and A keeps running. |
| FR-1621 | Must | Switching conversations (`load_conversation`, `new_conversation`) while a turn runs does not cancel, reset or detach it. Its output keeps being stored under its own conversation and never appears in another conversation's transcript. |
| FR-1622 | Must | Switching back to a conversation with a running turn shows its live progress (streaming text, activity) and then its final result; a turn that finished in the background shows its final result. |
| FR-1623 | Must | At most one running turn per (conversation, agent). A second prompt into the same conversation while that conversation has a run keeps the existing follow-up behaviour (queued on a writable dashboard, `409 busy` on a read-only one). A prompt for agent X while X runs in a *different* conversation is refused with `409` and the message `agent 'X' is running in another conversation (<title>); wait or pick another agent`. |
| FR-1624 | Must | Each Sessions sidebar entry has `running: true` exactly while that conversation has a running turn; the sidebar spinner follows it. `session.run_active` reflects only the displayed conversation. |
| FR-1625 | Must | When a background turn ends (completed, cancelled, error) its agent becomes idle and the run state is cleared, so later prompts in any conversation are not queued forever. Follow-ups queued for a conversation start when that conversation's run ends, whether or not it is displayed. |

## Acceptance
| TC | Covers | Test |
|---|---|---|
| TC-1620 | FR-1620 | Automated: `concurrent_send_other_conversation_other_agent_starts`. Manual (dashboard): start a long grok turn in A, open B, send to cursor. Expected: B reply arrives while A still streams; `queued: false`. |
| TC-1621 | FR-1621 | Automated: `switch_conversation_keeps_background_turn`. Manual: during A's grok turn switch to B and back. Expected: A finishes with full output; B transcript has no A text. |
| TC-1622 | FR-1622 | Manual: switch to A while it streams. Expected: live text grows; final result shown when done. Light-theme screenshot. |
| TC-1623 | FR-1623 | Automated: `same_agent_other_conversation_refused`, existing follow-up queue tests. Manual: grok running in A, send grok in B. Expected: 409 with the message above; sending again in A is queued as before. |
| TC-1624 | FR-1624 | Automated: `sessions_running_flag_per_conversation`. Manual: A and B both running → both entries show the spinner; each clears when its turn ends. Screenshot. |
| TC-1625 | FR-1625 | Automated: `background_turn_end_clears_run_state`. Manual: after A finishes in the background, a new prompt in B is `queued: false`. |
| TC-1626 | FR-1620..1625 | Manual matrix in one dashboard process: grok A + cursor (composer-2.5) B, cursor A + agy (gemini-3.8-flash) B, agy A + grok B. Expected: each pair runs concurrently and both results are correct. |
