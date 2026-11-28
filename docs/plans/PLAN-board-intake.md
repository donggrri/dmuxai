# PLAN: Board intake from a prompt (planner auto plan)

Spec: [SPEC-board-intake](../specs/SPEC-board-intake.md)
(FR-3860..FR-3866, NFR-3860; fixes #375). Builds on
[SPEC-board-approval-hardening](../specs/SPEC-board-approval-hardening.md)
(launch token, `DMUXAI_RUN_ID` on children), which stays unchanged.

## Order

1. `src/app.rs` — headless approval gate (FR-3860, NFR-3860): the pending-plan
   writer picks the latest revision without errors **or** the fallback revision
   (its `errors` hold the planner failure, which made the old code skip it, #375).
   It returns the hash, `plan_source` (`fallback` when the revision or the
   triage source is fallback) and a masked `planner_error` capped at 500 chars.
   `HeadlessOutcome` gains `plan_source` / `planner_error`; no extra agent turn.
2. `src/app.rs` + `src/cli.rs` — `run --graph --plan-only` (FR-3866): parser
   flag and conflicts (exit 2); `HeadlessRequest.plan_only` starts planning
   directly (`Route::Complex`, override kind `Plan`, like `/plan` but without a
   runner pin), skips the slash-command path and never auto-approves, so the run
   always stops at exit 7. MCP `run` gets `plan_only` → `--plan-only`.
3. `src/board/mod.rs` / `store.rs` — `BoardItem.planning` (serde default,
   skipped when absent), `set_planning` (masked `planner` thread note, event),
   `add_reporting_created` for key idempotency (FR-3863), `PendingItem.warning`
   and `BoardItem::planning_warning` (FR-3862).
4. `src/board/runner.rs` — intake: `intake_add` (validation before creation),
   `intake_prepare` (`runs/<id>.plan.lock`, same lock + launch-token scheme as
   board runs, FR-3864), `intake_execute` (child `dmuxai -C <ws> run --graph
   --plan-only --json -`, prompt on stdin, 900 s, env policy of
   `spawn_child_dmuxai`; exit 7 → existing `attach_plan`, FR-3106/FR-3130),
   detached launcher (`board intake <id>`, hidden subcommand, token in env).
5. Surfaces: CLI `board add --prompt P|- [--detach]` (+ warning on stderr in
   show/pending text), MCP `board_add.prompt` (detached), HTTP `POST /api/board`
   in `src/web/board_api.rs` (201 plain create; `prompt` only when launching is
   allowed, 202 detached; 403 in `serve`).
6. Dashboard (FR-3865): `index.html` New task button + form, `board.js`
   (visibility from `launch_allowed`, POST via `boardFetchJson`, `Planning…` /
   warning on decision cards, kanban cards and the modal), `board.css`, and
   `window.dmuxaiCurrentWorkspace` in `app.js`.
7. Tests from spec section 5, docs (API.md, README, CHANGELOG), TASKS, REVIEW.

Spec-first insertion is unchanged: it only happens where the headless planner
path already applies it (`park_fallback_plan` / plan finalization). Board-edited
plans are untouched (#374 is out of scope).

## Risks

- Detached intake child crashes before writing a result: the item stays
  `planning`; the `.plan.lock` goes stale (pid dead) so a later intake can run,
  but nothing flips the state automatically.
- `plan_only` with no admitted planner runner yields a fallback plan, which is
  still exit 7 with `plan_source: "fallback"` (intended by FR-3866).
- Overhead (#301): the only new work in `run --graph` is choosing a revision and
  masking an error string at the approval gate; board intake is opt-in.

## Rollback

Revert the change set. `planning` is an optional, serde-defaulted field: an
older binary ignores it on read and drops it on the next write. Intake locks
live under `<state base>/board/runs/<id>.plan.lock`; remove stale ones after a
rollback. Pending-plan and run-manifest formats only gained optional keys.
