# SPEC: open-model-lists

Status: draft
Owner: DmuxAI
Created: 2026-11-26
Baseline: `897c973`
Related: SPEC-model-policy, SPEC-cursor-model-pin, SPEC-cli-runners-models, SPEC-agent-graph-orchestration, SPEC-model-resolver

## 1. Decision summary

The owner wants every model that a runner's own CLI offers to be selectable in DmuxAI: Cursor models such as `claude-sonnet-5-5-high` and `grok-4.7-high`, and every model `agy models` lists (Gemini 4 Carbon/Neon/Argon, Claude, GPT-OSS, orchestrators). DmuxAI therefore stops keeping its own allow-list for the Cursor and AGY runners. Policy for those two runners becomes: any non-empty model id is accepted, except ids containing `fast`, which stay blocked for every runner unless the user opts in with `[models] allow_fast = true` in the **user** config. `dmuxai models`, the MCP `models` tool, the F2 picker and the dashboard picker list what the CLI reports (`cursor-agent --list-models`, `agy models`) with a timeout, a short in-process cache and a static fallback of verified ids. The Cursor default stays `composer-2.5`; repo graph definitions stay `composer-2.5` only. Opencode, Codex, Grok and custom-agent policies are unchanged.

Verified on 2026-11-26 against `cursor-agent 2026.11.22-e9042b2` and the box `agy`: Cursor reports `composer-2.5`, `claude-sonnet-5-5-{low,medium,high,xhigh,max}`, `grok-4.7-{low,medium,high,xhigh}` (plus `-fast` variants), `gemini-4-carbon-*` and more; AGY reports `gemini-4-{neon,carbon,argon,hydrogen}-*`, `gemini-3.8-flash-*`, `gemini-pro-orchestrator*`, `claude-opus-5-5-*`, `claude-sonnet-5-5-*`, `gpt-oss-120b-medium`.

## 2. Scope

### In scope

- `validate_model_for_runner` for `agy` (open) and the single fast rule with the user opt-in (`src/model_policy.rs`).
- The user-level `[models] allow_fast` setting and its process-wide effect (`src/settings.rs`, binary entry points).
- Dynamic model listing with timeout, cache and static fallback for `cursor` and `agy` in `collect_model_listing` (`src/agent_api.rs`), used by `dmuxai models` and the MCP `models` tool.
- `dmuxai runners --json` `model_policy` text for cursor and agy (`runners_json`).
- AGY `SelectModel` validation and a 10 s bound plus static fallback for the adapters' own `list_models` (TUI F2 / dashboard).
- F2 picker, dashboard picker and graph node/override validation reading the same policy.
- Spec amendments of FR-314, FR-1852, FR-2834, FR-2835 and the cursor-model-pin non-goal.

### Out of scope / non-goals

