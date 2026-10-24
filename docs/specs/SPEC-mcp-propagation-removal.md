# SPEC-mcp-propagation-removal

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: main
Related: Dongjin decision 2026-10-24, supersedes SPEC-mcp-injection (deleted 2026-10-24), SPEC-web-dashboard INT-561/INT-563 propagation, SPEC-obsidian-memory FR-733

## 1. Decision summary

Following Dongjin's decision on 2026-10-24, Model Context Protocol (MCP) server propagation from DmuxAI to runner adapters is completely removed. Every agent CLI now configures MCP servers in its own native configuration; DmuxAI no longer passes its `[[mcp.servers]]` list to any runner process.

`[[mcp.servers]]` remains in `dmuxai.toml` exclusively for the web dashboard:
- Providing the server list for `GET /api/mcp/servers` (redacted registry view).
- Powering Jira/Confluence issue and document import for to-dos (`POST /api/import`).
- In addition, DmuxAI's own stdio MCP servers (`dmuxai mcp` and `dmuxai mcp-memory`) remain fully supported for external IDE and agent integrations.

### Old config behavior

#### Previous behavior

Previously, DmuxAI attempted to centralize MCP definitions under `[[mcp.servers]]` and propagate them across diverse runner adapters:
- OpenCode: injected MCP entries into a per-run `OPENCODE_CONFIG` temporary JSON file.
- Codex: injected `-c mcp_servers.<name>.*` command-line overrides and child environment variables (`DMUXAI_MCP_<SERVER>_<HEADER>`).
- Custom agents: generated per-turn `{"mcpServers":{...}}` JSON files exposed through `{mcp_config}` placeholders and `DMUXAI_MCP_CONFIG` environment variables.
- Cursor Legacy: attempted to write workspace `.cursor/mcp.json` or emitted a warning notice.
- ACP runners (Cursor ACP, custom ACP, Grok ACP): populated session initialization parameters (`session/new`, `session/load`, `session/resume`) with `mcpServers`.
- Obsidian memory: automatically injected a built-in `dmuxai-memory` MCP server into `[[mcp.servers]]` when `[memory]` was configured.

This ad-hoc propagation caused complex runner coupling, fragile escaping, and configuration conflicts. Under the new architecture, runners configure their own tools natively, and DmuxAI remains focused on orchestration.

#### Existing config behavior after this change

When loading an existing `dmuxai.toml` file created prior to this change:
- **Zero parse or deserialization errors:** Existing configurations always load successfully; legacy or deprecated fields never cause a parse error or failure to launch.
- **`[[mcp.servers]] runners`:** The `runners` allow-list key is accepted and ignored, emitting exactly one deprecation warning across all loaded configuration files per settings load (`runners is deprecated and ignored`).
- **`{mcp_config}` in custom agents:** If a custom agent command includes `{mcp_config}`, the placeholder is unset, dropping both the argument and its preceding flag (e.g. `--mcp`), emitting exactly one deprecation warning per custom agent. The child process environment remains scrubbed of `DMUXAI_MCP_CONFIG`.
- **`dmuxai-memory` entry:** An entry named `dmuxai-memory` in `[[mcp.servers]]` is treated as an ordinary user-defined entry. DmuxAI no longer auto-adds or injects any built-in `dmuxai-memory` entry into `mcp_servers` when `[memory]` is enabled.
- **Dashboard registry and imports:** Generic `[[mcp.servers]]` entries are only listed in the web dashboard registry (`GET /api/mcp/servers`). Entries configured with `kind = "jira"` or `kind = "confluence"` still power issue and document import for to-dos (`POST /api/import`).
- **Agent isolation:** Agents keep using only their own native MCP configuration (e.g. `~/.cursor/mcp.json`, OpenCode native settings, Claude Desktop configs). DmuxAI never passes any MCP configuration, CLI flags, environment variables, or temporary files to runner processes.

## 2. Scope

### In scope

