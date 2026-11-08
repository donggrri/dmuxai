# REVIEW: Headless graph plan limit and exact-plan approval (#269) — follow-up

Scope: FR-2550 (hex-only `approve_plan_id`, usage exit 2 before filesystem
access) and FR-2551 (exit 7 saves the latest revision without validation
errors). FR-2542..FR-2548 were reviewed with the shipped commit (`bd89987`).

## Spec compliance

| Requirement | Verdict | Evidence |
| --- | --- | --- |
| FR-2550 | Met | `state_path::valid_pending_plan_hash` (16 lowercase hex, exact length) rejects `../x` (path shaping), uppercase, 15/17-char and non-hex ids. Enforced in `parse_run_options` (first mandatory check, before the graph check), `headless_start_approved_plan` (exit 2 before the path is resolved) and MCP `run`. No provider request is sent and no file under `graph-pending` is created or read for a rejected id. |
| FR-2551 | Met | `headless_write_pending_plan` selects `revisions.iter().rev().find(|revision| revision.errors.is_none())`, the same revision `approve_awaiting_plan` would execute. |
| Non-goals | Respected | No change to the hash format (sha256, first 16 lowercase chars), the pending-file layout, `--approve-plan` behavior, or MCP forwarding of a valid id. `tc_2549` was updated only because its `"abc123"` fixture no longer matches the tightened id contract. |

## Commands and results

- `cargo build` — ok
- `cargo test tc_254` / `cargo test tc_2550` — all pass (TC-2541..TC-2549, and
  `tc_2550_approve_plan_id_rejects_non_hex`,
  `tc_2550_mcp_run_approve_plan_id_rejects_non_hex`,
  `tc_2550_headless_approve_plan_id_rejects_non_hex`,
  `tc_2550_write_pending_plan_skips_error_revision`,
  `valid_pending_plan_hash_accepts_only_16_lowercase_hex`)
- `cargo test --all-targets` — 0 failures across all suites
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo fmt` / `cargo fmt --check` — clean
- `bash scripts/check-spec.sh docs/specs/SPEC-graph-plan-approval-269.md` — PASS
- `python3 scripts/check-spec.py --all` — 93/93 pass, 0 allowlisted

## Residual risk / untested paths

- exit 7 with *only* invalid revisions (every `errors: Some`) hits the `?`
  fallback and saves nothing; behavior accepted, not separately asserted.
- MCP clients that spawn the CLI themselves hit the same CLI-parser rejection;
  the MCP-side check is defense in depth.
- No live-provider run needed; all tests are deterministic.
