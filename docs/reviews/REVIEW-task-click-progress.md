# REVIEW: Task click shows prompt and progress

Spec: [SPEC-task-click-progress.md](../specs/SPEC-task-click-progress.md)

| ID | Result | Evidence |
| --- | --- | --- |
| UX-1310 | Met | Selected row renders `prompt:` from `graph.user_prompt` and one progress line per node id (status, runner, model, elapsed, job, agent activity). Missing node id is `<id> · unknown`. Prompt and progress stay hidden until the row is expanded. `cargo test --lib task_checklist_asset` passed. |

Non-goals hold: no snapshot or API change, hover still shows only `done_check` and node ids, TUI unchanged.

Residual risk: TC-1310 checks the bundled script and CSS, not a browser click. Elapsed time uses the existing `nodeElapsed` clock. `scripts/check-spec.ps1` was not run because PowerShell is not installed in this environment.
