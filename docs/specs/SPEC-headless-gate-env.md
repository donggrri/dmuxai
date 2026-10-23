# SPEC: Headless sensitive gate and inherited MCP env

Status: approved  
Owner: dmuxai  
Baseline: `main`

## Problem

Headless `dmuxai run --agent ID --approve-plan` still exits 7 when the prompt
contains sensitive keywords (#110). `sensitive_keyword_match` false-positives on
substrings and quoted/code text. Custom agent children can inherit
`DMUXAI_MCP_CONFIG` from the parent when no MCP is configured for the turn
(#118).

## 4. Requirements

| ID | Priority | Given | Then | Edge cases | Tests |
| --- | --- | --- | --- | --- | --- |
| FR-1660 | Must | `dmuxai run <prompt> --agent ID --approve-plan` headless | The single explicit-agent step’s sensitive gate is auto-confirmed and the run continues | Without `--approve-plan`, exit 7 at the gate is unchanged; judge path without `--agent` is unchanged | TC-1660 |
| FR-1661 | Must | `dmuxai run` with `--allow-sensitive` | Sensitive gates are auto-confirmed in headless runs (direct `--agent` and judge/graph paths) | Does not replace plan approval; `--approve-plan` still required for complex plans | TC-1661 |
| FR-1662 | Must | User prompt or plan step `job` text | `sensitive_keyword_match` ignores backticks, double quotes, and fenced ``` blocks; matches whole words only (`auth`, `login` yes; `authors`, `dbg` no) | Existing keyword list semantics preserved for standalone words | TC-1662 |
| FR-1663 | Must | A custom or built-in agent child process with no MCP config for the turn | `DMUXAI_MCP_CONFIG` is removed from the child environment so a parent value never leaks | When MCP is configured for the turn, `DMUXAI_MCP_CONFIG` is set as today | TC-1663 |
| FR-1664 | Must | `dmuxai --help` | Documents `--allow-sensitive` on the `run` usage line and exit-code note for sensitive gates | — | TC-1664 |
| FR-1665 | Should | README headless `dmuxai run` section | Documents `--allow-sensitive`, and that `--agent` with `--approve-plan` confirms the sensitive gate on a one-step run | — | TC-1665 |
| FR-1825 | Must | Headless `dmuxai run` reaching a sensitive (`WaitingGate`) gate | The process exits 7 unless `--allow-sensitive` or `--agent` with `--approve-plan` auto-confirms it | Does not replace plan approval | TC-1825 |
| FR-1826 | Must | `dmuxai run` with `--allow-sensitive` in headless mode | Sensitive gates are auto-confirmed on direct, judge, and graph paths | Plan approval still required for complex plans | TC-1826 |
| FR-1827 | Must | The same sensitive gate in dashboard (`--web`/TUI) mode | The gate pauses and `/graph continue` resumes it; there is no CLI-flag bypass | view-only `serve` cannot run agents | TC-1827 |

### Code-vs-spec mismatches

| ID | Spec says | Code does (file:function) | Proposed resolution |
| --- | --- | --- | --- |
| OQ-1812 | Headless sensitive gate auto-confirm requires `--approve-plan` with `--agent` | `src/app.rs` `headless_idle_action` auto-confirms when `req.allow_sensitive || (req.agent.is_some() && req.approve_plan)` | Keep as documented; no code change |

## 5. Acceptance scenarios and test design

| Test ID | Requirement | Command |
| --- | --- | --- |
| TC-1660 | FR-1660 | `cargo test tc_1660_headless_agent_approve_confirms_sensitive_gate` |
| TC-1661 | FR-1661 | `cargo test tc_1661_allow_sensitive_bypasses_headless_gate` |
| TC-1662 | FR-1662 | `cargo test tc_1662_sensitive_keyword_match` |
| TC-1663 | FR-1663 | `cargo test tc_1663_custom_child_clears_inherited_mcp_env` |
| TC-1664 | FR-1664 | `cargo test tc_1664_help_documents_allow_sensitive` |
| TC-1665 | FR-1665 | manual: README headless section mentions `--allow-sensitive` and agent+approve sensitive bypass |
| TC-1825 | FR-1825 | `cargo test tc_1660_headless_agent_approve_confirms_sensitive_gate` |
| TC-1826 | FR-1826 | `cargo test tc_1661_allow_sensitive_bypasses_headless_gate` |
| TC-1827 | FR-1827 | planned — no automated test yet (Stream B) |
