# PLAN-task-sessions (design for SPEC-task-sessions)

Status: draft (design; awaiting owner review)

Owner: dmuxai
Created: 2026-11-02
Baseline: main e7446f0
Related: SPEC-automatic-cross-agent-sharing FR-025..FR-028, SPEC-codex-model-effort-mode-context, SPEC-concurrent-conversations, SPEC-dashboard-parity, SPEC-plan-gate

## 1. Decision summary

A task is a dmuxai conversation. Tasks reuse the existing ordered transcript,
so there is no new transcript table and no migration of existing conversations.
A task also remembers its last selected agent, model, and reasoning effort (when
the runner offers one), so re-opening it restores the picker. (User-facing
Ask/Plan/Implement modes were removed by SPEC-remove-modes-chat-markdown, so a
task stores no mode.)

Each agent keeps its own native provider session inside a task. When another
agent adds completed turns, the original agent keeps its binding and, on its
next request, resumes that native session with only the unseen turns since its
`known_through_seq`. The delta is bounded and marked as untrusted historical
context. If it cannot be delivered safely, dmuxai falls back to today's fresh
session plus bounded summary/recent projection.

Users can also ask a reviewer agent to perform a read-only review of the task.
The review sees the same delta, a bounded working-tree diff, and the latest plan;
its result is stored as a normal completed turn in the same task.

Users can also send one prompt to several agents at once (fan-out), mainly to
compare plans. Each agent runs as its own leg in the same task, read-only by
default, up to a concurrency limit. One failing leg does not fail the others.
The answers are shown side by side; the user picks one plan or asks one agent to
merge them, and that agent gets the other answers through its unseen delta.
Fan-out runs inside one dmuxai process (no parallel `dmuxai run` processes on one
state database) and reuses the graph executor's parallel scheduling where it
fits, kept on its own run kind so normal graph runs do not change.

### Current behavior

`src/conversation.rs` already stores one shared ordered transcript per
conversation in `turns` and `messages`. Every turn records `requested_agent` and
`model_id`, and each `(conversation, agent)` owns at most one
`provider_bindings` row with `provider_session_id`, `status`, and
`known_through_seq`. `begin_turn` builds the shared projection and calls
`active_binding_for`, which returns a binding only when it is `active` and
`known_through_seq` covers the projection head. On `TurnEvent::Completed`,
`apply_turn_event` advances `known_through_seq` to the completed turn's seq
only after the turn is saved with `TurnStatus::Completed`; failed, cancelled,
and interrupted turns do not advance it.

`src/app.rs` renders the projection with `render_restored_prompt` under the
`[DmuxAI shared context]` envelope, labelling each recent turn by sequence and
agent. Today, when another agent completes a turn after an agent's binding,
FR-027 treats that binding as stale: the next request drops native resume and
starts a fresh provider session with the full bounded summary/recent projection.
Per-agent resume is therefore lost after cross-agent traffic.

## 2. Scope

### In scope

- Expose conversations as tasks without changing transcript storage.
- Persist and restore each task's last agent, model, and reasoning effort.
- Keep and resume each agent's native provider session inside a task.
- Hand a bounded unseen-turn delta to a resumed agent, and consume it only after
  a successful completed turn.
- Add a read-only review turn with delta, bounded diff, and latest plan context.
- Send one prompt to several agents at once (fan-out, mainly for planning), show
  the answers side by side, and pick one plan or ask one agent to merge them.
- Show task-oriented copy, spoken agents, and live native sessions in the
  dashboard and terminal UI.

### Out of scope / non-goals

- Model cooldown.
- Mid-run steering or message injection.
- Per-task git worktree, and write-capable fan-out with per-leg worktrees.
- MCP support (on hold).
- A desktop app.
- A new transcript table or migration of existing turn/message rows.

### Assumptions and open questions

- "Live native session" means a provider binding whose status is `active`; it
  does not prove that the provider process is still running.
- Review diff scope is the workspace working tree at review time. Untracked
  files are listed by path only; their full contents are not sent.
- If the task has no completed non-review turn, a review is refused rather than
  inventing a reviewed agent.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Task | A dmuxai conversation, including its existing transcript and bindings. | A to-do item or a graph node |
