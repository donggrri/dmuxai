# TASKS: runner status and enable switch

Spec: [SPEC-runner-status.md](../specs/SPEC-runner-status.md) · Plan: [PLAN-runner-status.md](../plans/PLAN-runner-status.md)

- [ ] T1 probe core `src/runner_status.rs` — FR-3300..FR-3305, NFR-3311 — `cargo test --lib runner_status`
- [ ] T2 enabled setting, TOML editor, admission — FR-3307, FR-3308, NFR-3310 — `cargo test --lib settings agent_api`
- [ ] T3 CLI — FR-3306, FR-3308 — `cargo test --lib cli::`
- [ ] T4 HTTP and MCP — FR-3309, FR-3310 — `cargo test --lib web::server mcp_server`
- [x] T5 dashboard panel — FR-3311 — `cargo test --lib web::assets`
