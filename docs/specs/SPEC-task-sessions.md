# SPEC-task-sessions

Status: draft (delivered in stacked PRs)

Owner: dmuxai
Created: 2026-11-02
Baseline: main e7446f0
Related: docs/plans/PLAN-task-sessions.md (full design), SPEC-automatic-cross-agent-sharing FR-025..FR-028, SPEC-session-sidebar

## 1. Decision summary

A task is a dmuxai conversation. This spec adds task behavior step by step; the
full design and the planned rows for later steps are in
`docs/plans/PLAN-task-sessions.md` section 9. Each pull request copies its rows
here when it adds their tests.

Step 1 (PR-B): each conversation remembers its last selected agent, model and
reasoning effort, and selecting it restores the picker.

Step 2 (PR-C): each agent keeps its own native provider session inside a task.
When another agent adds completed turns, the original agent resumes that
session with only the bounded unseen delta; oversized or unusable deltas fall
back to the existing fresh-session projection.

Step 3 (PR-D): the dashboard and terminal UI show the task's spoken agents and
mark those with an active native binding as live. Sidebar task rows show the
task's stored agent and model.

## 2. Scope

### In scope

- Task settings columns on `conversations`, the v4 migration, write on picker
  change and turn start, restore on select (TUI and dashboard).
- Per-agent native resume, a bounded unseen-turn delta with source agent and
  model labels, fresh-session fallback, and consume-only-on-completed behavior.
- Task header with spoken-agent and live-session indicators in the dashboard and
  terminal UI, plus the task's stored agent/model badge on sidebar task rows
  (PR-D).

### Out of scope / non-goals

- Review turn (PR-E), fan-out (PR-G). Ask/Plan/Implement modes no longer exist,
  so no mode is stored.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Task | A dmuxai conversation | A to-do item or graph node |
