# TASKS: Local web dashboard

Spec: [SPEC-web-dashboard.md](../specs/SPEC-web-dashboard.md)

## Slice A

| Done | Task | Requirement IDs | Test command |
| --- | --- | --- | --- |
| [x] | Snapshot types and `snapshot_from_run` | FR-525, NFR-521 | `cargo test --lib web::` |
| [x] | `DashboardHub` timings and revisions | FR-526 | `cargo test --lib hub_tracks_node_timings_and_revisions` |
| [x] | Token, URL, bind policy | FR-521, FR-524 | `cargo test --lib token_is_random_hex bind_policy` |
| [x] | Router: auth, Host/Origin, state, SSE, static | FR-522, FR-523, FR-527, FR-529 | `cargo test --lib web::` |
| [x] | File source | FR-528 | `cargo test --lib file_source_picks_latest_valid_run` |
| [x] | Embedded page | FR-529, NFR-520 | `cargo test --lib static_page_is_embedded_and_hardened` |
| [x] | CLI `serve` / `--web` | FR-520, FR-521 | `cargo test --bin dmuxai parse_cli`; `cargo test --test web_serve` |
| [x] | App publishing | FR-530 | `cargo test --bin dmuxai app_publishes_graph_state_to_dashboard` |
| [x] | Docs, TEST-PLAN rows, review | all | `bash scripts/spec-index.sh`; `cargo test --test spec_index` |

## Slice B (to-do board)

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | opencode deepseek | `TodoStore` persistence and ordering | FR-540 | `cargo test --lib todo_store_persists_and_orders` |
| [x] | opencode deepseek | Router: `/api/todos` CRUD + snapshot/SSE wiring | FR-541 | `cargo test --lib todo_routes_crud_and_snapshot` |
| [x] | opencode deepseek | Route validation and error mapping (400/404/415) | FR-541 | `cargo test --lib todo_routes_reject_invalid_requests` |
| [x] | opencode deepseek | Run route + `DashboardServices`/`set_todos` wiring (409 web-mode) | FR-542 | `cargo test --lib todo_run_requires_web_mode` |
| [x] | AGY | App `RunTodo` command handling and run lifecycle | FR-542 | `cargo test --bin dmuxai dashboard_run_todo_with_agent_and_graph` |
| [x] | opencode glm | Page to-do panel (add/edit/done/delete/run controls) | UX-540, FR-541, FR-542 | `node --check src/web/assets/app.js`; `cargo test --lib static_page_has_todo_panel` |
| [x] | opencode glm | Docs: TEST-PLAN rows, tasks, review | FR-540..UX-540 | `bash scripts/spec-index.sh`; `cargo test --test spec_index` |
| [x] | opencode deepseek | Cross-review of AGY's App run lifecycle (11 findings) and verification of the fixes | FR-542 | `cargo test --bin dmuxai dashboard_run_todo` |
| [x] | opencode glm | Cross-review of store/routes (10 findings) and the serve view-only UI (OQ-521) | FR-541, UX-540 | `cargo test --lib web::` |
| [x] | AGY | Fix rounds: turn-keyed linkage, no silent submit, graph failure, shutdown/restart finalization | FR-542 | `cargo test --bin dmuxai dashboard_run_todo` |

## Slice C (shared MCP, import, Jira links)

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | opencode deepseek | `[[mcp.servers]]` config, merge, env-ref resolution, redaction | INT-560 | `cargo test --lib mcp_config_merges_user_and_repo_by_name` |
| [x] | opencode deepseek | Per-runner propagation helpers (opencode, Codex, custom, Cursor guard, AGY notice) | INT-561 | `cargo test --lib mcp::propagate` |
| [x] | opencode glm | Deterministic mock MCP server (stdio + streamable HTTP) and `__mock-mcp` fixtures | NFR-560 | `cargo test --lib mcp::mock` |
| [x] | AGY | MCP client (stdio + streamable HTTP), adapter hooks, `__mock-mcp`, wiring | INT-562, INT-563 | `cargo test --test mcp_client` |
| [x] | AGY | Client hardening: no redirects, process groups, bounded writes, streamed SSE, no secrets in Codex argv, once-per-session Cursor notice | NFR-561 | `cargo test --test mcp_client`; `cargo test --bin dmuxai codex_mcp_secrets_not_in_argv` |
| [x] | opencode deepseek | Registry, `/api/mcp/servers`, `/api/import`, Jira link/status/assignee fields | FR-561..FR-564 | `cargo test --lib import_` |
| [x] | opencode glm | Import panel, Jira ticket strip (mockup #3), README "Shared MCP servers" | UX-560, UX-561 | `cargo test --lib web::assets` |
| [x] | AGY | End-to-end serve + mock import, startup `jira_base_url` | INT-563, FR-564 | `cargo test --test web_mcp_import` |
| [x] | glm / deepseek | Cross-reviews (glm on deepseek: 11 findings; deepseek on AGY: 9 findings) and fix rounds | all | `cargo test --all-targets` |