| Task settings | The conversation's last selected agent, model, and reasoning effort. | Global runner defaults |
| Live session | An `active` provider binding for one `(task, agent)`. | A running turn |
| Unseen delta | Completed turns with `seq > known_through_seq` from agents other than the resuming agent. | The full summary/recent projection |
| Consumed delta | Unseen turns whose delivery turn completed and advanced `known_through_seq`. | Turns merely sent to a provider |
| Review turn | A normal turn requested from a reviewer under read-only policy. | A separate review record or artifact |
| Reviewed agent | The last agent with a completed non-review turn in the task. | The reviewer |

A provider binding is no longer discarded merely because another agent spoke.
It remains resumable while it is `active`, and `known_through_seq` still only
advances on a saved `Completed` turn. A binding that is stale, retired, missing,
or cannot deliver a bounded delta falls back to the existing projection path.

## 4. Requirements

The spec file `docs/specs/SPEC-task-sessions.md` is created by PR-B. Each PR
copies its rows from section 9 into that spec's requirements table when it adds
the tests, so the spec gate only ever sees rows with passing evidence.

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |

## 5. Acceptance scenarios and test design

Test rows are copied into the spec together with their requirement rows.

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Add nullable `last_agent`, `last_model_id`, and `last_reasoning_effort` to `conversations`; bump `user_version` from 3 to 4 with an idempotent column-checked migration. No transcript or binding tables change. | Older binaries ignore the additive columns; rollback leaves them harmless; existing rows keep NULL and behave as global defaults | TC-2321, TC-2322 |
| Provider binding | Keep an `active` binding across other agents' turns. A resumable binding returns its own provider session id and receives only the bounded unseen delta. `known_through_seq` advances only on saved `Completed`. | A stale, retired, missing, oversized-delta, or rejected-resume case restores the existing fresh-session projection. No provider CLI protocol changes. | TC-2323, TC-2324, TC-2325, TC-2326 |
| Review | A review is a normal turn with a reviewer agent/model and Plan/read-only policy. It stores either one completed turn or nothing. | No review table or artifact to migrate; cancelling or refusing a review leaves the transcript and bindings unchanged | TC-2327, TC-2328, TC-2329, TC-2330 |
| UI/API | Task copy and spoken/live-agent indicators are derived from the existing conversation snapshot and bindings. | Existing conversation and dashboard routes keep their shapes; missing binding data renders as no live agents | TC-2331 |
| Security | Delta, diff, and review output are untrusted historical data inside the existing envelope, never interpolated into shell commands or configuration; only the active task's own turns and workspace paths are included. | Existing sandbox, mode, and approval policy are unchanged; no secret or environment data is added | TC-2332 |

## 7. Implementation plan

This spec is delivered in stacked pull requests. Each PR adds its requirement and
test rows to this file and keeps the earlier PRs passing.

| PR | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| PR-A | Approve this spec, current-behavior description, contracts, and plan | `docs/plans/PLAN-task-sessions.md` | none (design only) | docs review |
| PR-B | Add task-settings columns, idempotent schema bump, write-on-picker-change, and restore-on-select | `src/conversation.rs`, `src/app.rs`, `tests/conversation.rs`, `src/app.rs` tests | FR-2320, FR-2321, FR-2322 | TC-2320, TC-2321, TC-2322 |
| PR-C | Resume each agent's own session, build the bounded delta, add fallbacks, and consume only on completed turns | `src/conversation.rs`, `src/app.rs`, `tests/conversation.rs`, provider tests | FR-2323, FR-2324, FR-2325, FR-2326, FR-2332 | TC-2323, TC-2324, TC-2325, TC-2326, TC-2332 |
| PR-D | Render task copy, spoken agents, and live native-session indicators in the dashboard and terminal UI, plus the stored agent/model badge on sidebar task rows | `src/app.rs`, `src/conversation.rs`, `src/web/snapshot.rs`, `src/web/assets/session.js`, `src/web/assets.rs`, UI tests | FR-2331, FR-2341 | TC-2331, TC-2341 |
| PR-E | Add `/review <agent> [model]`, the dashboard Review button, read-only review context, and completed review turns | `src/app.rs`, `src/conversation.rs`, `src/web/assets/session.js`, `src/web/assets.rs`, integration tests | FR-2327, FR-2328, FR-2329, FR-2330, FR-2332 | TC-2327, TC-2328, TC-2329, TC-2330 |
| PR-G | Multi-agent fan-out for planning: `/fanout`, `--agents`, dashboard multi-agent toggle, concurrency limit, read-only legs, per-leg cancel, partial failure, side-by-side view, pick/merge | `src/app.rs`, `src/cli.rs`, `src/settings.rs`, `src/conversation.rs`, `src/web/assets/session.js`, `src/web/assets.rs`, tests | FR-2333..FR-2339 | TC-2333..TC-2339 |

