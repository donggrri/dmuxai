# TASKS: Board web fixes (#358, #359, #360, #369)

Spec: [../specs/SPEC-board-web-fixes.md](../specs/SPEC-board-web-fixes.md) ·
Plan: [../plans/PLAN-board-web-fixes.md](../plans/PLAN-board-web-fixes.md)

| Task | Requirement IDs | Test command | Done |
| --- | --- | --- | --- |
| 1. Pass the listener `WebAccess` into `board_api::router`; owner writes read the peer `ConnectInfo` and apply the WSL-listener fallback in `human_browser_rejection` (`tc_3810_wsl_listener_human_headers_fallback`, `tc_3811_loopback_keeps_strict_human_rule`). | FR-3810, FR-3811, NFR-3810 | `cargo test --lib tc_3810 tc_3811` | [x] |
| 2. Add `dynamic_cap` and `json_and_hash()` to `PendingPlanFile`; alias `HeadlessPendingPlan` to it in `src/app.rs`; keep `dynamic_cap` in dashboard/CLI/MCP plan edits (`tc_3813_dynamic_cap_hash_roundtrip`). | FR-3813 | `cargo test --lib tc_3813`; `cargo test --bin dmuxai tc_2942 tc_2550` | [x] |
| 3. `boardCollectSteps` keeps original step fields; new steps get the first task id (`tc_3812_board_save_keeps_step_fields`, `tc_3812_board_save_plan_keeps_task_and_sensitive`). | FR-3812 | `cargo test --lib tc_3812`; `cargo test --test web_board_e2e -- --ignored tc_3812` | [x] |
| 4. Per-item stale-plan notice that survives drawer refreshes (`tc_3814_stale_approve_shows_notice`); hard notice assert in `tc_3605_auto_run_opt_out_and_stale_plan_hash`. | FR-3814 | `cargo test --test web_board_e2e -- --ignored tc_3814 tc_3605` | [x] |
| 5. CHANGELOG (Unreleased, Fixed), `docs/API.md` owner-write note, spec status, review. | — | `bash scripts/check-spec.sh docs/specs/SPEC-board-web-fixes.md`, `python3 scripts/check-spec.py --all`, `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets` | [x] |
