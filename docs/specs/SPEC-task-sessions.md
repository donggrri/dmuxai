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

## 2. Scope

### In scope

- Task settings columns on `conversations`, the v4 migration, write on picker
  change and turn start, restore on select (TUI and dashboard).

### Out of scope / non-goals

- Provider binding / resume changes (PR-C), task header (PR-D), review turn
  (PR-E), fan-out (PR-G). Ask/Plan/Implement modes no longer exist, so no mode is stored.

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
| FR-2322 | Must | The user changes agent, model, or effort in an active task, or selects an existing task | Successful changes are stored on that conversation; selecting the task restores the stored agent, model, and effort into the picker | NULL fields use global/default selection without writing a value; an unknown or unsupported stored value is ignored and replaced by the default; a failed write leaves the current session unchanged and shows a notice without blocking the turn | TC-2322 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2320 | FR-2320 | conversation integration | Two conversations with completed turns and bindings | List tasks, select each, reopen the database | Same ids, titles, turn counts, messages, and bindings; no transcript row is copied or rewritten | `tests/conversation.rs` `tc_2320_existing_conversation_is_task_without_transcript_migration` |
| TC-2321 | FR-2321 | unit (migration) | A v3-shaped database with an existing conversation and no task-settings columns | Open with `ConversationEngine::open`; open again | Columns exist, `user_version` is 4, old row has NULL settings, transcript unchanged, second open is a no-op | `tests/conversation.rs` `tc_2321_task_settings_migration_is_idempotent` |
| TC-2322 | FR-2322 | conversation + app integration | Task with stored agent/model/effort; task with NULL settings; task with an unknown stored model | Change pickers, leave the task, select it again; select a NULL task | Stored values are restored; NULL and unknown values fall back to defaults without blocking selection or a turn | `tests/conversation.rs` `tc_2322_task_settings_round_trip_and_defaults`; `src/app.rs` `tc_2322_select_restores_task_picker` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Nullable `last_agent`, `last_model_id`, `last_reasoning_effort` on `conversations`; `user_version` 3 -> 4, column-checked and idempotent | Older binaries ignore the columns; NULL rows behave as today | TC-2321, TC-2322 |
