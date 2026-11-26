# SPEC: cursor-agent-keyword

Status: draft
Owner: DmuxAI
Created: 2026-11-26
Baseline: `0d11b4f`
Related: SPEC-open-model-lists, SPEC-cursor-model-pin, SPEC-cli-runners-models, SPEC-runner-fallback, SPEC-graph-allowed-runners

## 1. Decision summary

The Cursor runner is the `cursor-agent` binary, but every agent-id parse boundary in DmuxAI only knows the id `cursor`. The owner wants `cursor-agent` to be the documented keyword for running Cursor. DmuxAI therefore accepts `cursor-agent` (case-insensitive, trimmed) at every place an agent/runner id is parsed, normalizes it at the boundary to the internal canonical id `cursor`, and keeps `cursor` as a permanent backward-compatible alias. The internal id, the stored `agent` column, run journals, state files and JSON output ids stay `cursor`, so nothing migrates and existing consumers keep working. Help text, README, docs and the CHANGELOG name `cursor-agent` as the primary keyword and mention `cursor` as the alias.

## 2. Scope

### In scope

- One shared helper `canonical_runner_id` (new `src/runner_id.rs`, public in the library) mapping `cursor-agent` to `cursor`; every other id is returned trimmed and lowercased unchanged.
- Every parse boundary of an agent/runner id: CLI `--agent` / `--agents` / `--runner`-style flags, `dmuxai models --agent`, `runners`, config keys `[runners.<id>]`, `[graph] allowed_runners`, `[runners.<id>] fallback` entries (`cursor-agent` and `cursor-agent:<model>`), routing default agent, graph plan `runner` fields and `/graph override`, TUI agent commands, MCP tool arguments, web API request fields, headless run `--agent`, and run-journal / event readers that compare agent ids from input.
- Custom-agent name collision: `cursor-agent` is a reserved name like `cursor`, `agy` and `codex`.
- User-facing documentation and help text.

### Out of scope / non-goals

