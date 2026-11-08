# PLAN: Headless graph plan limit and exact-plan approval (#269) — follow-up

Covers FR-2550 and FR-2551 from
[../specs/SPEC-graph-plan-approval-269.md](../specs/SPEC-graph-plan-approval-269.md).
FR-2542..FR-2547 and FR-2548 are already shipped (`bd89987`).

## Order of work

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add `dmuxai::state_path::valid_pending_plan_hash` (16 lowercase hex chars, exact length). Add regression test `tc_2550_write_pending_plan_skips_error_revision` that fails before the FR-2551 fix. | `src/state_path.rs`, `src/app.rs` tests | FR-2550, FR-2551 | `cargo test tc_2550` |
| 2 | Reject non-hex `--approve-plan-id` in the CLI parser with a usage error, before any other filesystem access. | `src/cli.rs` | FR-2550 | `cargo test tc_2550_approve_plan_id_rejects_non_hex` |
| 3 | Reject non-hex ids in `headless_start_approved_plan` with exit 2 before resolving the path. | `src/app.rs` | FR-2550 | `cargo test tc_2550_headless_approve_plan_id_rejects_non_hex` |
| 4 | Reject non-hex `approve_plan_id` in the MCP `run` tool argument validation. | `src/mcp_server.rs` | FR-2550 | `cargo test tc_2550_mcp_run_approve_plan_id_rejects_non_hex` |
| 5 | `headless_write_pending_plan` picks the latest revision with `errors: None` (same revision approval runs). | `src/app.rs` | FR-2551 | `cargo test tc_2550_write_pending_plan_skips_error_revision` |
| 6 | Update spec test table; run spec gate, fmt, tests, clippy. | `docs/` | — | `scripts/check-spec.sh`, `scripts/check-spec.py --all`, cargo gates |

## Risks

- The validation message wording is locked by the shared tests; changing it
  would break CLI/MCP parity assertions.
- `headless_write_pending_plan` must not change the hash format (sha256, first
  16 lowercase chars) or saved pending plans become unreachable.

## Rollback

Revert the single commit; no schema, persistence format, or provider protocol
change is involved.
