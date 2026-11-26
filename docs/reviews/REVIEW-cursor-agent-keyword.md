# REVIEW: cursor-agent keyword

Spec: [SPEC-cursor-agent-keyword.md](../specs/SPEC-cursor-agent-keyword.md)

## Compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-3200 | Met | `tc_3200_canonical_runner_id_maps_cursor_agent` |
| FR-3201 | Met | `tc_3201_cli_accepts_cursor_agent_keyword` |
| FR-3202 | Met | `tc_3202_config_accepts_cursor_agent_keyword`, `tc_3203_cursor_and_cursor_agent_tables_conflict_warns` |
| FR-3203 | Met | `tc_3204_plan_runner_accepts_cursor_agent` |
| FR-3204 | Met | `tc_3205_mcp_accepts_cursor_agent_keyword`, `normalize_request_agent_id` |
| FR-3205 | Met | `tc_3206_custom_agent_cannot_take_cursor_agent_name` |
| FR-3206 | Met | `tc_3207_runners_json_lists_cursor_agent_alias` |
| UX-3200 | Met | `tc_3208_docs_name_cursor_agent_keyword` |

## Parse boundaries changed

| File | Function / site |
| --- | --- |
| `src/runner_id.rs` | `canonical_runner_id` (shared helper) |
| `src/cli.rs` | `RUN_USAGE`, `MODELS_USAGE`, `RUNNERS_USAGE`; `resolve_cli_builtin_runner`, `parse_cli_agents_csv` |
| `src/settings.rs` | `parse_runners_tables`, `allowed_runners`, `routing.default_agent`, `graph_runner_allowed` |
| `src/runner_fallback.rs` | `FallbackTarget::parse`, `FallbackSettings::from_tables` (table keys) |
| `src/model_policy.rs` | `validate_model_for_runner_with`, `default_model_for_runner` |
| `src/app.rs` | `runner_from_str`, `parse_agent`, `run_headless`, `resolve_dashboard_agent` |
| `src/graph/plan_doc.rs` | `check_runner`, `validate_plan_with_limit` node build, `fallback_plan`, `pin_graph_plan_runners` |
| `src/graph/policy.rs` | `assign_runner` |
| `src/agent_api.rs` | `runner_run_admitted`, `collect_models_json`, `collect_model_listing_with`, `runners_json` |
| `src/custom_agent.rs` | `install_registry` reserved names |
| `src/conversation.rs` | `AgentKey::from_db` |
| `src/theme.rs` | `[theme.agents]` key merge |
| `src/web/server.rs` | `run_todo`, `post_agent_prompts`, `normalize_request_agent_id` |
| `src/mcp_server.rs` | `models` tool (via `collect_models_json`) |

## `"cursor"` literals deliberately retained (internal / canonical)

- `src/agent_api.rs` `runner_ids`, status JSON, model cache keys, wire `id` fields
- `src/app.rs` `runner_name`, journals, UI labels, stored graph nodes after normalization
- `src/conversation.rs` `AgentKey::as_db`
- Config table path comments and docs examples using `[runners.cursor]` (legacy name)
- Tests fixing canonical stored runner `"cursor"`
- Process spawn paths (`cursor-agent` binary name, unrelated to runner id)

## Residual risks

- A future parse boundary that lowercases without `canonical_runner_id` would reject `cursor-agent` (safe failure, not a policy bypass).
- Welcome text and some internal help strings still display the canonical id `cursor` in runner lists; only user-facing CLI/docs were retargeted to `cursor-agent`.

## Scan result

Manual review of `rg '"cursor"' src` on parse paths: CLI, settings, graph plan, TUI (`runner_from_str` / `parse_agent`), MCP/web agent fields, fallback targets, routing and `allowed_runners` route through `canonical_runner_id` or `runner_from_str`. No unmatched input boundary identified in this diff.

## Test evidence

To be filled by the gate.