- Renaming the internal id, the SQLite `agent` column, journal fields, state paths or the JSON `id` of runner entries (they stay `cursor`; a new additive `aliases` array is allowed).
- Changing the binary name (`DMUXAI_CURSOR_BIN` and `cursor-agent` lookup stay) or the `[cursor]` config table name.
- Aliases for other runners.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3200 | Output ids stay `cursor` for wire compatibility; flipping output ids to `cursor-agent` would be a breaking change and needs an owner decision. | Owner | follow-up |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Keyword | The id a user types for the Cursor runner. Documented primary: `cursor-agent`; alias: `cursor`. | The binary path (`DMUXAI_CURSOR_BIN`). |
| Canonical id | The internal, stored and emitted id `cursor`. | The keyword. |
| Parse boundary | Any code that turns user, config, MCP, web or plan input into an agent/runner id. | Internal calls that already hold a canonical id. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3200 | Must | Given `canonical_runner_id(s)` | `cursor-agent`, `CURSOR-AGENT` and ` cursor-agent ` return `cursor`; `cursor` returns `cursor`; any other input returns its trimmed lowercase form unchanged (`agy`, `opencode-glm`, `cursor-agentx`) | Empty input returns an empty string | TC-3200 |
| FR-3201 | Must | Given the CLI flags that take an agent id (`run --agent`, `models --agent`, `--agents a,b`, `status`/`runners` filters) | `cursor-agent` behaves exactly like `cursor`; outcome JSON still reports `agent: "cursor"` | An unknown id still exits 3 and its message lists `cursor-agent` (not `cursor`) among the built-in ids | TC-3201 |
| FR-3202 | Must | Given config keys `[runners.cursor-agent]`, `[runners.cursor-agent.models."<m>"]`, `[graph] allowed_runners = ["cursor-agent"]`, fallback targets `cursor-agent` / `cursor-agent:<model>`, routing default agent `cursor-agent` | Each is read as `cursor` | If both `[runners.cursor]` and `[runners.cursor-agent]` define a key, `cursor-agent` wins and one config warning names both tables | TC-3202, TC-3203 |
| FR-3203 | Must | Given graph plan JSON with `"runner": "cursor-agent"`, `/graph override`, and TUI `/agent`-style commands | Accepted and stored as `cursor` | Unknown runners are rejected as before | TC-3204 |
| FR-3204 | Must | Given an MCP `run`/`models` call with `agent: "cursor-agent"` and a web API request naming that agent | Accepted; responses use the canonical id `cursor` | Unknown ids keep their existing error | TC-3205 |
| FR-3205 | Must | Given a custom agent named `cursor-agent` (any case) in `[[agents.custom]]` | Rejected with the existing `already used` warning | Custom agents named `cursor` behave as before | TC-3206 |
| FR-3206 | Must | Given `dmuxai runners --json` | Each runner entry may carry `aliases`; the cursor entry has `"aliases": ["cursor-agent"]`, `id` stays `cursor`; other entries have no alias or an empty array | Consumers that ignore the field are unaffected | TC-3207 |
| UX-3200 | Must | Given `--help` for `run`, `models`, `runners`, README, `docs/API.md`, other usage docs and the CHANGELOG | They name `cursor-agent` as the keyword for the Cursor runner and state that `cursor` still works | A doc test fails when README or `--help` lose the keyword | TC-3208 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3200 | FR-3200 | unit | the inputs listed in FR-3200 | call `canonical_runner_id` | outputs as specified | `src/runner_id.rs` `tc_3200_canonical_runner_id_maps_cursor_agent` |
| TC-3201 | FR-3201 | unit | CLI argv with `--agent cursor-agent` for `run`, `models`, and `--agents cursor-agent,agy`; an unknown id | parse and resolve | parsed agent equals the one for `cursor`; the unknown-id message lists `cursor-agent` | `src/cli.rs` `tc_3201_cli_accepts_cursor_agent_keyword` |
| TC-3202 | FR-3202 | unit | user and workspace config using `[runners.cursor-agent]`, `allowed_runners`, `fallback` entries and routing default with `cursor-agent` | load settings | runner models, prompts, graph admission and fallback chain are those of `cursor` | `src/settings.rs` `tc_3202_config_accepts_cursor_agent_keyword` |
| TC-3203 | FR-3202 | unit | config with both `[runners.cursor]` and `[runners.cursor-agent]` setting `model` | load settings | `cursor-agent` value wins; one warning names both tables | `src/settings.rs` `tc_3203_cursor_and_cursor_agent_tables_conflict_warns` |
| TC-3204 | FR-3203 | unit | plan JSON with `runner: cursor-agent`, `/graph override cursor-agent`, TUI parse of `cursor-agent` | validate/parse | accepted, stored runner is `cursor` | `src/graph/plan_doc.rs` `tc_3204_plan_runner_accepts_cursor_agent` |
| TC-3205 | FR-3204 | unit | MCP `models` call with `agent: cursor-agent` through the backend, and the web agent-id parser | handle | same result as `cursor`, canonical id in the response | `src/mcp_server.rs` `tc_3205_mcp_accepts_cursor_agent_keyword` |
| TC-3206 | FR-3205 | unit | settings with `[[agents.custom]] name = "Cursor-Agent"` | load settings | rejected with an `already used` warning | `src/custom_agent.rs` `tc_3206_custom_agent_cannot_take_cursor_agent_name` |
| TC-3207 | FR-3206 | unit | `runners_json` | read the cursor entry | `id == "cursor"` and `aliases == ["cursor-agent"]` | `src/agent_api.rs` `tc_3207_runners_json_lists_cursor_agent_alias` |
| TC-3208 | UX-3200 | unit | `--help` text of `run`, `models`, `runners` and the files README.md, docs/API.md, CHANGELOG.md | read | each contains `cursor-agent` and the README/API mention that `cursor` is still accepted | `tests/model_policy_repo.rs` `tc_3208_docs_name_cursor_agent_keyword` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Unchanged: agent ids stored and journaled are `cursor` | No migration; rollback is code-only | TC-3201 |
| JSON / MCP / web | Responses keep `cursor`; `runners --json` adds `aliases` | Additive | TC-3207 |
| Config | `cursor-agent` accepted wherever `cursor` is; conflicts resolved in favour of `cursor-agent` with a warning | Existing configs unchanged | TC-3202, TC-3203 |
| Safety | The alias reaches the same policy, admission, sensitive-gate and model validation as `cursor`; no new execution path | A missed boundary only rejects the alias, never bypasses a policy | TC-3201, TC-3205 |

## 7. Implementation plan

See [PLAN-cursor-agent-keyword.md](../plans/PLAN-cursor-agent-keyword.md) and [TASKS-cursor-agent-keyword.md](../tasks/TASKS-cursor-agent-keyword.md).

## 8. Review and test exit criteria

- [ ] TC-3200..TC-3208 pass.
- [ ] A scan of `"cursor"` literals in parse paths shows no boundary that skips `canonical_runner_id` (recorded in the review).
- [ ] Full local gates pass: fmt, clippy `-D warnings`, `cargo test`, `check-spec`, `check-tc-links`, web ignored tests.
