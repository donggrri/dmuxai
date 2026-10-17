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

## Slice B (to-do board) and slice C (shared MCP)

Planned; see SPEC sections 4 and 5.