Rollback: PR-B's columns are additive and can remain unread; PR-C and PR-E fall
back to the existing fresh-session and normal-turn paths when disabled. No
transcript or provider session data needs rewriting.

## 8. Exit criteria

- [ ] Every Must requirement maps to a passing TC in the traceability table.
- [ ] A database written before this change opens, migrates once, and keeps all
      existing conversations readable.
- [ ] Per-agent resume, delta bounds, and consume-only-on-completed behavior pass
      deterministic tests without live provider credentials.
- [ ] A review is refused when read-only is unavailable and never grants write
      permission.
- [ ] Delta, diff, and review output remain untrusted, active-task-only data.
- [ ] `cargo fmt --check`, `cargo test --all-targets`, and
      `cargo clippy --all-targets --all-features -- -D warnings` pass.
- [ ] `python3 scripts/check-spec.py --all` and `bash scripts/spec-index.sh`
      pass, and the review record names residual risk and rollback notes.

## 9. Planned requirements and tests

These rows are the design for PR-B..PR-G. Each PR copies its rows into
`docs/specs/SPEC-task-sessions.md` (dropping the first column) when it adds the tests.

| Lands in | ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| PR-B | FR-2320 | Must | An existing conversation is listed or selected as a task | It keeps its existing id, title, order, turns, messages, and bindings; task listing and selection add no transcript rows or transcript migration | No conversation data is rewritten or lost; archived conversations stay hidden as today | TC-2320 |
| PR-B | FR-2321 | Must | A conversation database at schema version 3 is opened | An idempotent migration adds nullable `last_agent`, `last_model_id`, and `last_reasoning_effort` columns to `conversations` and bumps `user_version` to 4 | Reopening is a no-op; old rows remain readable with NULL values; a failed migration writes nothing and reports the existing schema/database error | TC-2321 |
| PR-B | FR-2322 | Must | The user changes agent, model, or effort in an active task, or selects an existing task | Successful changes are stored on that conversation; selecting the task restores the stored agent, model, and effort into the picker | NULL fields use global/default selection without writing a value; an unknown or unsupported stored value is ignored and replaced by the default; a failed write leaves the current session unchanged and shows a notice without blocking the turn | TC-2322 |
| PR-C | FR-2323 | Must | Agent A has an `active` binding with `known_through_seq = K`, and completed turns from other agents have `seq > K` | A later request to A resumes A's own provider session (`resume_session` is that binding id); the binding is not retired merely because another agent spoke | Stale, retired, or missing binding uses the existing fresh-session path; no other agent's binding is resumed | TC-2323 |
| PR-C | FR-2324 | Must | A resumed agent receives unseen turns | The request carries only completed turns from other agents with `seq > known_through_seq`, ordered by seq, each labelled with source agent and model, inside the existing `[DmuxAI shared context]` untrusted historical-context envelope | Queued, running, failed, cancelled, and interrupted turns are not included; no turn from another task or workspace is included | TC-2324 |
| PR-C | FR-2325 | Must | A delta is larger than 200 turns or 65,536 UTF-8 bytes of text, the binding is stale or retired, or native resume is rejected | dmuxai retires the unusable binding if needed and starts a fresh provider session with the existing bounded summary/recent-tail projection; a native-resume rejection is retried once this way | An agent that never ran in the task gets the existing summary plus recent-tail projection; if the fresh retry also fails, the normal turn failure is reported | TC-2325 |
| PR-C | FR-2326 | Must | A turn that received an unseen delta finishes as completed, failed, cancelled, or interrupted | `known_through_seq` advances to the turn seq only after a `Completed` turn is saved; any other terminal state leaves it unchanged | The same unseen turns are delivered again on the next request, as a resumed delta when the binding still survives or through the fallback projection otherwise | TC-2326 |
| PR-E | FR-2327 | Must | The user enters `/review <agent> [model]` in the terminal UI, or uses the dashboard Review button | The named agent and optional model become the reviewer, and the reviewed agent is the task's last completed non-review agent; the review starts only after both are known | Missing, unknown, or non-runnable agent/model gives a clear message and creates no turn; with no completed non-review turn the review is refused | TC-2327 |
| PR-E | FR-2328 | Must | A review turn starts | The reviewer runs under the read-only turn policy (`TurnPolicy::ReadOnly`) even though normal turns are write-capable, and the task's stored settings are unchanged | A runner that cannot run read-only refuses the review before any turn is saved with a message such as `review needs a runner that supports read-only turns`; the reviewer never receives write permission | TC-2328 |
| PR-E | FR-2329 | Must | A review turn is prepared | Its context contains the same bounded unseen delta as FR-2324, the workspace working-tree git diff of files changed since the reviewed agent's last completed turn, and the latest plan if one exists | The diff is bounded to 65,536 UTF-8 bytes, ends with `[diff truncated]` when cut, lists untracked files by path only, and is labelled untrusted; a non-git workspace or diff error lets the review proceed without a diff and with a notice | TC-2329 |
| PR-E | FR-2330 | Must | A review turn completes | It is saved as a normal completed turn in the same task with `requested_agent` set to the reviewer and its model id, labelled `Review by <reviewer>` in the transcript and as a reviewer turn in later deltas, so the reviewed agent sees it on its next turn | A failed or cancelled review is not a completed turn and does not advance the reviewer's `known_through_seq`; no separate review table or artifact is created | TC-2330 |
| PR-D | FR-2331 | Must | The dashboard sidebar/task header or terminal task picker is shown | Copy uses "task" only where it clarifies the conversation-as-task model; the task header lists agents that have spoken and marks those with an `active` native binding as live; switching agent inside the task remains the existing picker | With no bindings, no agent is marked live; an agent with only stale bindings is shown as having spoken but not live | TC-2331 |
| PR-D | FR-2341 | Must | The dashboard sidebar lists tasks of the current workspace | Each task row shows its stored agent and model (task settings, FR-2322) as a short badge, falling back to `last_agent` when settings are NULL; the row tooltip lists the agents that have spoken in that task | NULL settings and no turns show no badge; an unknown agent id shows the raw id | TC-2341 |
| PR-C | FR-2332 | Must | A delta, review diff, or review result is rendered or sent | It is treated as untrusted historical data, kept inside the existing context envelope, never interpolated into shell commands or configuration, and contains only the active task's own turns plus workspace paths | No environment variables, credentials, tokens, secret-store contents, or other-task text are included; existing sandbox and mode policy are unchanged | TC-2332 |
| PR-G | FR-2333 | Must | The user picks two or more agents (each optionally with a model) for one prompt: `/fanout <agent[:model]>,<agent[:model]>... <prompt>` in the terminal UI, `dmuxai run - --agents a[:m],b[:m]` headless, or the dashboard composer's multi-agent toggle | One leg per agent starts in the active task with the same prompt; each leg is a normal turn with its own `requested_agent`, model and provider binding | Duplicate agents, unknown agents, disallowed models or fewer than two legs are rejected before any turn is saved. Two models on one runner need two `[[agents.custom]]` entries (one model per agent) | TC-2333 |
| PR-G | FR-2334 | Must | A fan-out starts | Legs run in parallel up to a concurrency limit (`[fanout] max_parallel`, default 3); the rest wait queued and start as legs finish. Before starting, the UI states how many agents will run | The limit is clamped to 1..=8; a queued leg can be cancelled before it starts | TC-2334 |
| PR-G | FR-2335 | Must | A fan-out leg starts | Legs run under the read-only turn policy by default (planning); `--write` / a dashboard switch is required for write-capable legs. A runner with no read-only policy runs only with a do-not-edit note plus a `git status --porcelain` check before and after; a changed tree marks that leg `tainted` | Write-capable fan-out with per-leg git worktrees is out of scope | TC-2335 |
| PR-G | FR-2336 | Must | Legs of one fan-out are prepared | Each leg receives its own unseen delta (FR-2324) as of the fan-out start, and never the answers of the other legs of the same fan-out, so the plans stay independent | A leg that falls back to another runner never lands on a runner already used by another leg; the fallback is recorded on the leg | TC-2336 |
| PR-G | FR-2337 | Must | One leg fails, times out, or is cancelled | The other legs keep running and are saved; the fan-out is `completed` when at least one leg completed, otherwise `failed`. Each leg can be cancelled on its own, and "cancel all" stops every leg | A failed leg does not advance its `known_through_seq` (FR-2326) | TC-2337 |
| PR-G | FR-2338 | Must | Fan-out legs finish | The dashboard shows leg answers side by side (one column per agent with agent, model, status and elapsed time); the terminal UI shows them one after another with agent headers; headless JSON adds `fanout: {id, legs: [{agent, model, ok, text, error, elapsed_ms}]}`. All leg turns are saved in the shared transcript tagged by agent and fan-out id | A task reloaded later still groups the legs by fan-out id | TC-2338 |
| PR-G | FR-2339 | Must | After a fan-out, the user picks one leg's plan, or asks one agent to merge or compare the plans | Picking a plan records it as the task's latest plan. A merge/compare request is a normal turn to the chosen agent, which receives the other legs' answers through its unseen delta (FR-2324), each wrapped as untrusted data | If the chosen agent was a leg, its own answer is not repeated in its delta | TC-2339 |

