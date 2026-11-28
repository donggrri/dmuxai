# REVIEW: Board web fixes (#358, #359, #360, #369)

Spec: [../specs/SPEC-board-web-fixes.md](../specs/SPEC-board-web-fixes.md) ·
Plan: [../plans/PLAN-board-web-fixes.md](../plans/PLAN-board-web-fixes.md) ·
Tasks: [../tasks/TASKS-board-web-fixes.md](../tasks/TASKS-board-web-fixes.md)

## Spec compliance

| Requirement | Verdict | Evidence |
| --- | --- | --- |
| FR-3810 | Met | `human_browser_rejection` accepts a request without `Sec-Fetch-*` only when `BoardApiState.wsl_host_peer` is set (WSL interface listener only), the connection peer IP equals it, `Origin == http://<Host>` and `X-Dmuxai-Human: 1`. TC-3810 covers 200 for that request and 403 for missing/wrong human header, missing/wrong Origin, `Sec-Fetch-Site: cross-site`, a lone `Sec-Fetch-Mode`, another peer (with a valid token so the middleware passes) and no peer info. |
| FR-3811 | Met | Loopback and remote listeners build the router with `wsl_host_peer = None`, so only the strict rule applies. TC-3811 asserts 403 for Origin + human header without `Sec-Fetch-*` on both, and 200 for a strict same-origin request on loopback. |
| FR-3812 | Met | `boardCollectSteps(plan)` copies every own field of `plan.steps[data-step-index]` before applying the table inputs; new rows get `task = tasks[0].id` when tasks exist. The plan passed is the one the table was rendered from (falls back to `boardSelectedItem.plan`). TC-3812 e2e: 200, `task`/`sensitive` kept, new step in `task-1`; the asset test guards the shape. |
| FR-3813 | Met | `PendingPlanFile` carries `dynamic_cap` (`serde(default, skip_serializing_if = "Option::is_none")`) and `json_and_hash()`; `src/app.rs` aliases `HeadlessPendingPlan` to it and hashes with the same function, so the two cannot drift. Dashboard, CLI and MCP plan edits pass the current plan's `dynamic_cap` to `edit_plan_from_json_keep_cap`. TC-3813: headless bytes with `dynamic_cap: 3` attach via `board add --plan-hash` and `attach_plan`, an edit keeps the cap with a verified hash, a capless plan hashes as before, a tampered cap is refused. `board ask --revise` attaches through the same `attach_plan`/`edit_plan` hash check. |
| FR-3814 | Met | The notice is keyed to an item (`boardPlanConflictId`). It is set on 409 before the modal reloads (the modal opens if it was closed), is no longer cleared by the drawer fetch, and is cleared on close, on opening another item, and on the next successful decision. TC-3814 checks 409, notice visible, notice still there after a list refresh re-fetches the open item, gone after close/reopen, no decision, no run. TC-3605 now hard-asserts the notice. |
| NFR-3810 | Met | Peer lookup and the fallback run only inside the four owner-write handlers; the middleware and other routes are unchanged (TC-3811 also posts a side-chat message without human headers). |
| Non-goals | Respected | No spec-first steps on board plans (#374), no SSE token change (#367), no loopback peer check (#355). No `.github/` changes. |

## Regression evidence

With the fix reverted in `board.js` only, `tc_3605`, `tc_3812` (422 "step must
reference a task") and `tc_3814` (notice never appears) all fail; with the fix
they pass. TC-3810 fails on the old strict-only rule by construction.

## Commands and results

- `cargo fmt` / `cargo fmt --check`: clean
- `cargo clippy --all-targets --all-features -- -D warnings`: clean
- `cargo test --lib -- board:: web::`: 272 passed
- `cargo test --bin dmuxai -- tc_2942 tc_2550`: 4 passed
- `cargo test --all-targets`: 0 failures
- `cargo test --test web_board_e2e -- --ignored --test-threads=1` (Chrome +
  Playwright present): 13 passed, including `tc_3605`, `tc_3812`, `tc_3814`
- `bash scripts/check-spec.sh docs/specs/SPEC-board-web-fixes.md`: PASS
- `python3 scripts/check-spec.py --all`: 117/117 pass, 0 allowlisted

## Residual risk / untested paths

- The WSL fallback was tested at the router level with a synthetic peer; no
  real Windows-host browser run was done (no WSL host in CI).
- A successful plan save no longer clears the stale-plan notice; FR-3814 lists
  only a new decision, closing the modal, or opening another item.
- Board events re-render the open modal and discard unsaved plan-table edits.
  That bug already exists and is outside this spec. TC-3812 and TC-3814 block
  the event stream in the browser so they stay deterministic.
- The "Remove" button on a newly added (unsaved) row still does nothing. That
  bug also already exists and is outside this spec.
