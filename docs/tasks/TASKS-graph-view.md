# TASKS: Dashboard node-graph view (slice H)

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | opencode deepseek | `layout_graph` + tests | FR-700 | `cargo test --lib web::layout` |
| [x] | opencode deepseek | Snapshot deps/layout/layers/detail, served state | FR-701, NFR-700 | `cargo test --lib snapshot_exposes_deps_and_layout`; `cargo test --test web_serve` |
| [x] | AGY | Node detail (log tail, changed files) | FR-702 | `cargo test --bin dmuxai dashboard_nodes_carry_log_tail_and_changed_files` |
| [x] | opencode deepseek | Subway-map Graph view, toggle, side panel, pan/zoom | UX-700..UX-703 | `cargo test --lib graph_view_svg_structure` |
| [x] | AGY | Headless browser check | UX-700..UX-703 | `cargo test --test web_graph_view -- --ignored` |
| [x] | cursor (composer-2.5) | Cross-review of UI and detail tracking (glm unavailable: provider errors, then quota) | all | `cargo test --all-targets` |
| [x] | — | README screenshot `docs/images/web-graph-view.png` | UX-700 | — |
