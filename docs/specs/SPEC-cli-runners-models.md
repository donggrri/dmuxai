# SPEC: cli-runners-models

Status: Implemented

Owner: dmuxai
Created: 2026-11-18
Baseline: `0503518`
Related: SPEC-agent-api FR-760..FR-767, SPEC-model-policy FR-910/FR-911, SPEC-agy-bare-model, SPEC-grok-runner FR-896/FR-899, SPEC-runner-auth-error-kind FR-2280..FR-2282, SPEC-terminal-sessions FR-2617

## 1. Decision summary

A caller that drives DmuxAI headlessly (Grok Bot, scripts, MCP clients) cannot
discover which runners exist, which ones it may use, or which model ids each
runner accepts, and several messages point it the wrong way: `dmuxai run --help`
prints the root usage, an invalid agy model is rejected with "only composer-2.5
is permitted", a logged-out grok turn is reported as `provider`, and
`status --json` reports the built-in grok/opencode presets as `allowed: false`
while `run --agent grok` is in fact admitted when `allowed_runners` is unset.
This change adds per-subcommand help, `dmuxai runners` / `dmuxai models` (and an
MCP `models` tool), admits the Gemini 4 Hydrogen family on agy with an accurate
policy message, classifies grok login failures as `auth`, and makes `status`
report headless admission and graph admission separately.

## 2. Scope

### In scope

- Subcommand-local help for every agent-facing subcommand.
- `dmuxai runners [--json]` and `dmuxai models [--agent ID] [--json]`, plus an MCP `models` tool.
- agy model policy: `gemini-4-hydrogen-*` admitted; runner-accurate rejection messages.
- agy model listing from `agy models`.
- grok turn failures that say the CLI is not signed in map to `error_kind: "auth"`.
- `status --json` `allowed` matches headless `run` admission; new `graph_allowed` field.

### Out of scope / non-goals

