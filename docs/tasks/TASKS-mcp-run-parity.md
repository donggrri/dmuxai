# TASKS: MCP run parity with headless CLI run

Status: complete
Owner: DmuxAI
Created: 2026-11-07
Spec: [SPEC-mcp-run-parity](../specs/SPEC-mcp-run-parity.md)
Plan: [PLAN-mcp-run-parity](../plans/PLAN-mcp-run-parity.md)

| Done | # | Task | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| [x] | T1 | Add `model`, `approve_plan`, `allow_sensitive` to `RunArgs` and the `run` tool schema; remove `mode` from the schema. | FR-2450, FR-2451, FR-2452 | `cargo test --lib mcp_stdio_protocol_and_tools` |
| [x] | T2 | Reject any provided `mode` and validate `model` against `agent` / `graph` before calling the backend. | FR-2450, FR-2452 | `cargo test --lib tc_2451_mcp_run_rejects_mode_and_validates_model` |
| [x] | T3 | Forward `--model`, `--approve-plan` and `--allow-sensitive` from `CliMcpBackend::run` via `mcp_run_argv`. | FR-2450, FR-2451 | `cargo test --lib tc_2450_mcp_run_argv_forwards_parity_flags` |
| [x] | T4 | Mark exit code 7 results with `approval_required: true` and `isError: true`. | FR-2453 | `cargo test --lib tc_2452_mcp_run_surfaces_exit_7_approval_required` |
| [x] | T5 | Update `docs/API.md` and `docs/specs/SPEC-agent-api.md` FR-767. | FR-2450..FR-2453 | `bash scripts/check-spec.sh docs/specs/SPEC-mcp-run-parity.md`; manual doc read-through |
| [x] | T6 | Run focused tests and formatting with `CARGO_TARGET_DIR=/workspace/dmux-wt/mcp-run-parity/target`. | All | `cargo fmt --check`; `cargo test --lib mcp_server`; `cargo test --lib cli::tests` |