| Task settings | Last selected agent, model and reasoning effort of a conversation; NULL means global default | Global runner defaults |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2320 | Must | An existing conversation is listed or selected as a task | It keeps its existing id, title, order, turns, messages, and bindings; task listing and selection add no transcript rows or transcript migration | No conversation data is rewritten or lost; archived conversations stay hidden as today | TC-2320 |
| FR-2321 | Must | A conversation database at schema version 3 is opened | An idempotent migration adds nullable `last_agent`, `last_model_id`, and `last_reasoning_effort` columns to `conversations` and bumps `user_version` to 4 | Reopening is a no-op; old rows remain readable with NULL values; a failed migration writes nothing and reports the existing schema/database error | TC-2321 |
| FR-2322 | Must | The user changes agent, model, or effort in an active task, or selects an existing task | Successful changes are stored on that conversation; selecting the task restores the stored agent, model, and effort into the picker; restore is silent: no "model selected" chat item | NULL fields use global/default selection without writing a value; an unknown or unsupported stored value is ignored and replaced by the default; a failed write leaves the current session unchanged and shows a notice without blocking the turn | TC-2322, TC-2340 |
| FR-2323 | Must | Agent A has an `active` binding with `known_through_seq = K`, and completed turns from other agents have `seq > K` | A later request to A resumes A's own provider session (`resume_session` is that binding id); the binding is not retired merely because another agent spoke | Stale, retired, or missing binding uses the existing fresh-session path; no other agent's binding is resumed | TC-2323 |
| FR-2324 | Must | A resumed agent receives unseen turns | The request carries only completed turns from other agents with `seq > known_through_seq`, ordered by seq, each labelled with source agent and model, inside the existing `[DmuxAI shared context]` untrusted historical-context envelope | Queued, running, failed, cancelled, and interrupted turns are not included; no turn from another task or workspace is included | TC-2324 |
| FR-2325 | Must | A delta is larger than 200 turns or 65,536 UTF-8 bytes of text, the binding is stale or retired, or native resume is rejected | dmuxai retires the unusable binding if needed and starts a fresh provider session with the existing bounded summary/recent-tail projection; a native-resume rejection is retried once this way | An agent that never ran in the task gets the existing summary plus recent-tail projection; if the fresh retry also fails, the normal turn failure is reported | TC-2325 |
| FR-2326 | Must | A turn that received an unseen delta finishes as completed, failed, cancelled, or interrupted | `known_through_seq` advances to the turn seq only after a `Completed` turn is saved; any other terminal state leaves it unchanged | The same unseen turns are delivered again on the next request, as a resumed delta when the binding still survives or through the fallback projection otherwise | TC-2326 |
| FR-2331 | Must | The dashboard sidebar/task header or terminal task picker is shown | Copy uses "task" only where it clarifies the conversation-as-task model; the task header lists agents that have spoken and marks those with an `active` native binding as live; switching agent inside the task remains the existing picker | With no bindings, no agent is marked live; an agent with only stale bindings is shown as having spoken but not live | TC-2331 |
| FR-2332 | Must | A delta, review diff, or review result is rendered or sent | It is treated as untrusted historical data, kept inside the existing context envelope, never interpolated into shell commands or configuration, and contains only the active task's own turns plus workspace paths | No environment variables, credentials, tokens, secret-store contents, or other-task text are included; existing sandbox and mode policy are unchanged | TC-2332 |
| FR-2341 | Must | The dashboard sidebar lists tasks of the current workspace | Each task row shows its stored agent and model (task settings, FR-2322) as a short badge, falling back to `last_agent` when settings are NULL; the row tooltip lists the agents that have spoken in that task | NULL settings and no turns show no badge; an unknown agent id shows the raw id | TC-2341 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2320 | FR-2320 | conversation integration | Two conversations with completed turns and bindings | List tasks, select each, reopen the database | Same ids, titles, turn counts, messages, and bindings; no transcript row is copied or rewritten | `tests/conversation.rs` `tc_2320_existing_conversation_is_task_without_transcript_migration` |
| TC-2321 | FR-2321 | unit (migration) | A v3-shaped database with an existing conversation and no task-settings columns | Open with `ConversationEngine::open`; open again | Columns exist, `user_version` is 4, old row has NULL settings, transcript unchanged, second open is a no-op | `tests/conversation.rs` `tc_2321_task_settings_migration_is_idempotent` |
| TC-2322 | FR-2322 | conversation + app integration | Task with stored agent/model/effort; task with NULL settings; task with an unknown stored model | Change pickers, leave the task, select it again; select a NULL task | Stored values are restored; NULL and unknown values fall back to defaults without blocking selection or a turn | `tests/conversation.rs` `tc_2322_task_settings_round_trip_and_defaults`; `src/app.rs` `tc_2322_select_restores_task_picker` |
| TC-2323 | FR-2323 | conversation integration | Codex completes a turn, Cursor completes the next turn, Codex is requested again | Inspect the Codex request | Codex `resume_session` equals its active binding id; the Cursor turn does not retire it | `tests/conversation.rs` `tc_2323_agent_resumes_own_session_after_other_agent_turn` |
| TC-2324 | FR-2324 | conversation integration | Agent A bound through seq N; other agents complete turns N+1 and N+2; another task has later turns | Begin A again and inspect context | Context contains only N+1 and N+2, in order, with agent and model labels inside `[DmuxAI shared context]`; no other-task turn appears | `tests/conversation.rs` `tc_2324_delta_contains_only_unseen_active_task_turns` |
| TC-2325 | FR-2325 | conversation integration | Delta with 201 unseen turns, then a 65,537-byte delta, then a stale binding; an agent with no binding | Begin turns in each setup | Oversized and stale cases use no resume id and receive the existing bounded projection; never-run agent uses summary/recent tail; a rejected resume is retried fresh once | `tests/conversation.rs` `tc_2325_delta_bounds_and_resume_fallbacks`; `src/app.rs` `tc_2325_resume_rejection_retries_fresh` |
| TC-2326 | FR-2326 | conversation integration | Agent A bound through seq K with unseen turns from agent B | Complete A's turn; then fail, cancel, and interrupt separate attempts | Only the completed turn advances `known_through_seq`; each unsuccessful terminal state re-delivers the same unseen turns on the next request | `tests/conversation.rs` `tc_2326_delta_is_consumed_only_on_completed` |
| TC-2331 | FR-2331 | unit (static asset + app state) | Task with three spoken agents, two active bindings and one stale binding | Render the dashboard header and terminal task view | Copy says task where applicable; spoken agents are listed; only the two active bindings are marked live; the existing agent picker still switches agents | `src/web/assets.rs` `tc_2331_task_header_lists_spoken_and_live_agents`; `src/app.rs` `tc_2331_task_header_live_session_marks`; `tests/conversation.rs` `tc_2331_spoken_agents_query`; `tests/web_dashboard_fullwidth.rs` `web_task_header_chips` |
| TC-2332 | FR-2332 | unit + conversation integration | Delta containing shell metacharacters, secret-looking text, and a second task | Build and inspect the provider request | Delta content stays inside the untrusted envelope, is not placed in a shell command or config, and contains no environment/secret-store data or second-task text; review diff/result paths reuse the same envelope when PR-E adds them | `src/app.rs` `tc_2332_delta_remains_untrusted_data`; `tests/conversation.rs` `tc_2332_delta_excludes_other_tasks_and_secrets` |
| TC-2340 | FR-2322 | app unit | Task with a stored model; an explicit model pick after a restore | Select the task, then deliver the adapter's `ModelSelected` echo; pick a model explicitly, then deliver its echo | Restore adds no "model selected" chat item while the catalog is updated; the explicit pick adds exactly one "model selected" item | `src/app.rs` `tc_2322_restore_is_quiet` |
| TC-2341 | FR-2341 | unit (static asset + app state) | Task row with stored agent/model, a task with NULL settings and no turns, and a task with an unknown stored agent id | Render the dashboard sidebar | The badge shows the stored agent and short model; NULL settings with no turns show no badge; an unknown id is shown raw; the tooltip lists the agents that have spoken | `src/web/assets.rs` `tc_2341_sidebar_task_row_shows_agent_badge`; `tests/web_dashboard_fullwidth.rs` `web_sidebar_agent_badges` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Nullable `last_agent`, `last_model_id`, `last_reasoning_effort` on `conversations`; `user_version` 3 -> 4, column-checked and idempotent | Older binaries ignore the columns; NULL rows behave as today | TC-2321, TC-2322 |
| Provider binding | Keep an `active` binding across other agents' turns. A resumable binding returns its own provider session id and receives only the bounded unseen delta. `known_through_seq` advances only on saved `Completed`. | A stale, retired, missing, oversized-delta, or rejected-resume case restores the existing fresh-session projection. No provider CLI protocol changes. | TC-2323, TC-2324, TC-2325, TC-2326 |
| UI/API | The dashboard task header and sidebar badges are derived from the existing conversation snapshot, completed turns, bindings, and task settings; the terminal task view uses the same read-only query. | Existing conversation and dashboard routes keep their shapes; missing binding or task data renders as no live agents or no badge | TC-2331, TC-2341 |
| Security | Delta and review output are untrusted historical data inside the existing envelope, never interpolated into shell commands or configuration; only the active task's own turns and workspace paths are included. | Existing sandbox, mode, and approval policy are unchanged; no secret or environment data is added | TC-2332 |
