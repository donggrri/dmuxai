# SPEC: cursor-agent-keyword

Status: draft
Owner: DmuxAI
Created: 2026-11-26
Baseline: `0d11b4f`
Decision update: 2026-11-26 owner decision (OQ-3200 closed): the canonical id becomes `cursor-agent`
Related: SPEC-open-model-lists, SPEC-cursor-model-pin, SPEC-cli-runners-models, SPEC-runner-fallback, SPEC-graph-allowed-runners

## 1. Decision summary

The Cursor runner is the `cursor-agent` binary, but DmuxAI used the id `cursor` everywhere. The owner decided (OQ-3200, closed) that `cursor-agent` becomes THE id: the canonical runner id is `cursor-agent` internally, in stored values and in every JSON/web/event/journal output. `cursor` stays a permanent legacy alias: it is accepted wherever an id is read (CLI, MCP, web requests, config keys and values, SQLite conversations, run-journal manifests, event logs, graph plans) and normalizes to `cursor-agent` at the boundary, so existing setups and data keep working without a migration step. New writes use `cursor-agent`. Matching is case-insensitive and trimmed. Help text, README, docs and the CHANGELOG name `cursor-agent` as the id and mention `cursor` as the legacy alias. This is a breaking-ish change for consumers that parse the `cursor` id from DmuxAI output.

## 2. Scope

### In scope

- One shared helper `canonical_runner_id` (new `src/runner_id.rs`, public in the library) mapping the legacy alias `cursor` to `cursor-agent`; every other id is returned trimmed and lowercased unchanged; one constant `CURSOR_RUNNER_ID = "cursor-agent"` for production code.
- Every producer of an id: internal agent names, SQLite `agent` column writes, run-journal manifests, events and progress lines, headless outcome JSON, `runners`/`models`/`status` JSON, MCP results, web API and snapshots, graph plans and stores.
- Reading legacy `cursor` values from config, SQLite conversations, run journals, event logs, graph stores and CLI/MCP/web input.
- Every parse boundary of an agent/runner id: CLI `--agent` / `--agents` (fan-out legs) / `--runner`-style flags, `dmuxai models --agent`, `runners`, config keys `[runners.<id>]`, `[graph] allowed_runners`, `[runners.<id>] fallback` entries (`cursor-agent` and `cursor-agent:<model>`), routing default agent, graph plan `runner` fields and `/graph override`, TUI agent commands, MCP tool arguments, web API request fields, headless run `--agent`, and run-journal / event readers that compare agent ids from input.
- Custom-agent name collision: `cursor-agent` is a reserved name like `cursor`, `agy` and `codex`.
- User-facing documentation and help text.

### Out of scope / non-goals

