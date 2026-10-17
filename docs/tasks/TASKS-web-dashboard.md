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
| [ ] | opencode deepseek | `TodoStore` persistence and ordering | FR-540 | `cargo test --lib todo_store_persists_and_orders` |
| [ ] | opencode deepseek | Router: `/api/todos` CRUD + snapshot/SSE wiring | FR-541 | `cargo test --lib todo_routes_crud_and_snapshot` |
| [ ] | opencode deepseek | Route validation and error mapping (400/404/415) | FR-541 | `cargo test --lib todo_routes_reject_invalid_requests` |
| [ ] | opencode deepseek | Run route + `DashboardServices`/`set_todos` wiring (409 web-mode) | FR-542 | `cargo test --lib todo_run_requires_web_mode` |
| [ ] | AGY | App `RunTodo` command handling and run lifecycle | FR-542 | `cargo test --bin dmuxai dashboard_run_todo_with_agent_and_graph` |
| [ ] | opencode glm | Page to-do panel (add/edit/done/delete/run controls) | UX-540, FR-541, FR-542 | `node --check src/web/assets/app.js`; `cargo test --lib static_page_has_todo_panel` |
| [ ] | opencode glm | Docs: TEST-PLAN rows, tasks, review | FR-540..UX-540 | `bash scripts/spec-index.sh`; `cargo test --test spec_index` |

## Slice C (shared MCP)

Planned; see SPEC section 4 (INT-560..NFR-560).
