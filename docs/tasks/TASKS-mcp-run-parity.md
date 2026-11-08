# TASKS: MCP run parity with headless CLI run

Status: complete
Owner: DmuxAI
Created: 2026-11-07
Spec: [SPEC-mcp-run-parity](../specs/SPEC-mcp-run-parity.md)
Plan: [PLAN-mcp-run-parity](../plans/PLAN-mcp-run-parity.md)

| Done | # | Task | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| [x] | T1 | Add `model`, `approve_plan`, `allow_sensitive` to `RunArgs` and the `run` tool schema; remove `mode` from the schema. | FR-2509, FR-2510, FR-2511 | `cargo test --lib mcp_stdio_protocol_and_tools` |
| [x] | T2 | Reject any provided `mode` and validate `model` against `agent` / `graph` before calling the backend. | FR-2509, FR-2511 | `cargo test --lib tc_2510_mcp_run_rejects_mode_and_validates_model` |
| [x] | T3 | Forward `--model`, `--approve-plan` and `--allow-sensitive` from `CliMcpBackend::run` via `mcp_run_argv`. | FR-2509, FR-2510 | `cargo test --bin dmuxai tc_2509_mcp_run_argv_forwards_parity_flags` |
| [x] | T4 | Mark exit code 7 results with `approval_required: true` and `isError: true`. | FR-2512 | `cargo test --lib tc_2511_mcp_run_surfaces_exit_7_approval_required` |
| [x] | T5 | Update `docs/API.md` and `docs/specs/SPEC-agent-api.md` FR-767. | FR-2509..FR-2512 | `bash scripts/check-spec.sh docs/specs/SPEC-mcp-run-parity.md`; manual doc read-through |
| [x] | T6 | Run focused tests and formatting. | All | `cargo fmt --check`; `cargo test --lib mcp_server`; `cargo test --bin dmuxai cli::tests`; `cargo test --bin dmuxai agent::custom::tests::tc_2084_no_runner_gets_mcp_args_or_files` |
