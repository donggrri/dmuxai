# SPEC-mcp-injection

Status: Removed (superseded by SPEC-mcp-propagation-removal, 2026-10-24)

> **Removed (2026-10-24):** Superseded by [SPEC-mcp-propagation-removal](SPEC-mcp-propagation-removal.md). DmuxAI no longer passes MCP servers to any runner; every agent configures MCP in its own configuration.

Owner: dmuxai
Created: 2026-10-24
Baseline: main
Related: #113, refactor scan finding #4, architecture audit PR-7, superseded by SPEC-mcp-propagation-removal

## 1. Decision summary

Model Context Protocol (MCP) server injection was previously centralized into a dedicated seam module (`src/mcp/inject.rs`, lib crate `dmuxai::mcp::inject::RunnerMcp`). As of 2026-10-24 (Dongjin decision), runner MCP propagation has been completely removed in favor of agent-native configuration. See [SPEC-mcp-propagation-removal](SPEC-mcp-propagation-removal.md).

## 2. Scope

### In scope

- Historical record of runner-specific MCP injection and applicability filtering.
- Replaced by [SPEC-mcp-propagation-removal](SPEC-mcp-propagation-removal.md).

### Out of scope / non-goals

- Restoring runner MCP injection or propagation.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1980 | Adapter injection output formatting remains strictly backwards-compatible with existing runners (superseded: propagation removed). | dmuxai | Part A implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| `RunnerMcp` | (Removed) Container holding resolved MCP servers applicable to a single runner, filtered once at spawn. | Unfiltered `ResolvedMcpServer` collection. |
| Injection seam | (Removed) Abstraction layer transforming filtered MCP definitions into runner-specific delivery payloads. | Ad-hoc per-runner propagation logic. |
| Cursor legacy mode | Cursor execution via stream-json CLI where workspace configuration files were previously generated. | Cursor ACP execution via JSON-RPC protocol. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1980 | Removed | Given a resolved MCP server list and a target runner name | **Removed** (2026-10-24): superseded by SPEC-mcp-propagation-removal FR-2081 and FR-2085. | — | TC-2081, TC-2085 |
| FR-1981 | Removed | Given a `RunnerMcp` instance | **Removed** (2026-10-24): superseded by SPEC-mcp-propagation-removal FR-2084 and FR-2085. | — | TC-2084, TC-2085 |
| FR-1982 | Removed | Given a Cursor legacy turn with applicable MCP servers | **Removed** (2026-10-24): superseded by SPEC-mcp-propagation-removal FR-2084. | — | TC-2084 |
| FR-1983 | Removed | Given a Cursor ACP turn with applicable MCP servers | **Removed** (2026-10-24): superseded by SPEC-mcp-propagation-removal FR-2080. | — | TC-2080 |
| FR-1984 | Removed | Given the repository source tree | **Removed** (2026-10-24): superseded by SPEC-mcp-propagation-removal FR-2085. | — | TC-2085 |

## 5. Acceptance scenarios and test design

All prior acceptance tests (TC-1980..TC-1984) were deleted in PR-9 part A with the removal of propagation modules. Replaced by TC-2080..TC-2085 in [SPEC-mcp-propagation-removal](SPEC-mcp-propagation-removal.md).

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Injection seam | Runner MCP injection removed; agents configure MCP natively | Superseded by SPEC-mcp-propagation-removal | TC-2084, TC-2085 |
| Secret isolation | Secrets never passed to runner processes | Preserves security and sanitization invariants | TC-2084 |
| Cursor legacy injection | Cursor legacy turns no longer receive or write MCP configurations | Managed by user in native agent config | TC-2084 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Superseded and removed by PR-9 Part A & B | `src/mcp/*`, `src/agent/*` | FR-1980..FR-1984 | `cargo test tc_2080`, `tc_2084`, `tc_2085` |

## 8. Review and test exit criteria

- [x] All requirements marked removed and superseded by SPEC-mcp-propagation-removal.
- [x] Deleted tests verified removed from test plan.
- [x] Spec check allowlisted as superseded spec without active TC rows.
