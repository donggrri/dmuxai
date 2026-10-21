# SPEC: Task click shows prompt and progress

Status: complete
Owner: DmuxAI
Created: 2026-10-22
Baseline: dashboard task checklist (UX-936)
Related: [SPEC-plan-gate](SPEC-plan-gate.md) UX-936, [SPEC-task-progress-visibility](SPEC-task-progress-visibility.md)

## 1. Decision summary

Clicking a task in the dashboard checklist highlights its swimlane nodes and
expands `done_check` and node ids, but it does not show the prompt that
started the run or how each linked step is progressing. Selecting a task now
also shows that run's user prompt and one progress line per linked node,
using the snapshot already on the page.

## 2. Scope

### In scope

- Selected task row shows the graph user prompt.
- Selected task row shows one line per linked node: id, status, runner, model,
  elapsed time, job, and the matching agent's live activity when the snapshot
  has one.
- Lines refresh from later snapshots and from the dashboard clock while a node
  is running.
- Text is inserted with `textContent` or text nodes.

### Out of scope / non-goals

- A new HTTP API or a change to snapshot JSON.
- Showing the prompt on hover without a selection.
- Opening provider logs beyond the activity string and fields already on the node.
- Changing TUI task rendering or approval-panel task edits.

### Assumptions and open questions

No open questions. The prompt is `graph.user_prompt` (already truncated by the
snapshot). A node id that is not in `graph.nodes` is shown as unknown.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Selected task | The checklist row whose id is toggled by click or Enter/Space | Hover, which still only reveals `done_check` and node ids |
| Progress line | One text line for a node id listed on that task | The graph-detail side panel |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| UX-1310 | Must | Given a dashboard run with tasks, when the user selects a task row | The expanded row shows `prompt: ` plus `graph.user_prompt`, and one `.task-checklist-progress` line per linked node id with that node's status, runner, model, elapsed time (or an em dash when none), and job. If an agent with the same id as the node runner has `activity`, that string is appended. Swimlane highlight from UX-936 stays | An empty prompt renders `prompt: —`. A node id missing from `graph.nodes` renders `<id> · unknown` and no invented status. Deselecting the row hides the prompt and progress lines. A run with no tasks still hides the checklist | TC-1310 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1310 | UX-1310 | unit (asset) | Bundled `app.js` and `app.css` | Inspect the task-checklist renderer | Selected-row markup uses `task-checklist-prompt` and `task-checklist-progress`, reads `user_prompt` and agent `activity`, and does not use `innerHTML` in the checklist block | `src/web/assets.rs` `task_checklist_asset` |
