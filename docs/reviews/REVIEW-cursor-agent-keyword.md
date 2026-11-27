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
| FR-3207 | Met | `tc_3211_json_outputs_use_cursor_agent_id`, `tc_3212_web_snapshot_uses_cursor_agent_id` |
| FR-3208 | Met | `tc_3209_legacy_cursor_row_reads_as_cursor_agent`, `tc_3210_legacy_cursor_journal_runner_normalizes`, `tc_3213_legacy_cursor_config_and_graph_store_normalize`, `tc_3214_legacy_event_agent_normalizes` |
| FR-3209 | Met | `tc_3213_legacy_cursor_config_and_graph_store_normalize` |
| UX-3200 | Met | `tc_3208_docs_name_cursor_agent_keyword` |

## Parse boundaries changed

| File | Function / site |
| --- | --- |
| `src/runner_id.rs` | `CURSOR_RUNNER_ID`, `canonical_runner_id` |
| `src/cli.rs` | `RUN_USAGE`, `MODELS_USAGE`, `RUNNERS_USAGE`; CLI agent resolution, `start_headless_journal` |
| `src/settings.rs` | `parse_runners_tables`, `allowed_runners`, `routing.default_agent`, `graph_runner_allowed` |
| `src/runner_fallback.rs` | `config_for`, `chain_for`, `FallbackTarget::parse` |
| `src/model_policy.rs` | `validate_model_for_runner_with`, `default_model_for_runner` |
| `src/app.rs` | `runner_from_str`, `parse_agent`, `agent_runner_name`, `runner_is_allowed`, `first_allowed_runner_id`, `restore_runner_selection` |
| `src/graph/plan_doc.rs` | `check_runner`, validate/build, `fallback_plan`, `pin_graph_plan_runners` |
| `src/graph/policy.rs` | `assign_runner` |
| `src/agent_api.rs` | `runner_ids`, `runners_json`, `models_json`, `status_json` event aggregation |
| `src/agent/acp_runner.rs` | `cursor_acp_argv` model policy check |
| `src/agent/cursor.rs` | `validate_model_for_runner` calls |
| `src/custom_agent.rs` | `install_registry` reserved names |
| `src/conversation.rs` | `AgentKey::from_db`, `as_db`, `db_aliases`, SQL queries matching both spellings |
| `src/theme.rs` | `agent_color`, `unknown_agent_ids` legacy `[theme.agents]` keys |
| `src/web/server.rs` | `normalize_request_agent_id`, agent prompt API |
| `src/web/runs_api.rs` | `progress_as_json_values`, `tail_text_lines` progress normalization |
| `src/web/assets/session.js` | `SHARE_TARGETS`, `agentDisplayLabel` |
| `src/mcp_server.rs` | `models` tool (via `collect_models_json`), schema description |
| `src/run_journal.rs` | `normalize_manifest_runners` on `load`, `normalize_progress_line` on `load_progress_tail` |
| `src/events.rs` | `normalize_event_runner` on read, `EventLog::append` canonicalization |
| `src/graph/store.rs` | `GraphRun::normalize_legacy_runners` on read |
| `src/init.rs` | Starter config `[theme.agents]` example |

## `"cursor"` literals deliberately retained (internal / legacy)

- `src/runner_id.rs`: legacy alias mapping (`cursor` → `cursor-agent`)
- `src/runner_id.rs` / `src/agent_api.rs`: `aliases: ["cursor"]` on the Cursor runner JSON entry
- `Agent::Cursor` enum variant, `[cursor]` config table name, SQLite column name `agent`, `DMUXAI_CURSOR_BIN`
- Legacy config table paths in user docs/examples: `[runners.cursor]` still valid on read
- Tests that pass legacy `cursor` as **input** to prove alias coverage
- Binary / process names: `cursor-agent` CLI, `.cursor/skills` staging path

## Residual risks

- Downgrade to a pre-flip binary: conversation rows and manifests written as `cursor-agent` are not recognized as Cursor.
- A future boundary that lowercases without `canonical_runner_id` would accept wrong ids (safe failure, not a policy bypass).

## Scan result

Manual review of production `rg '"cursor"' src`: remaining literals are legacy alias handling, aliases JSON, the `Agent::Cursor` variant, `[cursor]` table name, env `DMUXAI_CURSOR_BIN`, staging paths, and tests/fixtures that intentionally use legacy input. Emit paths use `CURSOR_RUNNER_ID` or `canonical_runner_id`.

## Test evidence

To be filled by the gate.