- ACP session initialization parameters (`session/new`, `session/load`, `session/resume`) in `src/agent/acp/session.rs` and `src/agent/cursor.rs` always pass `mcpServers: []`.
- Settings parser in `src/settings.rs` tolerates the legacy `runners` key in `[[mcp.servers]]` with a single deprecation warning per settings load, never failing parsing.
- Custom agent spec parser in `src/custom_agent.rs` marks `{mcp_config}` deprecated with one warning per agent, unsetting the placeholder and dropping its preceding flag; `DMUXAI_MCP_CONFIG` remains scrubbed from child process environments.
- Memory settings in `src/settings.rs` no longer auto-inject a built-in `dmuxai-memory` entry into `mcp_servers`.
- Complete elimination of MCP command-line arguments, environment variables, and temporary config files across Codex, OpenCode, Custom, Cursor, and AGY adapters.
- Complete deletion of obsolete propagation and injection modules (`src/mcp/propagate.rs`, `src/mcp/inject.rs`, `RunnerMcp`, and runner-filtering `applies_to`).
- Regression test suite asserting empty ACP arrays, legacy config deprecation warnings, absence of built-in memory entries, absence of runner MCP arguments/files, and source scan verifying absence of dead propagation code.

### Out of scope / non-goals

- Modifying the web dashboard MCP registry view (`GET /api/mcp/servers`) or Jira/Confluence import endpoints (`POST /api/import`).
- Removing the `dmuxai mcp` stdio MCP server exposing DmuxAI tools.
- Removing the `dmuxai mcp-memory` subcommand providing Obsidian vault memory tools over stdio.
- Changing child environment scrubbing of `DMUXAI_MCP_CONFIG` (protected by FR-1663).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2080 | Agents desiring `dmuxai mcp-memory` or external tools configure them directly in `~/.cursor/mcp.json`, OpenCode config, or their own MCP settings. | dmuxai | PR-9 delivery |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Agent-native MCP config | Configuration maintained directly by the user in the agent's own configuration format (e.g. `~/.cursor/mcp.json`). | DmuxAI `[[mcp.servers]]` in `dmuxai.toml`. |
| Dashboard MCP servers | `[[mcp.servers]]` entries in `dmuxai.toml` used solely for dashboard registry display and Jira/Confluence import. | Runner-propagated MCP tools. |
| Memory MCP server | `dmuxai mcp-memory` stdio tool server run directly by external agents. | Automatic in-memory injection by DmuxAI. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2080 | Must | ACP session initialization or resumption (`session/new`, `session/load`, `session/resume`) for Cursor ACP or custom/grok ACP | Payload parameters always include `"mcpServers": []` regardless of any `[[mcp.servers]]` configured in DmuxAI | Workspace or user MCP server settings never populate ACP session params | TC-2080 |
| FR-2081 | Must | Settings loaded from TOML containing `[[mcp.servers]] runners = [...]` | Settings load successfully with `runners` ignored, emitting exactly one deprecation warning across all loaded files | Never a TOML deserialize or parse error; warnings collected in `config_warnings` | TC-2081 |
| FR-2082 | Must | Custom agent command containing `{mcp_config}` placeholder | The placeholder is unset, dropping the argument and its preceding flag (`-flag`), and exactly one deprecation warning is emitted per custom agent; `DMUXAI_MCP_CONFIG` remains scrubbed from child process environment (FR-1663) | Multiple references across agents produce one warning per agent; child environment never inherits `DMUXAI_MCP_CONFIG` | TC-2082 |
| FR-2083 | Must | Settings loaded with valid `[memory] vault = "..."` configuration | No built-in `dmuxai-memory` MCP entry is added to `mcp_servers`; users configure `dmuxai mcp-memory --workspace <dir>` in their agent's own MCP config | `mcp_servers` contains only explicit user and workspace definitions; no auto-generated entry | TC-2083 |
| FR-2084 | Must | A turn is submitted to any runner adapter (Codex, OpenCode, Custom, Cursor, AGY) with MCP servers configured in `dmuxai.toml` | No runner receives MCP arguments, environment variables, or generated configuration files: Codex has no `-c mcp_servers.*` argv, OpenCode temp config has no `mcp` key, custom agents receive no `dmuxai-mcp-*.json` files or env vars, and Cursor/AGY emit no notices | Turns execute normally without MCP propagation artifacts; temporary directories remain free of MCP files | TC-2084 |
| FR-2085 | Must | Source tree scan of `src/**/*.rs` | No references remain to `RunnerMcp`, `mcp::propagate`, `mcp::inject`, `.applies_to(`, or `.cursor/mcp.json` | Any accidental re-introduction fails automated regression tests | TC-2085 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2080 | FR-2080 | unit + integration | Configured MCP servers in `dmuxai.toml` and ACP session parameter builders / live Cursor ACP session | Construct `session/new`, `session/load`, `session/resume` params; execute Cursor ACP turn | All session payloads contain `"mcpServers": []`; captured ACP turn parameters confirm empty array | `src/agent/acp/session.rs` `tc_2080_acp_session_params_send_empty_mcp_servers`; `src/agent/cursor.rs` `tc_2080_cursor_acp_session_new_has_empty_mcp_servers` |
| TC-2081 | FR-2081 | unit | Config files with `runners = [...]` on `[[mcp.servers]]` across user and workspace configs | Call `DmuxSettings::load_with_user_config` | Servers are parsed and present in `mcp_servers`; exactly one warning is emitted stating `runners is deprecated and ignored` | `src/settings.rs` `tc_2081_legacy_mcp_runners_key_loads_with_one_warning` |
| TC-2082 | FR-2082 | unit | Custom agent configured with `command = ["x", "--mcp", "{mcp_config}", "{prompt}"]` | Load settings and build argv via `build_argv` | Warning emitted naming agent and deprecation; generated argv drops `--mcp` and `{mcp_config}`, producing `["x", "prompt"]` | `src/custom_agent.rs` `tc_2082_custom_mcp_config_placeholder_dropped_with_warning` |
| TC-2083 | FR-2083 | unit | Workspace with `[memory] vault = "..."` configured | Call `DmuxSettings::load_with_exe` | `mcp_servers` does not contain any entry named `dmuxai-memory`; no spurious warnings emitted | `src/settings.rs` `tc_2083_memory_on_adds_no_builtin_mcp_entry` |
| TC-2084 | FR-2084 | integration | Active MCP servers configured; execute Codex args builder and custom agent turn with `{temp_config}` | Inspect Codex argv, run custom turn inspecting stdout and filesystem | Codex argv has no `mcp_servers`; custom agent output shows `env=` with empty `DMUXAI_MCP_CONFIG` and JSON config without `mcp` key; no `dmuxai-mcp-*.json` files created | `src/agent/custom.rs` `tc_2084_no_runner_gets_mcp_args_or_files` |
| TC-2085 | FR-2085 | unit | Source tree `src/**/*.rs` | Traverse directory and scan file contents for obsolete propagation identifiers | No matches found for `RunnerMcp`, `mcp::propagate`, `mcp::inject`, `applies_to(`, or `.cursor/mcp.json` | `src/mcp/config.rs` `tc_2085_no_propagation_code_left` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| ACP Protocol | `mcpServers` parameter is always empty array `[]` | Fully compatible with ACP specification | TC-2080 |
| Config Schema | `runners` key in `[[mcp.servers]]` and `{mcp_config}` placeholder in `[[agents.custom]]` ignored with deprecation warnings | Backward compatible with existing user `dmuxai.toml` files; never causes parse failure | TC-2081, TC-2082 |
| Environment Isolation | `DMUXAI_MCP_CONFIG` remains unconditionally stripped from all child processes (FR-1663) | Preserves security against parent environment leakage | TC-2084 |
| Memory MCP | `dmuxai mcp-memory` executable remains available for direct invocation | Users register memory CLI in their own agent configurations | TC-2083 |

