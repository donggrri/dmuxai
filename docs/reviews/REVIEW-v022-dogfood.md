# REVIEW: v0.2.2 dogfood fixes

Status: approved
Plan: [PLAN-v022-dogfood](../plans/PLAN-v022-dogfood.md) · Tasks: [TASKS-v022-dogfood](../tasks/TASKS-v022-dogfood.md)
Source: dogfooding issues 2-5 in `REVIEW-dashboard-full-parity.md` (PR #68). Issue 1 (text segment separator) shipped in PR #72.

## Spec compliance

| ID | Result | Evidence |
| --- | --- | --- |
| FR-980 | Met. `GrokToolTracker` (`src/grok_activity.rs`) maps `tool_call` / `tool_call_update` to `ActivityItem::Tool`; `run_turn` emits them for the built-in grok preset only. | TC-980, TC-981; live grok-4.7 turn showed `▸ read …`, `▸ run git log --oneline ✓ 0.1s` |
| FR-981 | Met. `AgentView.activity` = TUI activity detail; chip and run pill render it with `textContent`. | TC-982, TC-983; live `/api/state` `activity = "▸ run sleep 20 && echo done"` |
| FR-982 | Met. `cancel_selected` records `TurnEvent::Cancelled` before dropping the active turn and appends one notice; the late adapter event is still ignored. | TC-984, TC-985; live dashboard cancel → `system: grok turn cancelled.`, run state `cancelled` |
| FR-983 | Met. Welcome and empty-conversation help start with `Runners: <picker order>.` | TC-816; live `Runners: cursor, agy, codex, grok.` |
| FR-984 | Met. `Message.local_only`; the dashboard URL notice uses `push_local_notice`; session view, headless output, sharing and last-error skip it. Ids stay index-based. | TC-986, TC-987; live `/api/state` contains the token 0 times (start, running, cancelled, new conversation) |

## Review

Implementation by Grok Build (`grok-4.7`, `--effort medium`, one concern per turn). Code review by opencode `beam-free` (deepseek-v4.1-flash-free, glm-5.3-flash-free, nemotron-3-ultra-free and space-bunny-free timed out at 5 min without a verdict). beam-free: no blockers; findings and resolution:

- MAJOR, test depended on the process-wide registry and test order → fixed: unconditional `install_registry` (same preset as the other installer) plus an `index_of("grok")` assertion.
- MINOR, Windows `\` paths not stripped → fixed, with a test.
- MINOR, detail could exceed 4 KiB by the ellipsis → fixed, test tightened.
- MINOR, cancel notice for a non-visible conversation is transient → accepted: the stored status is `cancelled`, so reopening shows the line.
- MINOR, drag-copy can copy the on-screen token URL → accepted: the URL is meant to be visible and copyable in the TUI.

## Residual risk

- `apply_activity` matches a running row by tool name, so parallel grok calls with the same verb can update rows out of order (content stays correct). Shared with other runners.
- A cancelled turn leaves its last running tool row without a ✓/✗ mark; the cancelled line follows it.
- `tests/mcp_client.rs` `mcp_client_bounds_writes_and_streams_sse` is flaky on `origin/main` too (1 of 6 runs fails); not touched here.
- `grok_live_smoke_headless_turn` uses the 5 s mock deadline and failed once on a slow live reply, then passed.

## Test evidence

- `cargo fmt --all --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings` (Linux and `--target x86_64-pc-windows-gnu`): clean.
- `cargo test --locked --all-targets --no-fail-fast`: all pass except the pre-existing flaky MCP test above.
- Ignored browser tests (`web_workspace_dir`, `web_todo_sidebar`, `web_session_sidebar`, `web_graph_view`, `web_docs_preview`, `web_running`, `web_prompt_center`) with `--ignored`: all pass.
- Live: `dmuxai --web` with isolated `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`DMUXAI_STATE_DIR`, `[graph] enabled = false`, agent `grok`, model `grok-4.7`, two real turns (one completed, one cancelled during `sleep 20`).
