# TASKS: coordinator observability

Spec: [SPEC-coordinator-observability.md](../specs/SPEC-coordinator-observability.md) · Plan: [PLAN-coordinator-observability.md](../plans/PLAN-coordinator-observability.md)

- [x] T1 notify_events names — FR-3853 — `cargo test --lib tc_3853`
- [x] T2 event kinds, detail, wait filters — FR-3854 — `cargo test --lib tc_3854`
- [x] T3 `awaiting_approval` once per revision — FR-3851 — `cargo test --lib tc_3851`
- [x] T4 `run_started` in board run, no thread without target — FR-3850, NFR-3850 — `cargo test --lib tc_3850`
- [x] T5 `notify_spec_review_requested` hook — FR-3852 — `cargo test --lib tc_3852`
- [x] T5b `DMUXAI_BOARD_ITEM_ID` on the board run child (internal, scrubbed from agent children); hook prefers that `in_progress` item over `run_ids` — FR-3852 — `cargo test --lib tc_3852`
- [x] T5c Rebase onto SPEC-spec-review-gate: the gate's wait point (`SpecReviewGate::after_node_finished`) calls the hook once per pending round; `spec_review_requested` is the single pending-review notification (`spec_review` kept as a `notify_events` alias) — FR-3852 — `cargo test --lib tc_3852`, `cargo test --lib tc_3711`
- [x] T6 `src/runs_view.rs` shared by HTTP, CLI and MCP — FR-3855, FR-3856 — `cargo test --test serve_runs_api`
- [x] T7 CLI `runs list|show` — FR-3855, FR-3856 — `cargo test --bin dmuxai tc_385`, `cargo test --test run_journal_cli`
- [x] T8 MCP `runs_list`/`runs_show` — FR-3855, FR-3856 — `cargo test --lib -- tc_3855_mcp tc_3856_mcp`
- [x] T9 docs/API.md, README, CHANGELOG — FR-3857 — `cargo test --test spec_index tc_3857`