### Removed requirements

The following requirements from earlier specifications (including FR-1980..FR-1984 / TC-1980..TC-1984 from SPEC-mcp-injection, deleted 2026-10-24) are superseded and removed by this decision:

| Superseded requirement | Former scope | Replacement | Reason |
| --- | --- | --- | --- |
| FR-1980 / TC-1980 (was SPEC-mcp-injection, deleted 2026-10-24) | `RunnerMcp::for_runner` applicability filtering | FR-2081, FR-2085 | Runners configure MCP natively; filtering logic removed |
| FR-1981 / TC-1981 (was SPEC-mcp-injection, deleted 2026-10-24) | `RunnerMcp` runner-specific payload builders | FR-2084, FR-2085 | Adapters no longer build MCP arguments or payloads |
| FR-1982 / TC-1982 (was SPEC-mcp-injection, deleted 2026-10-24) | Cursor legacy `.cursor/mcp.json` guard and notice | FR-2084, FR-2085 | Cursor legacy turns never receive or write MCP configs |
| FR-1983 / TC-1983 (was SPEC-mcp-injection, deleted 2026-10-24) | Cursor ACP session `mcpServers` parameter injection | FR-2080 | ACP sessions always send empty `mcpServers: []` |
| FR-1984 / TC-1984 (was SPEC-mcp-injection, deleted 2026-10-24) | Enforce no `.applies_to(` calls outside `src/mcp/` | FR-2085 | All propagation and applicability checking removed |
| [INT-561](SPEC-web-dashboard.md) | Native MCP propagation to runners (OpenCode, Codex, custom, Cursor, AGY) | FR-2084 | DmuxAI no longer propagates MCP servers to runners |
| [INT-563](SPEC-web-dashboard.md) | Adapter spawn-time MCP propagation delivery | FR-2084 | Runner adapters receive no MCP configuration |
| [NFR-561](SPEC-web-dashboard.md) (propagation portion) | Codex child env and secret keys in runner argv | FR-2084 | Codex app-server receives no MCP overrides |
| [FR-733](SPEC-obsidian-memory.md) | Auto-injection of built-in `dmuxai-memory` MCP server into `mcp_servers` | FR-2083 | Users configure `dmuxai mcp-memory` in agent MCP configs |
| [INT-732](SPEC-obsidian-memory.md) | Documentation of automatic `dmuxai-memory` MCP registration | FR-2083 | Replaced by manual agent configuration guide |
| [FR-1414](SPEC-acp-runners.md) | ACP session `mcpServers` carrying configured MCP servers | FR-2080 | Always sends `mcpServers: []` |
| [FR-1851](SPEC-custom-agent-provider.md) | Custom agent `{mcp_config}` and OpenCode `mcp` JSON injection | FR-2082, FR-2084 | OpenCode and custom agents receive no MCP temp configs |
| [FR-1867](SPEC-codex-sandbox-policy.md) | Codex app-server stdio `-c mcp_servers.*` CLI overrides | FR-2084 | Codex app-server receives no MCP CLI arguments |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | ACP session parameter builders send empty `mcpServers: []` | `src/agent/acp/session.rs`, `src/agent/cursor.rs` | FR-2080 | `cargo test tc_2080` |
| 2 | Deprecate `[[mcp.servers]] runners` with single warning | `src/settings.rs` | FR-2081 | `cargo test tc_2081` |
| 3 | Deprecate `{mcp_config}` placeholder in custom agent spec | `src/custom_agent.rs` | FR-2082 | `cargo test tc_2082` |
| 4 | Remove automatic built-in `dmuxai-memory` injection | `src/settings.rs` | FR-2083 | `cargo test tc_2083` |
| 5 | Remove runner MCP injection arguments, environment, and temp files | `src/agent/codex.rs`, `src/agent/custom.rs`, `src/agent/cursor.rs`, `src/app.rs` | FR-2084 | `cargo test tc_2084` |
| 6 | Delete dead propagation modules (`src/mcp/propagate.rs`, `src/mcp/inject.rs`) | `src/mcp/mod.rs` | FR-2085 | `cargo test tc_2085` |
| 7 | Update specifications, test plans, tasks, and documentation | `docs/specs/*`, `docs/TEST-PLAN.md`, `docs/tasks/*`, `README.md`, `docs/API.md`, `CHANGELOG.md` | FR-2080..FR-2085 | `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-2080..FR-2085) map to passing test evidence (TC-2080..TC-2085).
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [x] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
