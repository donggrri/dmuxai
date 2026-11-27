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
| FR-1825 | Must | Headless `dmuxai run` reaching a sensitive (`WaitingGate`) gate | The process exits 7 unless `--allow-sensitive` or `--agent` with `--approve-plan` auto-confirms it; the error is `sensitive step needs approval: rerun with --allow-sensitive (or --agent <name> --approve-plan)` and `error_kind` is `sensitive_gate` | Does not replace plan approval; a read-only turn (`--read-only`, pending/legacy read-only policy) is not gated | TC-1825, TC-1678 |
| FR-1826 | Must | `dmuxai run` with `--allow-sensitive` in headless mode | Sensitive gates are auto-confirmed on direct, judge, and graph paths | Plan approval still required for complex plans | TC-1826 |
| FR-1827 | Must | The same sensitive gate in dashboard (`--web`/TUI) mode | The gate pauses and `/graph continue` resumes it; there is no CLI-flag bypass | view-only `serve` cannot run agents | TC-1827 |
| TC-3210 | FR-3210 | `cargo test tc_3210_read_only_without_agent_skips_sensitive_gate` (no `--agent`, prompt `deploy the db migration to production`, `mode=ask` → exit 0, no `sensitive_gate` error kind, no write policy) |
| TC-3211 | FR-3211 | `cargo test tc_3211_board_ask_without_planner_sensitive_prompt_not_exit_7` (`[board] planner` unset; the exact prompt `build_ask_prompt` produces for an item whose title/thread contains `deploy`/`db` is run headless with `mode=ask` and no agent, using the test app's stub adapters → exit 0 answer, not exit 7; plus the ask argv still carries `--read-only` and no `--allow-sensitive`) |
| FR-3210 | Must | Headless `dmuxai run --read-only` **without** `--agent` (auto routing / triage, judge or graph planning path), e.g. a board planner ask with no `[board] planner` (#353) | The run never exits 7 because of the sensitive keyword gate on any path: a read-only turn skips the sensitive gate (`WaitingGate`) wherever it is decided, and a read-only turn that would otherwise stop for plan approval only because of a sensitive step runs as a read-only answer instead. Exit is 0 with the answer text, or the normal failure code | `--read-only` stays incompatible with `--write`; non read-only runs keep FR-1825 exactly (exit 7 without `--allow-sensitive` / `--agent ... --approve-plan`); read-only never grants edits (the turn policy stays read-only on every step, no write tools) | TC-3210 |
| FR-3211 | Must | The dashboard / `board ask` child (`run --read-only --json -`) with no `[board] planner` and a prompt or thread that contains sensitive words (`deploy`, `db`, `login`, ...) | The ask returns a `planner` answer instead of `planner ask failed (exit 7)` | With `[board] planner` set the behaviour is unchanged | TC-3211 |

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
| TC-1678 | FR-1825 | `cargo test tc_1678_headless_gate_hint_and_error_kind`, `cargo test tc_1678_read_only_turn_skips_sensitive_gate`, `cargo test run_sensitive_gate_exits_7_headless` |
| TC-1826 | FR-1826 | `cargo test tc_1661_allow_sensitive_bypasses_headless_gate` |
| TC-1827 | FR-1827 | planned — no automated test yet (Stream B) |
