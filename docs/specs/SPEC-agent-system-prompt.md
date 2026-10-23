# SPEC: agent-system-prompt

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `856d3fc`
Related: #140

## 1. Decision summary

DmuxAI supports a per-runner system prompt configured under `[runners.default]` / `[runners.<id>]` in `dmuxai.toml` (user config first, workspace override per key). A prompt is either inline (`system_prompt`) or read from a file (`system_prompt_file`), is capped at 32 KiB, and is delivered according to the runner: opencode receives it through an `instructions` array in a temp config, Codex through `developerInstructions` on `thread/start` and `thread/resume`, and Cursor/AGY/Grok/generic custom agents through a prepended `<agent-instructions>` block placed before memory. Custom agents can opt into native delivery via `{system_prompt}` / `{system_prompt_file}` placeholders and `system_prompt_arg`. Injection cadence is `session` (default: fresh session or hash change) or `always`; only `chat` and `worker` roles ever receive it. The transcript stores only user text, headless JSON reports `{source, hash, delivery}` (never the text), and the dashboard exposes GET/POST `/api/agent-prompts` with a preview-only response.

## 2. Scope

### In scope

- `RunnerPromptConfig` / `RunnerPromptToml` parsing in `src/agent_prompt.rs` and `src/settings.rs` (`[runners.default]` / `[runners.<id>]`).
- `resolve`, `should_inject`, `hash_prompt`, `prepend_block`, `delivery` in `src/agent_prompt.rs`.
- Turn injection in `src/app.rs` `try_submit_to_agent`, including the `system prompt applied` notice and `pending_prompt_application`.
- Codex `developerInstructions` in `src/agent/codex.rs` `thread_open_request`.
- opencode `instructions` array and custom-agent placeholders in `src/agent/custom.rs`.
- Headless outcome `system_prompt` field in `src/app.rs` / `src/cli.rs`.
- Dashboard API and panel in `src/web/server.rs` and `src/web/assets/agent-prompts.js`.

### Out of scope / non-goals

