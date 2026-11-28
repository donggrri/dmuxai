# PLAN: Board web fixes (#358, #359, #360, #369)

Covers FR-3810..FR-3814 and NFR-3810 from
[../specs/SPEC-board-web-fixes.md](../specs/SPEC-board-web-fixes.md).
Spec-first steps on board plans (#374) are out of scope.

## Order of work

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | `board_api::router` takes the listener's `WebAccess`; `BoardApiState.wsl_host_peer` is set only for `ListenerKind::WslInterface`. Owner-write handlers read the per-connection `ConnectInfo<PeerConnectInfo>` extension and pass it to `human_browser_rejection`, which keeps the strict rule and adds the WSL fallback (trusted peer, no `Sec-Fetch-*`, `Origin == http://<Host>`, `X-Dmuxai-Human: 1`). | `src/web/board_api.rs`, `src/web/server.rs` | FR-3810, FR-3811, NFR-3810 | `cargo test --lib tc_3810 tc_3811` |
| 2 | `PendingPlanFile` gains `dynamic_cap` (serde default, skipped when `None`) and `json_and_hash()`; `src/app.rs` uses the same type as `HeadlessPendingPlan` and the same hash function. Board plan edits (dashboard, CLI, MCP) read the current pending plan and pass its `dynamic_cap` to `edit_plan_from_json_keep_cap`. | `src/board/plan.rs`, `src/board/mod.rs`, `src/board/mcp.rs`, `src/cli.rs`, `src/app.rs`, `src/web/board_api.rs` | FR-3813 | `cargo test --lib tc_3813`, `cargo test --bin dmuxai tc_2942 tc_2550` |
| 3 | `boardCollectSteps(plan)` starts each row from the rendered plan's original step (by `data-step-index`) and new rows default `task` to the first task id. | `src/web/assets/board.js` | FR-3812 | `cargo test --lib tc_3812`, `cargo test --test web_board_e2e -- --ignored tc_3812` |
| 4 | Stale-plan notice is per item (`boardPlanConflictId`): set on 409 before reopening the modal, never reset by the drawer fetch; cleared on close, on opening another item, and on the next successful decision. | `src/web/assets/board.js` | FR-3814 | `cargo test --test web_board_e2e -- --ignored tc_3814 tc_3605` |
| 5 | Tests named in the spec; TC-3605 notice check becomes a hard assert. CHANGELOG and `docs/API.md` notes. | `src/web/board_api.rs`, `src/board/plan.rs`, `src/web/assets.rs`, `tests/web_board_e2e.rs`, `CHANGELOG.md`, `docs/API.md` | all | cargo gates, spec gate |

## Risks

- The WSL fallback trusts `Origin`; it is limited to one listener and one peer
  IP, and still requires the header that board.js sends only from click/key
  handlers. Loopback and remote listeners keep the strict rule.
- Changing `PendingPlanFile` serialization would change plan hashes; the new
  field is skipped when `None`, so existing pending files and board plan copies
  keep their hashes (asserted in TC-3813).

## Rollback

Revert the change. No persisted format is migrated: pending plans written with
`dynamic_cap` were already produced by the headless planner, and plans without
it hash as before.
