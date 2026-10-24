# TASKS: MCP propagation removal

Spec: [SPEC-mcp-propagation-removal.md](../specs/SPEC-mcp-propagation-removal.md)

## Part A (Code implementation)

Owner: agy-flash (gemini-3.8-flash-high)

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | agy-flash (gemini-3.8-flash-high) | ACP session builders (`session/new`, `session/load`, `session/resume`) always send empty `mcpServers: []` | FR-2080 | `cargo test tc_2080_acp_session_params_send_empty_mcp_servers`, `cargo test tc_2080_cursor_acp_session_new_has_empty_mcp_servers` |
| [x] | agy-flash (gemini-3.8-flash-high) | `[[mcp.servers]] runners` key accepted and ignored with exactly one deprecation warning | FR-2081 | `cargo test tc_2081_legacy_mcp_runners_key_loads_with_one_warning` |
| [x] | agy-flash (gemini-3.8-flash-high) | Custom agent `{mcp_config}` placeholder dropped with one deprecation warning per agent; `DMUXAI_MCP_CONFIG` scrubbed from child env | FR-2082 | `cargo test tc_2082_custom_mcp_config_placeholder_dropped_with_warning` |
| [x] | agy-flash (gemini-3.8-flash-high) | Built-in `dmuxai-memory` auto-entry injection removed from `[memory]` settings | FR-2083 | `cargo test tc_2083_memory_on_adds_no_builtin_mcp_entry` |
| [x] | agy-flash (gemini-3.8-flash-high) | Remove MCP CLI arguments, environment variables, and temporary config files across runner adapters (Codex, OpenCode, Custom, Cursor, AGY) | FR-2084 | `cargo test tc_2084_no_runner_gets_mcp_args_or_files` |
| [x] | agy-flash (gemini-3.8-flash-high) | Delete dead propagation modules (`src/mcp/propagate.rs`, `src/mcp/inject.rs`, `RunnerMcp`, `applies_to`) and add source scan regression test | FR-2085 | `cargo test tc_2085_no_propagation_code_left` |

## Part B (Documentation, spec, test-plan updates)

Owner: agy-flash (gemini-3.8-flash-high)

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | agy-flash (gemini-3.8-flash-high) | Author SPEC-mcp-propagation-removal.md with FR-2080..FR-2085 and TC-2080..TC-2085 | FR-2080..FR-2085 | `python3 scripts/check-spec.py docs/specs/SPEC-mcp-propagation-removal.md` |
| [x] | agy-flash (gemini-3.8-flash-high) | Mark superseded specs and requirements (SPEC-mcp-injection, SPEC-web-dashboard, SPEC-obsidian-memory, SPEC-acp-runners, SPEC-custom-agent-provider, SPEC-codex-sandbox-policy) | all | `python3 scripts/check-spec.py --all` |
| [x] | agy-flash (gemini-3.8-flash-high) | Update TEST-PLAN.md: remove deleted TC rows, add TC-2080..TC-2085 rows | all | `python3 scripts/check-tc-links.py` |
| [x] | agy-flash (gemini-3.8-flash-high) | Update tc-link and spec-check allowlists | all | `python3 scripts/check-tc-links.py`, `python3 scripts/check-spec.py --all` |
| [x] | agy-flash (gemini-3.8-flash-high) | Update README.md, docs/API.md, and CHANGELOG.md | all | manual review, `cargo test --test spec_index` |
