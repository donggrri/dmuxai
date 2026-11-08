# TASKS: Headless graph plan limit and exact-plan approval (#269) — follow-up

| Task | Requirement IDs | Test command |
| --- | --- | --- |
| 1. Add `valid_pending_plan_hash` (16 lowercase hex) in `state_path.rs`; add `tc_2550_approve_plan_id_rejects_non_hex` failing first for `../x`, `ABC`, 15 chars in the CLI parser. | FR-2550 | `cargo test tc_2550_approve_plan_id_rejects_non_hex` |
| 2. Reject non-hex `approve_plan_id` in `parse_run_options` before any filesystem access. | FR-2550 | `cargo test tc_2550_approve_plan_id_rejects_non_hex` |
| 3. Reject non-hex id in `headless_start_approved_plan` with exit 2 before the path is resolved (`tc_2550_headless_approve_plan_id_rejects_non_hex`). | FR-2550 | `cargo test tc_2550_headless_approve_plan_id_rejects_non_hex` |
| 4. Reject non-hex `approve_plan_id` in the MCP `run` tool (`tc_2550_mcp_run_approve_plan_id_rejects_non_hex`). | FR-2550 | `cargo test tc_2550_mcp_run_approve_plan_id_rejects_non_hex` |
| 5. Save the latest revision with `errors: None` in `headless_write_pending_plan` (`tc_2550_write_pending_plan_skips_error_revision`). | FR-2551 | `cargo test tc_2550_write_pending_plan_skips_error_revision` |
| 6. Add TC-2550 to `docs/specs/SPEC-graph-plan-approval-269.md`; run the spec and cargo gates. | — | `scripts/check-spec.sh docs/specs/SPEC-graph-plan-approval-269.md`, `scripts/check-spec.py --all`, `cargo fmt --check`, `cargo test --all-targets`, `cargo clippy --all-targets --all-features -- -D warnings` |
