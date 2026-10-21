# PLAN: Task click shows prompt and progress

Status: complete
Spec: [SPEC-task-click-progress.md](../specs/SPEC-task-click-progress.md)

1. Extend the dashboard task checklist in `src/web/assets/app.js` so a selected
   row renders the graph user prompt and one progress line per linked node
   from `graph.nodes` and `snapshot.agents[].activity`. Unknown node ids get
   `<id> · unknown`. The dashboard tick re-renders the checklist while a task
   stays selected so elapsed time moves.
2. Hide `.task-checklist-prompt` and `.task-checklist-progress` until the row
   has `task-checklist-row-expanded` (`src/web/assets/app.css`).
3. Extend `task_checklist_asset` (TC-1310) to lock those strings and the
   no-`innerHTML` rule.
4. Record the review after `cargo test task_checklist_asset`.

## Risks

- Clock interpolation for a running node uses the existing `nodeElapsed`
  helper. No new persistence. Rollback is reverting the asset and spec files.
