# Review: workspace directory (#56)

Date: 2026-10-18 (KST)
Reviewer: opencode-haiku (two rounds)
Scope: `src/workspace.rs`, `src/web/server.rs` (workspace API), `src/main.rs`, `src/web/hub.rs`, `src/app.rs`

## Round 1: REQUEST_CHANGES

| # | Finding | Resolution |
| --- | --- | --- |
| 1 | switch-pending can leak: the 10s timeout, a dropped reply and a dropped client future all skip `CancelSwitchWorkspace`, so prompts stay refused until restart | Fixed. The post-validation work runs in a spawned task, and the timeout or dropped-reply path sends cancel (with its own send timeout). Test: `workspace_switch_timeout_sends_cancel` |
| 2 | `validate` works in view-only mode and outside the browse roots, and writes a probe | Kept as specified (FR-872): it needs the token, returns ok or a reason only, and uses the same probe as the CLI. Documented in the SPEC implementation decisions |
| 3 | `GET /api/workspace` writes probe files while it validates the recent paths | Fixed. Uses `validate_workspace_dir_no_probe`. Test: `load_recent_workspaces_does_not_write_probes` (fails without the fix) |
| - | Preflight failure still answered 202 | Fixed. It now answers 500 with the notice text |

## Round 2: REQUEST_CHANGES, then addressed

- Regression tests for both fixes: added (above).
- Unbounded cancel send: wrapped in a timeout.
- API contract changes (504 / 500 / no-probe recents): documented in SPEC-workspace-dir.md under "Implementation decisions".

## Also fixed before review

- Clippy on Linux and the Windows target: needless returns in `async_main`, `&PathBuf` changed to `&Path`, an unused `Stdio` import on Windows, and `await_holding_lock` allowed in two env-locked tests.