- Changing which runners graph routing admits (FR-899 graph exclusion of built-in presets stays).
- Admitting other Gemini 4 variants (Argon, Orchestrator) or non-Gemini agy models (Claude, GPT-OSS).
- Logging the user in, or any OAuth flow.
- Probing authentication in `dmuxai runners` (it must not spawn provider processes).
- Removing `composer-2.5` from the agy policy (kept for backward compatibility).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2830 | `agy models` prints one `<id>\t<label>` line per model after a `Fetching available models...` banner (observed with the agy CLI on 2026-11-18). | dmuxai | Implementation |
| OQ-2831 | Whether agy should stop accepting `composer-2.5` is left to the owner; this spec keeps it. | 동진 | Later |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Headless admission | Whether `dmuxai run --agent ID` passes the `[graph] allowed_runners` check (exit 5 otherwise). With the list unset every registered runner is admitted. | Graph admission |
| Graph admission | Whether graph routing may assign the runner (`graph_runner_allowed`). With the list unset, built-in grok/opencode presets are excluded (SPEC-grok-runner FR-899). | Headless admission |
| Installed | The runner's executable resolves on this machine (no process is started). | Authenticated |
| Model catalog | Ids a runner reports through its model command, or a static list when it has none. | Model policy |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2830 | Must | Given any agent-facing subcommand (`run`, `todo`, `status`, `events`, `mcp`, `init`, `trust`, `runs`, `runners`, `models`), when it is invoked with `--help` or `-h` before any `--` | DmuxAI prints that subcommand's own usage (first line `Usage: dmuxai <subcommand>`), listing its flags, and exits 0. `todo <op> --help` prints the todo usage. `term` keeps FR-2617. | `run -- --help` still treats `--help` as prompt text. The root `dmuxai --help` is unchanged. | TC-2830 |
| FR-2831 | Must | Given a workspace and config, when `dmuxai runners [--json]` runs | It prints one entry per registered runner in `status` order with `id`, `label`, `kind` (`builtin`, `preset`, or `custom`), `allowed` (headless admission), `graph_allowed`, `installed` (bool or null when unknown), `default_model` (string or null), and `model_policy` (human-readable rule). JSON is `{ "ok": true, "runners": [...] }`. | No provider process is spawned. An unknown flag is a usage error (exit 2). | TC-2831 |
| FR-2832 | Must | Given `dmuxai models [--agent ID] [--json]` | For each runner (or only `ID`) it reports `models` as `[{ "id", "label"?, "allowed" }]` where `allowed` is the runner model policy verdict, `source` (`command`, `static`, or `none`), and `error`/`error_kind` when listing failed. JSON is `{ "ok": true, "runners": [{ "id", "allowed", "source", "models", ... }] }`. Each model command runs with a 10 s timeout. | Unknown `--agent` exits 3 with `error_kind` null and message `unknown agent '<id>'`. A failing/timeout listing reports `source: "none"` with `error`, exit stays 0, except the cursor and agy runners, which report their static fallback with `source: "static"` (SPEC-open-model-lists FR-3004, FR-3005). A grok listing that is not authenticated reports `error_kind: "auth"`. Runners with a models_filter report the ids after the filter. | TC-2832, TC-2833 |
| FR-2833 | Must | Given the agy runner, when its models are listed | DmuxAI runs `agy models`, ignores non-model lines, and takes the first tab- or whitespace-separated token of each line that starts with a lowercase letter or digit as the id and the rest as the label, keeping order and dropping duplicates. | Without `agy` on PATH or on failure, the static agy catalog (SPEC-open-model-lists FR-3006) is used with `source: "static"`. | TC-2834 |
| FR-2834 | Must | Given `validate_model_for_runner("agy", id)` | Ids whose lowercase form starts with `gemini-4-hydrogen` (e.g. `gemini-4-hydrogen-low/medium/high/xhigh`) are accepted, in addition to `gemini-*-flash*` and `composer-2.5`. Superseded in part by SPEC-open-model-lists FR-3000: every non-fast id is accepted on agy. `fast` ids stay rejected (FR-910, FR-3002). | Empty ids and `fast` ids are rejected; `gemini-4-argon-*`, `gemini-4-pro`, `claude-*` are accepted since FR-3000. | TC-2835 |
| FR-2835 | Must | Given a rejected model on agy (or any built-in runner) | The message names the runner and its actual rule, e.g. `fast models are blocked by policy: <id>` or `model grok-4.7 is not allowed for agy; only grok and cursor runners accept grok models` (agy no longer has a flash/hydrogen allow-list, FR-3000). It never says only composer-2.5 is permitted unless that is the runner's rule (codex). | Grok and cursor messages from FR-1854 are unchanged. | TC-2836 |
| FR-2836 | Must | Given a grok turn (ACP `grok agent stdio` path or the streaming-json preset) or any ACP runner turn that fails with an error whose text matches a not-signed-in pattern (case-insensitive: `authentication required`, `auth_required`, `no auth method`, `not signed in`, `not logged in`, `not authenticated`, `grok login`). Observed grok ACP text: `{"code":-32000,"data":"no auth method id provided","message":"Authentication required"}` | The turn error kind is `auth`, headless JSON `error_kind` is `"auth"`, and the message keeps the original text and appends a login hint: for grok ``Run `grok login` (or set XAI_API_KEY) and retry.``, for other runners ``Sign in to the <runner> CLI and retry.`` Runner fallback does not trigger (FR-2282). | Failures without those patterns keep their previous kind (`provider`, `process_exit`, ...). `cancelled` is never reclassified. | TC-2837 |
| FR-2837 | Must | Given documentation of headless/MCP error kinds | `docs/API.md` and the MCP `run` tool description list `auth` with its meaning; the CHANGELOG notes the reclassification (callers that matched `provider` for login errors must also accept `auth`). | — | TC-2838 |
| FR-2838 | Must | Given `[graph] allowed_runners` unset (no `--config`, or a config without the key) | `status --json` reports `allowed: true` for every registered runner, matching `run --agent` admission, and `graph_allowed: false` for the built-in grok/opencode presets (FR-899). With the list set, `allowed` and `graph_allowed` both equal list membership. | An empty list admits nothing (`allowed: false` for all). | TC-2839 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2830 | FR-2830 | unit | none | `parse_cli` for each subcommand with `--help`/`-h`; `run -- --help` | `HelpTopic` whose text starts with `Usage: dmuxai <sub>`; `run -- --help` stays a run with prompt `--help` | `src/main.rs` `tc_2830_subcommand_help_is_local` |
| TC-2831 | FR-2831 | unit | temp workspace with a custom agent and allowed_runners unset / set | build runners JSON | entries, fields, `allowed`/`graph_allowed` as specified | `src/agent_api.rs` `tc_2831_runners_json_fields_and_admission` |
| TC-2832 | FR-2832 | unit | parse args | `models --agent agy --json`, `runners --json`, bad flag | parsed commands; bad flag is usage error | `src/cli.rs` `tc_2832_parse_runners_and_models` |
| TC-2833 | FR-2832 | unit | fake listing results (ok, failure, grok not authenticated) | build models JSON | `allowed` per model from policy; `source`; `error_kind: auth` for grok login | `src/agent_api.rs` `tc_2833_models_json_policy_and_errors` |
| TC-2834 | FR-2833 | unit | `agy models` stdout fixture incl. banner | parse | ids in order incl. `gemini-4-hydrogen-high`, banner dropped, labels kept | `src/agent_api.rs` `tc_2834_parse_agy_models_output` |
| TC-2835 | FR-2834 | unit | none | `validate_model_for_runner("agy", …)` | hydrogen, argon, pro and claude accepted since FR-3000; fast and empty rejected | `src/model_policy.rs` `tc_2835_agy_accepts_gemini_4_hydrogen` |
| TC-2836 | FR-2835 | unit | none | reject `grok-4.7` on agy, `x` on codex | agy message names the grok-routing rule and not "only composer-2.5"; codex message keeps composer-2.5 | `src/model_policy.rs` `tc_2836_agy_rejection_message_reflects_policy` |
| TC-2837 | FR-2836 | unit | the observed grok ACP error JSON, the grok CLI `Not signed in` text, a 429 text, `cancelled` | classify the failure with the shared helper for grok and for another runner | login texts → `auth` with the runner-specific hint; others keep their kind | `src/agent/mod.rs` `tc_2837_login_failure_is_auth` |
| TC-2838 | FR-2837 | unit | repo docs and MCP tool definitions | read | `docs/API.md` and MCP run description mention `auth`; MCP lists a `models` tool | `src/mcp_server.rs` `tc_2838_mcp_documents_auth_and_models_tool` |
| TC-2839 | FR-2838 | unit | settings with builtin grok/opencode presets; list unset, set, empty | `status_json` agents | unset: allowed true, graph_allowed false for presets; set: both equal membership; empty: all false | `src/agent_api.rs` `tc_2839_status_allowed_matches_headless_run` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| `status --json` | `allowed` now means headless admission; new `graph_allowed` | Callers that used `allowed` for graph routing should read `graph_allowed`; noted in CHANGELOG | TC-2839 |
| Headless JSON / MCP | `error_kind: "auth"` for grok login turn failures (previously `provider`) | Additive value already defined by FR-2280 | TC-2837, TC-2838 |
| New CLI / MCP tool | `runners`, `models`, MCP `models` | Additive | TC-2831..TC-2833 |
| Provider processes | `models` spawns model-list commands (10 s timeout); `runners` spawns nothing | No credentials are read or printed | TC-2833 |

## 7. Implementation plan

See [PLAN-cli-runners-models.md](../plans/PLAN-cli-runners-models.md).

## 8. Review and test exit criteria

- [x] TC-2830..TC-2839 pass.
- [ ] Full local gates (fmt, clippy -D warnings, cargo test, check-spec, check-tc-links, gitleaks) pass.
