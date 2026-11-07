# PLAN: MCP run parity with headless CLI run

Status: complete
Owner: DmuxAI
Created: 2026-11-07
Spec: [SPEC-mcp-run-parity](../specs/SPEC-mcp-run-parity.md)
Related: SPEC-agent-api FR-767

## Goal

Make the MCP `run` tool accept and forward the headless CLI run controls
(`model`, `approve_plan`, `allow_sensitive`), reject the removed `mode`
argument, and mark approval exit code 7 explicitly.

## Ordered changes

1. `src/mcp_server.rs`
   - Replace `RunArgs.mode` / `RunArgs.read_only` with `model`,
     `approve_plan` and `allow_sensitive` (FR-2450, FR-2451).
   - Add the fields to the `run` input schema and remove `mode` (FR-2452).
   - Reject any `mode` key before building `RunArgs`, using the CLI's
     removed-mode wording (FR-2452).
   - Validate `model` like `parse_run_options`: graph+agent conflict first,
     then `model` + `graph`, then `model` without `agent` (FR-2450).
   - In `tool_result`, set `approval_required: true` and `isError: true` when a
     run outcome has exit code 7 (FR-2453).
2. `src/cli.rs`
   - Extract `mcp_run_argv(&RunArgs)` and forward `--model`,
     `--approve-plan` and `--allow-sensitive` for both `run` and
     `todo run` (FR-2450, FR-2451).
3. Tests
   - MCP stub tests for field capture, validation, mode rejection, schema and
     exit 7 (TC-2450..TC-2453).
   - CLI argv test for the exact child flags (TC-2450).
4. Documentation
   - Update `docs/API.md` §3 and the run notes.
   - Update `docs/specs/SPEC-agent-api.md` FR-767 and the internal-flag note.

## Risks

- A client that still sends `mode` now gets a tool error instead of a read-only
  run. This is intentional and matches the CLI removal; the error names the
  removed mode and tells the client to omit it.
- Adding `approval_required` to the MCP result is additive; the underlying CLI
  JSON keys are unchanged.
- Validation order mirrors the CLI; adding test coverage prevents drift.

## Rollback

Revert the changes to `src/mcp_server.rs`, `src/cli.rs` and the docs. There is
no persistence or protocol migration to undo; the previous MCP schema only
loses the new fields, and old clients that omit them keep working with the
defaults (`model = null`, both booleans false).