- Changing the Cursor default (`composer-2.5`), `docs/dmuxai-graph.yaml` or `.cursor/agents/*` (still composer-2.5 only, TC-310).
- Opencode (`opencode/*-free`), Codex (`composer-2.5` only), Grok (`grok-*` without fast) and custom agent `allowed_models` policies.
- Cursor ACP (`[cursor] protocol = "acp"`): its own non-fast model check is unchanged, and `acp_model_arg` keeps blocking fast models even when `allow_fast` is set.
- Built-in agent prompts for fast ids (`agent_prompt.rs`) and `validate_custom_model` fast handling for custom agents.
- Pricing, quota or "NO ZDR" labelling of models; provider-side model access errors are reported as provider errors.
- Any environment variable or CLI flag that enables fast models.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3000 | `grok-*` ids stay routable only to the `grok` and `cursor` runners (FR-911); AGY does not list grok models. | DmuxAI | PR |
| OQ-3001 | Codex stays `composer-2.5` only; opening it needs a separate decision about the codex catalog. | Owner | follow-up |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Open runner | A runner whose policy accepts any non-empty, non-fast model id (`cursor`, `agy`). | A runner with an allow-list (opencode, codex, grok, custom `allowed_models`). |
| Fast id | A model id whose lowercase form contains `fast` (e.g. `composer-2.5-fast`, `grok-4.7-high-fast`, `x[fast=true]`). | Models that are merely quick. |
| `allow_fast` | User-config boolean `[models] allow_fast`, default `false`, read once at process start. | Any workspace config value, which is ignored. |
| Listing source | `command` (runner CLI answered), `static` (fallback ids) or `none`. | The policy verdict `allowed`. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3000 | Must | Given `validate_model_for_runner("agy", id)` with a non-empty, non-fast id | Accepted for every id the AGY CLI can list, including `gemini-4-carbon-high`, `gemini-4-neon-xhigh`, `gemini-4-argon-low`, `gemini-pro-orchestrator`, `claude-sonnet-5-5-high`, `gpt-oss-120b-medium`, `gemini-3.8-flash-low` and `composer-2.5` | Empty id returns `Err`; `grok-*` ids stay rejected on agy with the FR-1854 misrouting message | TC-3000 |
| FR-3001 | Must | Given `validate_model_for_runner("cursor", id)` | Accepts any non-empty, non-fast id, including `claude-sonnet-5-5-high`, `grok-4.7-high`, `gemini-4-carbon-high`, `auto` and `composer-2.5` (shipped by FR-2180, kept) | Empty id returns `Err` | TC-3000 |
| FR-3002 | Must | Given any runner and an id containing `fast` (case-insensitive) and `allow_fast` is false | Rejected with `fast models are blocked by policy: <id>` | Empty ids keep their own error | TC-3001 |
| FR-3003 | Must | Given `[models] allow_fast = true` in the user config | `validate_model_for_runner` no longer applies the fast rule for open runners (`cursor`, `agy`); other runner rules (opencode `*-free`, codex, grok, custom) still apply as before | `allow_fast` lives on `DmuxSettings`, parsed only from the user config file (never from a workspace `dmuxai.toml`, which is ignored with a warning naming the key); there is no dedicated env var or flag; binaries set the process-wide switch once at startup from it; Cursor ACP always passes `allow_fast = false`; with the key absent or `false` behavior equals FR-3002 | TC-3001, TC-3008 |
| FR-3004 | Must | Given `dmuxai models --agent cursor` (CLI, MCP `models`, dashboard) | DmuxAI runs `cursor-agent --list-models` (10 s timeout), parses `<id> - <label>` lines, and reports `source: "command"` with every listed id; each model carries `allowed` from the policy, so fast ids stay in the JSON with `allowed: false` (FR-2832) unless `allow_fast` | Spawn failure, timeout, non-zero exit or zero parsed ids reports the static Cursor fallback (FR-3006) with `source: "static"` and exit 0 | TC-3003, TC-3004 |
| FR-3005 | Must | Given `dmuxai models --agent agy` | DmuxAI runs `agy models` (10 s timeout) and reports every listed id (the existing parser, banner dropped) with `source: "command"`; the listing is no longer filtered to the old flash/hydrogen set; fast ids are reported with `allowed: false` unless `allow_fast` | Failure, timeout or empty output reports the static AGY fallback (FR-3006) with `source: "static"` | TC-3004, TC-3006 |
| FR-3006 | Must | Given the static fallbacks | Cursor fallback is exactly `composer-2.5`, `claude-sonnet-5-5-medium`, `claude-sonnet-5-5-high`, `claude-opus-5-5-high`, `grok-4.7-medium`, `grok-4.7-high`, `gpt-5.5-high`; AGY fallback is every non-fast id printed by `agy models` on 2026-11-26 (`gemini-4-neon-{low,medium,high,xhigh}`, `gemini-4-carbon-{low,medium,high}`, `gemini-4-argon-{low,medium,high}`, `gemini-4-hydrogen-{low,medium,high,xhigh}`, `gemini-pro-orchestrator`, `gemini-pro-orchestrator-neon`, `gemini-pro-orchestrator-argon`, `gemini-3.8-flash-{low,medium,high}`, `claude-opus-5-5-{low,medium,high}`, `claude-sonnet-5-5-{low,medium,high}`, `gpt-oss-120b-medium`); every fallback id passes the policy for its runner and none contains `fast` | A fallback id that fails the policy is a test failure | TC-3002 |
| FR-3007 | Must | Given a successful or fallback listing for `cursor` or `agy` within 60 s of the previous one in the same process | The cached raw CLI output is returned without spawning the CLI again (policy filtering is applied on read); a failed or static result is not cached for longer than 5 s; the cache is keyed by runner, program path and workspace root | A cache miss behaves as FR-3004/FR-3005 | TC-3005 |
| FR-3008 | Must | Given the F2 picker (TUI), the dashboard model menu, `--model`, `/graph override`, graph node `model`, `[runners.<id>] model` and runner defaults | All validate through `validate_model_for_runner`, so a Cursor `claude-sonnet-5-5-high` / `grok-4.7-high` and an AGY `gemini-4-carbon-high` are selectable and reach the adapter argv unchanged; fast ids are never offered by the F2 and dashboard pickers (even with `allow_fast`) and are rejected by validation unless `allow_fast`; AGY `SelectModel` validates through the policy as Cursor does | Graph plans that name a fast model still fail validation (SPEC-agent-graph-orchestration) | TC-3009, TC-3010, TC-3011 |
| FR-3009 | Must | Given a Cursor turn with no model, or a fast model while `allow_fast` is false | Argv gets `--model composer-2.5`, and a fast model is replaced with `composer-2.5` plus the existing system notice (SPEC-cursor-model-pin FR-411 unchanged for fast ids) | A selected non-fast model is never replaced | TC-3011 |
| FR-3010 | Must | Given `dmuxai runners --json` (`runners_json`) | `runners[].model_policy` reads `any model reported by cursor-agent (default composer-2.5); fast blocked unless [models] allow_fast` for cursor and `any model reported by agy; fast blocked unless [models] allow_fast` for agy | Codex, grok, opencode and custom texts are unchanged TC-3012 |
| TC-3012 |
| FR-3011 | Must | Given the Cursor and AGY adapters' own `list_models` (TUI F2 and dashboard) | Each bounds the CLI call at 10 s and, on error, timeout or empty output, uses the FR-3006 static fallback instead of an empty menu; the AGY 24 h disk cache keeps working | A hung CLI never freezes the picker | TC-3013 |
| NFR-3000 | Must | Given validation calls | `validate_model_for_runner` stays a pure in-memory check and never spawns a process; only the listing functions spawn CLIs, bounded by 10 s and cached (FR-3007) | A hung CLI never blocks validation or F2 longer than the 10 s listing bound | TC-3005 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3000 | FR-3000, FR-3001 | unit | agy and cursor with the ids from FR-3000/FR-3001, empty id, `grok-4.7-high` on agy | `validate_model_for_runner` | non-fast ids accepted on both; empty rejected; grok misrouted on agy | `src/model_policy.rs` `tc_3000_agy_and_cursor_accept_any_non_fast_model` |
| TC-3001 | FR-3002, FR-3003 | unit | fast ids on every runner, with the pure variant `validate_model_for_runner_with(runner, id, allow_fast)` | call with `allow_fast` false and true | false: all rejected with the fast message; true: cursor/agy accept, opencode/codex/grok keep their rules; the global default is false | `src/model_policy.rs` `tc_3001_fast_blocked_unless_user_opt_in` |
| TC-3002 | FR-3006 | unit | the two static fallback lists | validate each id for its runner | all pass the policy, none contain `fast`, cursor list contains `composer-2.5`, `claude-sonnet-5-5-high`, `grok-4.7-high` | `src/agent_api.rs` `tc_3002_static_fallback_ids_pass_policy` |
| TC-3003 | FR-3004 | unit | a fake `cursor-agent` script printing `Available models`, `auto - Auto (default)`, `claude-sonnet-5-5-high - Claude Sonnet 5.5 High`, `grok-4.7-high - Grok 4.7 High`, `composer-2.5-fast - Composer 2.5 Fast` and a tip line, injected by program path, not a global env var | `collect_model_listing` for cursor | `source: "command"`; ids include sonnet and grok-4.7-high; the fast id is present with `allowed: false`; `(default)` is stripped from labels | `src/agent_api.rs` `tc_3003_cursor_listing_uses_cli_and_marks_fast_disallowed` |
| TC-3004 | FR-3004, FR-3005 | unit | fake CLIs that exit non-zero, hang past a shortened timeout, or print nothing | `collect_model_listing` for cursor and agy | static fallback with `source: "static"`, exit stays 0, no panic | `src/agent_api.rs` `tc_3004_listing_falls_back_to_static` |
| TC-3005 | FR-3007, NFR-3000 | unit | fake CLI that appends a line to a counter file per spawn | call the listing twice within the TTL, then after clearing the cache | one spawn for two calls; two spawns after clearing; a static result expires after at most 5 s | `src/agent_api.rs` `tc_3005_listing_is_cached` |
| TC-3006 | FR-3005 | unit | fake `agy models` output with banner, `gemini-4-carbon-high`, `gemini-4-neon-xhigh`, `claude-sonnet-5-5-high`, `gemini-pro-orchestrator` | `collect_model_listing` for agy | all ids listed in order, `allowed: true`, `source: "command"` | `src/agent_api.rs` `tc_3006_agy_listing_is_unfiltered` |
| TC-3007 | FR-3007 | unit | the same fake CLI at two program paths | list each once, repeat the first | the second path misses the cache; the repeat of the first hits it | `src/agent_api.rs` `tc_3007_cache_key_includes_program_path` |
| TC-3008 | FR-3003 | unit | user config with `[models] allow_fast = true`; workspace config with the same key | load settings | user value true; a workspace-only value stays false and yields a warning; absent key is false | `src/settings.rs` `tc_3008_allow_fast_is_user_config_only` |
| TC-3009 | FR-3008 | unit | graph executor/replan policy hooks with `validate_model_for_runner` | plan nodes `cursor:claude-sonnet-5-5-high`, `cursor:grok-4.7-high`, `agy:gemini-4-carbon-high`, `cursor:composer-2.5-fast` | the first three validate; the fast node is rejected | `src/graph/tasks.rs` `tc_3009_graph_nodes_accept_open_models_and_reject_fast` |
| TC-3010 | FR-3008 | unit | app with Cursor and AGY catalogs containing sonnet, grok, carbon and fast ids, with `allow_fast` unset | build the F2 / dashboard model menu | non-fast ids offered, fast ids absent, selecting carbon on agy and sonnet on cursor sticks | `src/app.rs` `tc_3010_pickers_offer_open_models_without_fast` |
| TC-3011 | FR-3008, FR-3009 | unit | Cursor `configure_command` with `claude-sonnet-5-5-high`, `grok-4.7-high`, None and `composer-2.5-fast` | build argv | the first two pass through; None and fast give `--model composer-2.5` | `src/agent/cursor.rs` `tc_3011_cursor_argv_passes_open_models_and_pins_default` |
| TC-3012 | FR-3010 | unit | `runners_json` | read cursor and agy `model_policy` | strings equal FR-3010 text; codex/grok unchanged | `src/agent_api.rs` `tc_3012_status_model_policy_text` |
| TC-3013 | FR-3011, FR-3008 | unit | fake CLIs that hang or exit non-zero for the cursor and agy adapters, injected by program path; AGY `SelectModel` with a fast id and with `gemini-4-carbon-high` | adapter `list_models`; `SelectModel` | static fallback within the bound, no empty menu; fast id rejected, carbon accepted | `src/agent/cursor.rs` `tc_3013_cursor_list_models_bounded_with_static_fallback`, `src/agent/agy.rs` `tc_3013_agy_list_models_bounded_and_select_validates` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Config | New user-level `[models] allow_fast` (default `false`); a workspace value is ignored with a warning | Additive; remove the key to restore the block | TC-3008 |
| Safety | Fast models stay blocked by default for every runner; no env var or flag can enable them; the opt-in is never read from a repository | The owner's no-fast rule is the default | TC-3001, TC-3008 |
| Provider processes | `cursor-agent --list-models` and `agy models` run only from the listing paths, 10 s timeout, stdin null, no credentials printed | Failure falls back to static ids | TC-3003, TC-3004 |
| Output | `models` JSON shape is unchanged (`source` stays `command`/`static`/`none`); more ids and `source: "command"` for cursor | Consumers that assumed a one-model cursor list must read `models` | TC-3003 |
| Rollback | Revert the policy change to restore the AGY flash/hydrogen allow-list and the one-model Cursor listing | No persistent data changes | TC-3000 |

## 7. Implementation plan

See [PLAN-open-model-lists.md](../plans/PLAN-open-model-lists.md) and [TASKS-open-model-lists.md](../tasks/TASKS-open-model-lists.md).

## 8. Review and test exit criteria

- [ ] TC-3000..TC-3013 pass.
- [ ] Amended rows in SPEC-model-policy, SPEC-cli-runners-models, SPEC-model-resolver and SPEC-cursor-model-pin match the code.
- [ ] Full local gates pass: fmt, clippy `-D warnings`, `cargo test`, `check-spec`, `check-tc-links`, web ignored tests.
- [ ] `REVIEW-open-model-lists.md` records commands, results, perf (#301) and residual risk.
