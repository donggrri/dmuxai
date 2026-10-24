# SPEC-mcp-injection

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: main
Related: #113, refactor scan finding #4, architecture audit PR-7

## 1. Decision summary

Model Context Protocol (MCP) server injection is centralized into a dedicated seam module (`src/mcp/inject.rs`, lib crate `dmuxai::mcp::inject::RunnerMcp`). Today, runner adapters individually inspect resolved MCP servers, duplicate runner applicability filtering across ad-hoc call sites (`applies_to`), and the Cursor legacy runner writes ephemeral `.cursor/mcp.json` configuration directly into the workspace root. This causes dirty workspace states (#113 "Cursor question turn shows an 'Edited .cursor/mcp.json' file card").

PR-7 establishes `RunnerMcp` as the single injection contract across all runners. Runner applicability filtering is performed exactly once at adapter instantiation. In part A, codex, custom (including builtin grok via ACP), and the agy notice in `src/app.rs` are converted to `RunnerMcp`, eliminating their ad-hoc `applies_to` checks while preserving byte-for-byte identical injection outputs.

## 2. Scope

### In scope

- Unified `RunnerMcp` interface encapsulating filtered servers for a specific runner (`for_runner`).
- Single point of evaluation for runner applicability (`applies_to` called only in `RunnerMcp::for_runner`).
- Adapters for runner-native MCP injection forms: OpenCode JSON config merge, Codex command-line overrides and child environment, custom agent per-turn JSON config, ACP `mcpServers` JSON-RPC array, AGY startup notice, and Cursor legacy notice.
- Refactoring `src/agent/codex.rs`, `src/agent/custom.rs`, and `src/app.rs` to consume `RunnerMcp`.
- Byte-for-byte identical injection outputs for Codex, OpenCode, Custom, ACP, and AGY.

### Out of scope / non-goals

- Cursor runner overhaul (Part B: legacy notice emission, removal of workspace `.cursor/mcp.json` writes and `CursorMcpGuard`, prewarm adjustments, and Cursor ACP migration).
- Changes to MCP configuration resolution or schema (`src/mcp/config.rs`).
- Dynamic runtime reconfiguration of MCP servers during active adapter turns.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1980 | Adapter injection output formatting remains strictly backwards-compatible with existing runners. | dmuxai | Part A implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| `RunnerMcp` | Container holding resolved MCP servers applicable to a single runner, filtered once at spawn. | Unfiltered `ResolvedMcpServer` collection. |
| Injection seam | Uniform abstraction layer transforming filtered MCP definitions into runner-specific delivery payloads. | Ad-hoc per-runner propagation logic. |
| Cursor legacy mode | Cursor execution via stream-json CLI where workspace configuration files were previously generated. | Cursor ACP execution via JSON-RPC protocol. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1980 | Must | Given a resolved MCP server list and a target runner name | `RunnerMcp::for_runner(all, runner)` filters servers by runner applicability exactly once using case/whitespace-insensitive matching (None applies to all) | Returns empty `RunnerMcp` when no servers match | TC-1980 |
| FR-1981 | Must | Given a `RunnerMcp` instance | Adapter methods (`opencode_config`, `codex_overrides`, `custom_json`, `acp_servers`, `agy_notice`) produce outputs byte-for-byte identical to existing `propagate.rs` builders | Empty servers yield empty overrides/arrays or None notices matching builder behavior | TC-1981 |
| FR-1982 | Must | Given a Cursor legacy turn with applicable MCP servers | DmuxAI no longer writes `<workspace>/.cursor/mcp.json`; the legacy notice is emitted exactly once per adapter session; prewarm runs regardless of MCP servers | No `.cursor/` directory is created; empty server lists emit no notice | TC-1982 |
| FR-1983 | Must | Given a Cursor ACP turn with applicable MCP servers | MCP servers are passed in `session/new` via `mcpServers` (using `RunnerMcp::acp_servers()`) | Missing/empty servers still send an empty `mcpServers` array | TC-1983 |
| FR-1984 | Must | Given the repository source tree | No `.applies_to(` call remains outside `src/mcp/` | A unit test fails the build if any such call is introduced | TC-1984 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1980 | FR-1980 | unit | Resolved MCP servers with various runner lists (None, single runner, multiple runners with whitespace/case variations, disjoint runners) | Call `RunnerMcp::for_runner` | Contains exactly the matching servers, preserves runner name, reports `is_empty()` accurately | `src/mcp/inject.rs` `tc_1980_for_runner_filters_once` |
| TC-1981 | FR-1981 | unit | Configured MCP servers (stdio with args/env, http with headers) and base config documents | Call each `RunnerMcp` adapter method and corresponding `propagate.rs` builder | Adapter method outputs equal previous builder outputs byte-for-byte, including empty server lists | `src/mcp/inject.rs` `tc_1981_adapter_outputs_equal_propagate_builders` |
| TC-1982 | FR-1982 | integration | Fake Cursor binary, legacy protocol, workspace with applicable MCP servers | Run two turns | No `.cursor/` directory is created; the legacy notice is emitted exactly once across both turns | `src/agent/cursor.rs` `tc_1982_cursor_legacy_leaves_no_cursor_dir_and_notice_once` |
| TC-1983 | FR-1983 | integration | Fake ACP agent with `FAKE_ACP_MCP_CAPTURE`, Cursor ACP protocol, applicable MCP servers | Run one ACP turn | Captured `mcpServers` contains the applicable server | `src/agent/cursor.rs` `tc_1983_cursor_acp_sends_mcp_servers` |
| TC-1984 | FR-1984 | unit | Repository `src/**/*.rs` excluding `src/mcp/` | Scan source text for `.applies_to(` | No occurrences found | `src/mcp/inject.rs` `tc_1984_no_applies_to_outside_mcp` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Injection seam | `RunnerMcp` is the exclusive channel delivering MCP servers to runner adapters | Unchanged payload generation maintains full runner compatibility | TC-1980, TC-1981 |
| Secret isolation | Secrets continue passing solely through per-run files or child environment, never via command arguments | Preserves existing security and sanitization invariants | TC-1981 |
| Cursor legacy injection | **Behavior change:** Cursor legacy turns no longer write `<workspace>/.cursor/mcp.json`; workspace stays clean | Cursor legacy loses automatic MCP propagation; users add servers to `~/.cursor/mcp.json` or switch to `protocol = "acp"` | TC-1982 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | MCP injection seam and runner filtering (`RunnerMcp`, codex, custom, ACP, agy) | `src/mcp/inject.rs`, `src/mcp/mod.rs`, `src/mcp/propagate.rs`, `src/agent/codex.rs`, `src/agent/custom.rs`, `src/app.rs` | FR-1980, FR-1981 | `tc_1980`, `tc_1981` |
| 2 | Cursor legacy turns emit notice once and leave no workspace .cursor/ | `src/mcp/propagate.rs`, `src/agent/cursor.rs` | FR-1982 | `tc_1982` |
| 3 | Cursor ACP passes MCP servers via session params | `src/agent/cursor.rs` | FR-1983 | `tc_1983` |
| 4 | Enforce no `applies_to(` calls outside `src/mcp` | `src/**/*.rs` | FR-1984 | `tc_1984` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-1980..FR-1981) map to passing test evidence (TC-1980..TC-1981).
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [x] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