- Renaming the SQLite `agent` column, journal field names, state paths, the `[cursor]` config table or the `Agent::Cursor` enum variant (only the id VALUES change).
- Rewriting existing data on disk: legacy `cursor` values are normalized on read; they are not bulk-migrated.
- Changing the binary name (`DMUXAI_CURSOR_BIN` and `cursor-agent` lookup stay) or the `[cursor]` config table name.
- Aliases for other runners.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3200 | Closed 2026-11-26 by the owner: output and stored ids become `cursor-agent` as well; `cursor` stays a legacy input alias (FR-3207..FR-3209). | Owner | closed |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Runner id | `cursor-agent`, the one id used internally, stored and emitted. | The binary path (`DMUXAI_CURSOR_BIN`). |
| Legacy alias | `cursor`, still accepted on read and normalized to `cursor-agent`. | A second runner; there is only one Cursor runner. |
| Parse boundary | Any code that turns user, config, MCP, web or plan input into an agent/runner id. | Internal calls that already hold a canonical id. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3200 | Must | Given `canonical_runner_id(s)` | `cursor`, `CURSOR`, ` cursor ` and `cursor-agent` (any case, trimmed) return `cursor-agent`; any other input returns its trimmed lowercase form unchanged (`agy`, `opencode-glm`, `cursor-agentx`) | Empty input returns an empty string | TC-3200 |
| FR-3201 | Must | Given the CLI flags that take an agent id (`run --agent`, `models --agent`, `--agents a,b`, `status`/`runners` filters) | `cursor-agent` and the legacy `cursor` behave identically; outcome JSON reports `agent: "cursor-agent"` | An unknown id still exits 3 and its message lists `cursor-agent` among the built-in ids | TC-3201 |
| FR-3202 | Must | Given config keys `[runners.cursor-agent]`, `[runners.cursor-agent.models."<m>"]`, `[graph] allowed_runners = ["cursor-agent"]`, fallback targets `cursor-agent` / `cursor-agent:<model>`, routing default agent `cursor-agent` | Each is read as `cursor` | If both `[runners.cursor]` and `[runners.cursor-agent]` define a key, `cursor-agent` wins and one config warning names both tables | TC-3202, TC-3203 |
| FR-3203 | Must | Given graph plan JSON with `"runner": "cursor-agent"` or legacy `"cursor"`, `/graph override`, and TUI `/agent`-style commands | Accepted and stored as `cursor-agent` | Unknown runners are rejected as before | TC-3204 |
| FR-3204 | Must | Given an MCP `run`/`models` call or a web API request naming the agent as `cursor-agent` or `cursor` | Accepted; responses use `cursor-agent` | Unknown ids keep their existing error | TC-3205 |
| FR-3205 | Must | Given a custom agent named `cursor-agent` (any case) in `[[agents.custom]]` | Rejected with the existing `already used` warning | Custom agents named `cursor` behave as before | TC-3206 |
| FR-3206 | Must | Given `dmuxai runners --json` | The Cursor entry has `"id": "cursor-agent"` and `"aliases": ["cursor"]`; other entries have no alias or an empty array | Consumers that ignore the field are unaffected | TC-3207 |
| FR-3207 | Must | Given any JSON or text output that names the Cursor runner: `runners`/`models`/`status` JSON, headless outcome (`agent`, `runner`), progress/event lines, MCP results, web API and snapshots, run-journal manifests and `progress.jsonl`, graph plans/stores written by DmuxAI | The id is `cursor-agent` | No output emits `cursor` as an id any more (aliases arrays excepted) | TC-3209, TC-3211, TC-3212 |
| FR-3208 | Must | Given persisted data written by older versions: SQLite conversation rows with `agent = 'cursor'`, run-journal manifests/`progress.jsonl` with runner `cursor`, event logs, graph stores with runner `cursor` | They are read as `cursor-agent` without error and without rewriting the files; new writes use `cursor-agent`; a database holding both spellings resolves to the same agent | A truly unknown stored id keeps its current handling | TC-3209, TC-3210, TC-3213, TC-3214 |
| FR-3209 | Must | Given config written for the old id: `[runners.cursor]`, `[runners.cursor.models."<m>"]`, `allowed_runners = ["cursor"]`, fallback targets `cursor` / `cursor:<model>`, routing default agent `cursor` | Behave exactly as the `cursor-agent` spelling; mixing both spellings follows FR-3202 | No warning for the legacy spelling alone | TC-3213 |
| UX-3200 | Must | Given `--help` for `run`, `models`, `runners`, README, `docs/API.md`, other usage docs and the CHANGELOG | They name `cursor-agent` as the id of the Cursor runner and state that the legacy `cursor` still works; the CHANGELOG has a breaking-ish note for the changed output id | A doc test fails when README or `--help` lose the keyword | TC-3208 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3200 | FR-3200 | unit | the inputs listed in FR-3200 | call `canonical_runner_id` | outputs as specified | `src/runner_id.rs` `tc_3200_canonical_runner_id_maps_cursor_agent` |
| TC-3201 | FR-3201 | unit | CLI argv with `--agent cursor-agent` and `--agent cursor` for `run`, `models`, and `--agents cursor,agy`; an unknown id | parse and resolve | both spellings resolve to the same agent whose id is `cursor-agent`; the unknown-id message lists `cursor-agent` | `src/cli.rs` `tc_3201_cli_accepts_cursor_agent_keyword` |
| TC-3202 | FR-3202 | unit | user and workspace config using `[runners.cursor-agent]`, `allowed_runners`, `fallback` entries and routing default with `cursor-agent` | load settings | runner models, prompts, graph admission and fallback chain are those of `cursor` | `src/settings.rs` `tc_3202_config_accepts_cursor_agent_keyword` |
| TC-3203 | FR-3202 | unit | config with both `[runners.cursor]` and `[runners.cursor-agent]` setting `model` | load settings | `cursor-agent` value wins; one warning names both tables | `src/settings.rs` `tc_3203_cursor_and_cursor_agent_tables_conflict_warns` |
| TC-3204 | FR-3203 | unit | plan JSON with `runner: cursor-agent` and `runner: cursor`, `/graph override`, TUI parse | validate/parse | accepted, stored runner is `cursor-agent` | `src/graph/plan_doc.rs` `tc_3204_plan_runner_accepts_cursor_agent` |
| TC-3205 | FR-3204 | unit | MCP `models` call with `agent: cursor-agent` and with `cursor` through the backend, and the web agent-id parser | handle | same result for both, response id `cursor-agent` | `src/mcp_server.rs` `tc_3205_mcp_accepts_cursor_agent_keyword` |
| TC-3206 | FR-3205 | unit | settings with `[[agents.custom]] name = "Cursor-Agent"` | load settings | rejected with an `already used` warning | `src/custom_agent.rs` `tc_3206_custom_agent_cannot_take_cursor_agent_name` |
| TC-3207 | FR-3206 | unit | `runners_json` | read the cursor entry | `id == "cursor-agent"` and `aliases == ["cursor"]` | `src/agent_api.rs` `tc_3207_runners_json_lists_cursor_agent_alias` |
| TC-3208 | UX-3200 | unit | `--help` text of `run`, `models`, `runners` and the files README.md, docs/API.md, CHANGELOG.md | read | each names `cursor-agent`; README/API mention that `cursor` is still accepted; the CHANGELOG mentions the changed output id | `tests/model_policy_repo.rs` `tc_3208_docs_name_cursor_agent_keyword` |
| TC-3209 | FR-3207, FR-3208 | unit | a conversation database seeded with a legacy row `agent = 'cursor'` | open, read, write a new turn | the legacy row reads as the Cursor agent with id `cursor-agent`; the new row is stored as `cursor-agent`; both rows list under the same agent | `src/conversation.rs` `tc_3209_legacy_cursor_row_reads_as_cursor_agent` |
| TC-3210 | FR-3208 | unit | a run-journal manifest and `progress.jsonl` fixture with runner/agent `cursor` | read through the journal reader | runner normalizes to `cursor-agent`; files are not rewritten; a fresh manifest writes `cursor-agent` | `src/run_journal.rs` `tc_3210_legacy_cursor_journal_runner_normalizes` |
| TC-3211 | FR-3207 | unit | `runners_json`, `models_json`, `status_json` and the headless outcome JSON for a cursor run | read every id/agent/runner field | all equal `cursor-agent`; no field equals `cursor` | `src/agent_api.rs` `tc_3211_json_outputs_use_cursor_agent_id` |
| TC-3212 | FR-3207 | unit | the web snapshot and API agent list for the cursor runner | build | ids are `cursor-agent` and a request naming `cursor` resolves to it | `src/web/snapshot.rs` `tc_3212_web_snapshot_uses_cursor_agent_id` |
| TC-3213 | FR-3208, FR-3209 | unit | legacy config (`[runners.cursor]`, `allowed_runners = ["cursor"]`, fallback `cursor:<model>`, routing default `cursor`) and a graph store JSON with runner `cursor` | load settings and the store | all behave as `cursor-agent`; no warning for the legacy spelling alone | `src/settings.rs` `tc_3213_legacy_cursor_config_and_graph_store_normalize` |
| TC-3214 | FR-3208 | unit | an event/progress record with agent or runner `cursor` | read through the event reader | normalized to `cursor-agent`; new events emit `cursor-agent` | `src/events.rs` `tc_3214_legacy_event_agent_normalizes` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | New writes store `cursor-agent`; legacy `cursor` rows, manifests and events are normalized on read and never rewritten | No migration step; an older binary cannot read `cursor-agent` rows as Cursor, so keep a backup before downgrading | TC-3209, TC-3210 |
| JSON / MCP / web | All responses, events and journals use `cursor-agent`; `runners --json` lists `aliases: ["cursor"]` | Breaking-ish for consumers parsing `cursor` (CHANGELOG note) | TC-3207, TC-3211, TC-3212 |
| Config | `cursor-agent` and legacy `cursor` are accepted wherever a runner id is read; conflicts resolved in favour of `cursor-agent` with a warning | Existing configs unchanged | TC-3202, TC-3203, TC-3213 |
| Safety | The alias reaches the same policy, admission, sensitive-gate and model validation as `cursor`; no new execution path | A missed boundary only rejects the alias, never bypasses a policy | TC-3201, TC-3205 |

## 7. Implementation plan

See [PLAN-cursor-agent-keyword.md](../plans/PLAN-cursor-agent-keyword.md) and [TASKS-cursor-agent-keyword.md](../tasks/TASKS-cursor-agent-keyword.md).

## 8. Review and test exit criteria

- [ ] TC-3200..TC-3214 pass.
- [ ] A scan of `"cursor"` literals in parse paths shows no boundary that skips `canonical_runner_id` (recorded in the review).
- [ ] Full local gates pass: fmt, clippy `-D warnings`, `cargo test`, `check-spec`, `check-tc-links`, web ignored tests.
