# SPEC: MCP run parity with headless CLI run

Status: complete
Owner: DmuxAI
Created: 2026-11-07
Baseline: `b1ff818`
Related: SPEC-agent-api FR-767, SPEC-plan-gate FR-947/FR-1818/FR-1819, SPEC-headless-gate-env FR-1825

## 1. Decision summary

Agents that call DmuxAI through `dmuxai mcp` must be able to express the same
headless run controls as `dmuxai run`: model selection, plan approval and
sensitive-gate confirmation. The MCP `run` tool therefore forwards `model`,
`approve_plan` and `allow_sensitive` with the CLI's validation, rejects the
removed legacy `mode` argument instead of silently interpreting it, and makes an
approval exit code 7 explicit in the tool result.

## 2. Scope

### In scope

- MCP `run` accepts `model`, `approve_plan` and `allow_sensitive` and forwards
  them to the child `dmuxai run` / `dmuxai todo run`.
- `model` requires `agent` and cannot be combined with `graph`.
- Any provided `mode` is rejected with the same removed-mode wording used by
  the CLI.
- Exit code 7 is surfaced as an error result with an explicit
  `approval_required: true` marker while preserving the run JSON.
- Documentation of the MCP tool contract.

### Out of scope / non-goals

- Changing CLI `run` parsing, exit codes, or gate behaviour.
- Adding read-only or progress controls to the MCP `run` tool.
- Real provider/model calls in tests.
- HTTP API `POST /api/prompt` changes.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2509 | `mode` remains removed everywhere; MCP rejects it rather than keeping a compatibility shim. Decided. | DmuxAI | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| MCP `run` | The `run` tool served by `dmuxai mcp` over stdio | Headless `dmuxai run` process |
| Approval-required result | A run outcome with `ok: false` and `exit_code: 7` | Generic exit 1 failure |
| Forwarded field | An MCP argument converted to the child CLI flag of the same meaning | A new run mode |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2509 | Must | Given an MCP `run` call with `model` | Then the child `dmuxai run` / `dmuxai todo run` receives `--model <value>` and the result is the CLI run JSON | `model` without `agent` is a tool error `model needs agent`; `model` with `graph` is a tool error `model cannot be used with graph`; no child starts | TC-2509 |
| FR-2510 | Must | Given an MCP `run` call with `approve_plan` and/or `allow_sensitive` | Then the child receives `--approve-plan` and/or `--allow-sensitive` exactly when the booleans are true | Omitted or false fields add no flag | TC-2509 |
| FR-2511 | Must | Given an MCP `run` call that includes a `mode` key | Then the call is rejected before any child starts with the removed-mode message (`mode was removed; DmuxAI runs a single agent mode (write-capable). Omit mode.`); `tools/list` does not advertise `mode` | This applies to every value, including `ask`, `plan`, `implement` and `agent`; `--mode` and `--read-only` are never passed to the child | TC-2510 |
| FR-2512 | Must | Given an MCP `run` whose child exits `7` | Then the tool result has `isError: true` and `structuredContent.approval_required: true` while keeping `exit_code: 7`, `error`, `text` and any `plan`; a successful run has no `approval_required` marker | Exit 7 is still the same run JSON documented for `run --json`; clients can branch on either the marker or the exit code | TC-2511 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2509 | FR-2509, FR-2510 | unit | Stub MCP backend, no provider | `tools/call` run with prompt + agent + model + both booleans, then a to-do run | Backend receives the exact fields; `mcp_run_argv` emits the matching `--model`, `--approve-plan`, `--allow-sensitive` flags for `run` and `todo run` | `src/mcp_server.rs` `tc_2509_mcp_run_forwards_parity_fields`; `src/cli.rs` `tc_2509_mcp_run_argv_forwards_parity_flags` |
| TC-2510 | FR-2511 | unit | Stub MCP backend, no provider | Call `run` with `mode` values and with invalid model combinations | Tool error for every `mode`; `model needs agent` and `model cannot be used with graph`; no backend run recorded | `src/mcp_server.rs` `tc_2510_mcp_run_rejects_mode_and_validates_model` |
| TC-2511 | FR-2512 | unit | Stub returning exit 7, then a success | Call `run` | Exit 7 result has `isError` and `approval_required: true`; success result omits the marker | `src/mcp_server.rs` `tc_2511_mcp_run_surfaces_exit_7_approval_required` |
| TC-2512 | FR-2511 | unit | `tools/list` | Inspect the `run` input schema | `model`, `approve_plan`, `allow_sensitive` are present; `mode` is absent | `src/mcp_server.rs` `tc_2512_mcp_run_schema_advertises_parity_fields_and_omits_mode` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema or storage change. | MCP run results keep the CLI JSON shape. | TC-2511 |
| Provider protocol | MCP JSON-RPC tool arguments only; no provider protocol change. | Clients that still send `mode` receive a clear tool error instead of a silent policy change. | TC-2510 |
| Security / permissions | Approval and sensitive flags are explicit opt-ins; omitting them keeps exit 7. | Rollback removes the new fields and restores the previous tool schema. | TC-2509, TC-2511 |
| Platform | Child process spawning is unchanged. | No platform-specific behavior added. | TC-2509 |

## 7. Implementation plan

See [PLAN-mcp-run-parity](../plans/PLAN-mcp-run-parity.md) and
[TASKS-mcp-run-parity](../tasks/TASKS-mcp-run-parity.md).

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add MCP run fields, schema, validation and mode rejection | `src/mcp_server.rs` | FR-2509, FR-2510, FR-2511 | TC-2509, TC-2510, TC-2512 |
| 2 | Forward the new fields to the child CLI | `src/cli.rs` | FR-2509, FR-2510 | TC-2509 |
| 3 | Surface exit 7 explicitly | `src/mcp_server.rs` | FR-2512 | TC-2511 |
| 4 | Update the API/MCP docs | `docs/API.md`, `docs/specs/SPEC-agent-api.md` | FR-2509..FR-2512 | manual review |

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, all targets test, and clippy.
- [x] External-provider smoke tests are not needed; tests use stubs and argv assertions.
- [x] `../reviews/REVIEW-mcp-run-parity.md` records commands, result, residual risk, and rollback notes.