| Lands in | Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- | --- |
| PR-B | TC-2320 | FR-2320 | conversation integration | Two conversations with completed turns and bindings | List tasks, select each, reopen the database | Same ids, titles, turn counts, messages, and bindings; no transcript row is copied or rewritten | `tests/conversation.rs` `tc_2320_existing_conversation_is_task_without_transcript_migration` |
| PR-B | TC-2321 | FR-2321 | unit (migration) | A v3-shaped database with an existing conversation and no task-settings columns | Open with `ConversationEngine::open`; open again | Columns exist, `user_version` is 4, old row has NULL settings, transcript unchanged, second open is a no-op | `tests/conversation.rs` `tc_2321_task_settings_migration_is_idempotent` |
| PR-B | TC-2322 | FR-2322 | conversation + app integration | Task with stored agent/model/effort; task with NULL settings; task with an unknown stored model | Change pickers, leave the task, select it again; select a NULL task | Stored values are restored; NULL and unknown values fall back to defaults without blocking selection or a turn | `tests/conversation.rs` `tc_2322_task_settings_round_trip_and_defaults`; `src/app.rs` `tc_2322_select_restores_task_picker` |
| PR-C | TC-2323 | FR-2323 | conversation integration | Codex completes a turn, Cursor completes the next turn, Codex is requested again | Inspect the Codex request | Codex `resume_session` equals its active binding id; the Cursor turn does not retire it | `tests/conversation.rs` `tc_2323_agent_resumes_own_session_after_other_agent_turn` |
| PR-C | TC-2324 | FR-2324 | conversation integration | Agent A bound through seq N; other agents complete turns N+1 and N+2; another task has later turns | Begin A again and inspect context | Context contains only N+1 and N+2, in order, with agent and model labels inside `[DmuxAI shared context]`; no other-task turn appears | `tests/conversation.rs` `tc_2324_delta_contains_only_unseen_active_task_turns` |
| PR-C | TC-2325 | FR-2325 | conversation integration | Delta with 201 unseen turns, then a 65,537-byte delta, then a stale binding; an agent with no binding | Begin turns in each setup | Oversized and stale cases use no resume id and receive the existing bounded projection; never-run agent uses summary/recent tail; a rejected resume is retried fresh once | `tests/conversation.rs` `tc_2325_delta_bounds_and_resume_fallbacks`; `src/app.rs` `tc_2325_resume_rejection_retries_fresh` |
| PR-C | TC-2326 | FR-2326 | conversation integration | Agent A bound through seq K with unseen turns from agent B | Complete A's turn; then fail, cancel, and interrupt separate attempts | Only the completed turn advances `known_through_seq`; each unsuccessful terminal state re-delivers the same unseen turns on the next request | `tests/conversation.rs` `tc_2326_delta_is_consumed_only_on_completed` |
| PR-E | TC-2327 | FR-2327 | app unit + static asset | Task with a completed Codex turn | Enter `/review cursor` and `/review cursor gpt-5`; click the dashboard Review button; try an unknown agent | Reviewer and model are selected, reviewed agent is Codex; invalid input shows a message and creates no turn; dashboard posts the same review request | `src/app.rs` `tc_2327_review_command_selects_reviewer`; `src/web/assets.rs` `tc_2327_dashboard_review_button` |
| PR-E | TC-2328 | FR-2328 | app + conversation integration | Normal turns are write-capable; reviewer supports read-only; a second reviewer does not | Start a review, then attempt it with the unsupported runner | Review request carries Plan/read-only policy and the task's stored settings are unchanged; unsupported runner is refused with a clear message and no saved turn | `src/app.rs` `tc_2328_review_is_read_only_regardless_of_task_mode` |
| PR-E | TC-2329 | FR-2329 | app integration | Task with an unseen delta, a modified tracked file, an untracked file, a latest plan, and a diff over 65,536 bytes | Prepare the review request | Prompt contains delta, bounded diff ending in `[diff truncated]`, untracked path only, latest plan, and untrusted labels; non-git workspace proceeds without diff | `src/app.rs` `tc_2329_review_context_has_bounded_diff_and_plan` |
| PR-E | TC-2330 | FR-2330 | conversation integration | Reviewer completes a review, then another review is cancelled | Inspect the task and the original agent's next delta | Review is a completed turn labelled `Review by <reviewer>` with reviewer agent/model; original agent's delta includes it; cancelled review is not completed and does not advance the binding | `tests/conversation.rs` `tc_2330_review_saved_as_completed_turn` |
| PR-D | TC-2331 | FR-2331 | unit (static asset + app state) | Task with three spoken agents, two active bindings and one stale binding | Render the dashboard header and terminal task view | Copy says task where applicable; spoken agents are listed; only the two active bindings are marked live; the existing agent picker still switches agents | `src/web/assets.rs` `tc_2331_task_header_lists_spoken_and_live_agents`; `src/app.rs` `tc_2331_task_header_live_session_marks`; `tests/conversation.rs` `tc_2331_spoken_agents_query`; `tests/web_dashboard_fullwidth.rs` `web_task_header_chips` |
| PR-D | TC-2341 | FR-2341 | unit (static asset + app state) | Task row with stored agent/model, a task with NULL settings and no turns, and a task with an unknown stored agent id | Render the dashboard sidebar | The badge shows the stored agent and short model; NULL settings with no turns show no badge; an unknown id is shown raw; the tooltip lists the agents that have spoken | `src/web/assets.rs` `tc_2341_sidebar_task_row_shows_agent_badge`; `tests/web_dashboard_fullwidth.rs` `web_sidebar_agent_badges` |
| PR-C | TC-2332 | FR-2332 | unit + conversation integration | Delta and diff containing shell metacharacters, secret-looking text, and a second task | Build and inspect provider requests and review prompt | Content stays inside the untrusted envelope, is not placed in a shell command or config, and contains no environment/secret-store data or second-task text | `src/app.rs` `tc_2332_delta_and_diff_remain_untrusted_data`; `tests/conversation.rs` `tc_2332_delta_excludes_other_tasks_and_secrets` |
| PR-G | TC-2333 | FR-2333 | app unit + headless | Task with three runnable agents | `/fanout a,b,c plan X`; `--agents a,b`; duplicate `a,a`; unknown agent | Three leg turns with distinct agents and bindings; invalid input saves no turn | `src/app.rs` `tc_2333_fanout_entry_points_validate_legs` |
| PR-G | TC-2334 | FR-2334 | app integration (fake adapters) | Four legs, `max_parallel = 2` | Start the fan-out | At most two legs run at once; queued legs start as others finish; a queued leg can be cancelled | `src/app.rs` `tc_2334_fanout_respects_concurrency_limit` |
| PR-G | TC-2335 | FR-2335 | app integration | One read-only-capable leg and one leg without read-only that edits a file | Run the fan-out | First leg uses read-only policy; the second is marked `tainted`; without `--write` no write-capable policy is sent | `src/app.rs` `tc_2335_fanout_legs_read_only_by_default` |
| PR-G | TC-2336 | FR-2336 | conversation integration | Two legs in one fan-out; leg A completes first | Inspect leg B's request and a forced fallback | B's context has no text from A's answer; the fallback runner is not another leg's runner | `tests/conversation.rs` `tc_2336_fanout_legs_are_independent` |
| PR-G | TC-2337 | FR-2337 | app integration | Three legs; one fails, one is cancelled | Let the third complete | Fan-out is `completed`; the completed leg is saved; the failed and cancelled legs keep their `known_through_seq` | `src/app.rs` `tc_2337_one_leg_failure_does_not_fail_others` |
| PR-G | TC-2338 | FR-2338 | static asset + headless JSON | Completed fan-out with two legs | Render the dashboard and read headless JSON | Side-by-side columns with agent/model/status; JSON `fanout.legs` has both legs; reload keeps grouping | `src/web/assets.rs` `tc_2338_fanout_side_by_side_view`; `src/app.rs` `tc_2338_fanout_headless_json` |
| PR-G | TC-2339 | FR-2339 | conversation integration | Completed fan-out with legs A and B | Pick B's plan; ask A to merge | Latest plan is B's; A's merge request delta contains B's answer once and not A's own | `tests/conversation.rs` `tc_2339_merge_turn_gets_other_legs_via_delta` |