- Model, temperature, and permission profiles (deferred to PR-3).
- Changing provider protocols beyond the fields listed above.
- Storing prompt text in transcripts or headless JSON.
- Redefining TCs belonging to other specs.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1700 | `system_prompt_file` is read as UTF-8; a non-UTF-8 or unreadable file is ignored with a config warning. | dmuxai | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| system_prompt | Inline prompt text under `[runners.<id>]`. | `agent.prompt` (replaces base prompt). |
| system_prompt_file | Path to a prompt file, relative to the config file that sets it unless absolute. | `temp_config_template`. |
| prompt_inject | `session` (default) or `always`. | Memory injection cadence. |
| prompt_roles | Roles allowed to receive the prompt; defaults to `chat` and `worker`. | Agent modes. |
| ResolvedPrompt | Resolved `{text, hash, source, inject}` ready for delivery. | Raw config. |
| Delivery | `Native` or `Prepend`. | Provider protocol. |
| prepend_block | `<agent-instructions>\n{text}\n</agent-instructions>\n\n`. | Memory block. |
| source | Provenance label such as `user:[runners.codex]` or `workspace:[runners.default] (file)`. | Hash. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1700 | Must | Given a `[runners.default]` or `[runners.<id>]` table in user or workspace `dmuxai.toml` | Parse `system_prompt` (inline) and `system_prompt_file` (path resolved relative to the config file dir, absolute allowed); the two are mutually exclusive; text longer than 32768 bytes is ignored with a warning; an unreadable file is ignored with a warning; both set is ignored with a warning; warnings go to `config_warnings` | Empty/whitespace prompt resolves to none later; cap is measured in bytes | TC-1707, TC-1708 |
| FR-1701 | Must | Given `prompt_inject` configured or defaulted | Parse `session` (default) or `always`; unknown value warns and defaults to `session`; `should_inject` returns true for `always`, and for `session` only on a fresh session or when the last injected hash differs; hash is a stable 16-hex SHA-256 prefix | Missing key defaults to `session` | TC-1710, TC-1713 |
| FR-1702 | Must | Given `prompt_roles` configured or defaulted | Default roles are `chat` and `worker`; only `chat` and `worker` are configurable; `judge`, `triage`, `planner`, `replan`, `summary`, `interview` are dropped with a warning even if listed; unknown roles warn; `resolve` returns `None` for every internal role unconditionally; graph worker turns use the `worker` role | An excluded role never receives a prompt even if explicitly listed | TC-1711, TC-1712 |
| FR-1703 | Must | Given user and workspace `dmuxai.toml` both define runner tables | Workspace overrides user per key per table; a runner-specific table overrides `default`; runner ids are matched case-insensitively; `source` records `user`/`workspace` scope and file provenance | Absent key in workspace preserves the user value | TC-1709 |
| FR-1704 | Must | Given a resolved prompt and a runner | Delivery is `Native` for opencode and codex, `Prepend` for cursor, agy, grok, and generic custom agents; opencode delivers via `instructions` array in a temp config (not `agent.prompt`, no `--agent`); Codex sets `developerInstructions` on `thread/start` and `thread/resume` (not `baseInstructions`); custom agents render `{system_prompt}` / `{system_prompt_file}` placeholders and `system_prompt_arg` opts into native delivery | A custom agent without placeholders or `system_prompt_arg` uses prepend | TC-1714, TC-1716, TC-1717 |
| FR-1705 | Must | Given a turn submission in `try_submit_to_agent` | Resolve the prompt once per turn using the turn's role; prepend the block before memory; push a `system prompt applied (source, hash)` notice; store only user text in the transcript; internal roles receive nothing; graph workers receive the prompt; native runners set `system_prompt` instead of prepending | No prompt configured -> no change and no notice | TC-1715 |
| FR-1706 | Must | Given a headless `dmuxai run --json` or the dashboard | Headless outcome reports `system_prompt {source, hash, delivery}` and never the text; GET/POST `/api/agent-prompts` list rows with set/source/hash/delivery/inject/roles/preview (preview truncated to 200 chars); POST validates size and inject; the dashboard panel renders and saves | Full prompt text never appears in any response | TC-1718, TC-1719, TC-1720 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1707 | FR-1700 | unit | runner tables in dmuxai.toml | parse settings | inline/file/relative/absolute prompts resolved; no warnings | `src/settings.rs` `agent_prompt_inline_text`, `agent_prompt_file_text`, `agent_prompt_relative_path_resolved_against_config_file_dir`, `agent_prompt_absolute_path_allowed` |
| TC-1708 | FR-1700 | unit | conflicting, oversize, or unreadable prompt config | parse settings | warnings emitted and prompt ignored | `src/settings.rs` `agent_prompt_both_set_warns_and_ignored`, `agent_prompt_greater_than_32kib_warns_and_ignored`, `agent_prompt_unreadable_file_warns_and_ignored` |
| TC-1709 | FR-1703 | unit | user and workspace configs | load settings | workspace overrides per key; runner overrides default; case-insensitive ids; source labels | `src/settings.rs` `agent_prompt_workspace_overrides_user`, `agent_prompt_runner_id_overrides_default`, `agent_prompt_workspace_runner_overrides_user_default` |
| TC-1710 | FR-1701 | unit | `prompt_inject` values | parse settings | session default, always parsed, unknown warns and defaults to session | `src/settings.rs` `agent_prompt_default_inject_session`, `agent_prompt_always_parsed`, `agent_prompt_unknown_inject_warns` |
| TC-1711 | FR-1702 | unit | `prompt_roles` values | parse settings | default [chat, worker]; internal roles dropped even if listed; unknown warns; case-insensitive runner | `src/settings.rs` `agent_prompt_default_roles`, `agent_prompt_judge_triage_planner_replan_summary_interview_excluded_even_if_listed`, `agent_prompt_runner_case_insensitive` |
| TC-1712 | FR-1702 | unit | settings plus resolver | resolve for each role | unconfigured, empty/whitespace, and excluded roles return None | `src/agent_prompt.rs` `agent_prompt_resolve_none_for_unconfigured_runner`, `agent_prompt_resolve_empty_or_whitespace_returns_none`, `agent_prompt_excluded_roles_always_return_none` |
| TC-1713 | FR-1701 | unit | inject truth table and hash | evaluate | always true; session per freshness/hash; stable 16-hex hash | `src/agent_prompt.rs` `agent_prompt_should_inject_truth_table`, `agent_prompt_hash_stable_and_changes_with_text` |
| TC-1714 | FR-1704 | unit | runner and custom spec | `delivery()` | opencode/codex native; cursor/agy/grok/custom prepend; opted-in custom native; prepend block format | `src/agent_prompt.rs` `agent_prompt_delivery_per_runner`, `agent_prompt_prepend_block_format`, `agent_prompt_delivery_native_for_opted_in_custom` |
| TC-1715 | FR-1705 | integration | app turns | submit prompts | session/always injection; block before memory; internal roles none; graph worker prompt; transcript only user text; native no prepend | `src/app.rs` `agent_prompt_session_injects_on_fresh_session_only`, `agent_prompt_always_injects_every_turn`, `agent_prompt_block_precedes_memory`, `agent_prompt_internal_roles_get_nothing`, `agent_prompt_graph_worker_gets_prompt`, `agent_prompt_transcript_has_no_block`, `agent_prompt_native_runner_no_prepend` |
| TC-1716 | FR-1704 | unit | codex thread requests | build payload | developerInstructions on thread/start and thread/resume; absent without prompt; no baseInstructions | `src/agent/codex.rs` `agent_prompt_codex_thread_start_developer_instructions`, `agent_prompt_codex_thread_resume_developer_instructions`, `agent_prompt_codex_no_prompt_no_developer_instructions` |
| TC-1717 | FR-1704 | integration | custom agent run | run_turn | placeholders render; absent without prompt; opencode temp config instructions; temp files removed | `src/agent/custom.rs` `agent_prompt_custom_placeholders_render`, `agent_prompt_custom_placeholders_absent_without_prompt`, `agent_prompt_opencode_temp_config_instructions`, `agent_prompt_opencode_temp_files_removed` |
| TC-1718 | FR-1706 | integration | headless run | `dmuxai run --json` | system_prompt {source, hash, delivery}; never text | `src/app.rs` `agent_prompt_headless_json_reports_source_and_hash` |
| TC-1719 | FR-1706 | integration | dashboard API | GET/POST /api/agent-prompts | unset by default; roundtrip; null clears; size/inject validation; preview only | `src/web/server.rs` `agent_prompt_api_get_lists_runners_unset_by_default`, `agent_prompt_api_post_then_get_roundtrip`, `agent_prompt_api_post_null_clears`, `agent_prompt_api_post_over_32kib_returns_400`, `agent_prompt_api_post_unknown_inject_returns_400`, `agent_prompt_api_text_never_full_beyond_preview` |
| TC-1720 | FR-1706 | browser/e2e (ignored) | Chrome and Playwright | render panel | system prompts panel renders and saves | `tests/web_agent_prompts.rs` `agent_prompts_panel_renders_in_headless_chrome` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Conversation transcript stores only user text; prompt injection state tracked per conversation/agent in memory | No DB schema change | TC-1715 |
| Provider protocol | opencode uses temp config `instructions`; Codex uses `developerInstructions`; custom uses placeholders or prepend | Unknown fields ignored; prepend fallback preserved | TC-1716, TC-1717 |
| Security / privacy | Prompt text never appears in transcripts, headless JSON beyond preview, or dashboard responses | Preview truncated to 200 chars | TC-1718, TC-1719 |
| Config precedence | User config then workspace; per-key merge; source label records provenance | Existing keys unaffected | TC-1709 |
| Platform | Temp file cleanup after turn; process group handling unchanged | Platform-specific behavior preserved | TC-1717 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Config parsing and merge | `src/agent_prompt.rs`, `src/settings.rs` | FR-1700, FR-1701, FR-1702, FR-1703 | TC-1707 to TC-1713 |
| 2 | Delivery and turn injection | `src/agent_prompt.rs`, `src/app.rs`, `src/agent/codex.rs`, `src/agent/custom.rs` | FR-1704, FR-1705 | TC-1714 to TC-1717 |
| 3 | Headless and dashboard surfaces | `src/app.rs`, `src/cli.rs`, `src/web/server.rs`, `src/web/assets` | FR-1706 | TC-1718 to TC-1720 |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] `../reviews/REVIEW-agent-system-prompt.md` records commands, result, residual risk, and rollback notes.
PYEOF
echo written