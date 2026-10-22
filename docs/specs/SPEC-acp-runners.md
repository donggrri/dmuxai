# SPEC: ACP runners and single agent mode

Status: In progress (supersedes PRs #78/#79)
Owner: DmuxAI
Created: 2026-10-20
Baseline: `a1538e3`
Related: SPEC-grok-runner, SPEC-cursor-model-pin, SPEC-plan-gate (#67), FR-932 (judge read-only turns)

## 1. Decision summary

Cursor and the built-in grok runner use the [Agent Client Protocol](https://agentclientprotocol.com) over stdio (`cursor-agent acp`, `grok agent stdio`) so streaming, tool calls, permission prompts, cancel, and session resume share one integration path. A `[cursor]` / `[agents]` `protocol = "acp" | "legacy"` switch (default `acp`) keeps the previous CLI formats available. AGY stays on `agy --print` stream-json until its ACP server ships; the runner abstraction allows moving it later.

DmuxAI removes user-facing **ExecutionMode** (Ask / Plan / Implement): every user turn runs in a single agent mode equivalent to former Implement (workspace writes and normal tool use). Plan approval (#67), sensitive-step gates, and internal read-only judge/plan/replan turns (FR-932) stay. Legacy runners always use their former Implement CLI flags. Stored or API `mode` values are ignored once with a warning.

**Platform:** Behavior in this spec is supported on **Linux only**. Existing Windows code in the repository is unchanged and out of scope; this spec does not add Windows- or WSL-specific handling or documentation for ACP or single agent mode.

## 2. Scope

### In scope

- ACP client (JSON-RPC 2.0 newline-delimited over child stdio): `initialize`, `session/new` / `session/resume`, `session/prompt`, `session/update` → activity/events, `session/cancel`, inbound `session/request_permission` with workspace and sensitive-step policy
- Cursor and grok presets wired to ACP; model selection unchanged (no `-fast` models); `protocol` config per runner
- Fake ACP agent fixture for deterministic tests
- Removal of `/mode`, mode pickers (TUI and dashboard), mode in snapshots/API where it exposed Ask/Plan/Implement; `TurnPolicy::ReadOnly` on internal turns only
- CHANGELOG, README, superseded notes on older mode specs

### Out of scope / non-goals

- AGY and opencode on ACP (both stay on their headless CLI modes; FR-1418)
- Removing or refactoring existing Windows-specific code paths elsewhere in the repo
- Windows/WSL caveats or new Windows behavior for ACP or single agent mode
- Release workflow changes

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1300 | Cursor and grok ACP servers speak ACP v1 with blocking `session/prompt` completion (stop reason on response or terminal `session/update`). | DmuxAI | Live smoke on Linux |
| OQ-1301 | `grok agent stdio` accepts `-m`, optional `--reasoning-effort`, and `--always-approve` for agent-mode turns. | DmuxAI | Live smoke on Linux |
| OQ-1326 | Native sandbox for ACP stdio agents (`cursor-agent acp --sandbox enabled` is best-effort; grok ACP sandbox flags unknown). | DmuxAI | Live smoke on Linux |

No Must requirement depends on an unanswered question for CI; live smoke is manual on Linux.

## 3. Terms

| Term | Meaning |
| --- | --- |
| Agent mode | The only user-facing run policy: workspace writes and tools allowed subject to gates (former Implement). |
| Read-only turn | Internal judge/plan/replan adapter turn; no user mode selector. |
| Legacy protocol | Prior headless CLI (Cursor `stream-json`, grok `streaming-json`). |
| ACP client | DmuxAI side of stdio JSON-RPC talking to the vendor ACP agent process. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure / boundary | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1300 | Must | `[cursor] protocol = "acp"`, Cursor binary resolvable. | Turns spawn `cursor-agent acp` (or `DMUXAI_CURSOR_BIN` + `acp`). Each turn runs `initialize`, then `session/load` or `session/resume` when capabilities allow, else `session/new`. Prompt via `session/prompt`. Session id persisted. | Default `protocol` is `legacy` (FR-1313). Child reaped on end/cancel/error; `session/cancel` on user cancel. | TC-1300, TC-1305, TC-1316 |
| FR-1301 | Must | Built-in grok registered, `grok_protocol` unset or `acp`. | Turns spawn `grok agent -m <model> [--reasoning-effort …] stdio` (flags before `stdio`). dmuxai answers `session/request_permission`; no `--always-approve`. | `protocol = "legacy"` uses SPEC-grok-runner argv. | TC-1301, TC-1305, TC-1316 |
| FR-1302 | Must | ACP turn in progress. | `session/update` maps to `TextDelta`, `Activity`, plan/tool events consistent with existing UI. Debug logs name ACP methods/stop reasons, never prompt bodies or secrets. | Malformed lines logged and skipped where safe. | TC-1302 |
| FR-1303 | Must | User cancels or child crashes. | `session/cancel` sent when possible; no orphan agent process after turn end. | Crash → `Error` event and non-zero handling. | TC-1303, TC-1304 |
| FR-1304 | Must | Agent sends `session/request_permission`. | Allow workspace read/write and normal tools; deny paths outside workspace; defer to existing sensitive-step gates when applicable; otherwise allow selected option. | Cancelled permission → turn ends cleanly. | TC-1306, TC-1307 |
| FR-1305 | Must | `protocol = "legacy"` for Cursor or grok. | Behavior matches pre-ACP release for that runner (no ACP spawn). | Cursor default `legacy`; grok default `acp`. | TC-1308, TC-1305 |
| FR-1306 | Must | User sends prompt (any runner). | No `/mode` command; no mode picker; adapters run agent-mode CLI (or ACP) without Ask/Plan. | — | TC-1309 |
| FR-1307 | Must | Internal judge/plan/replan turn (FR-932). | `TurnPolicy::ReadOnly` on the request; adapter uses read-only CLI/ACP behavior; user agent policy unchanged after turn. | No read-only support → existing fallback paths. | TC-1333 |
| FR-1309 | Must | Docs updated. | README and CHANGELOG describe ACP default, legacy switch, single agent mode, Linux-only support for this feature set. Older mode specs carry a superseded note. | — | TC-1312 |
| NFR-1300 | Must | ACP or legacy turn. | MCP merge behavior unchanged from baseline. | — | TC-1300 |
| NFR-1301 | Must | This spec’s ACP and agent-mode contracts. | Documented supported platform is Linux; no new Windows/WSL documentation for these features. | — | TC-1491 |
| FR-1313 | Must | Cursor `protocol = "acp"`. | After `session/new`/`load`, if effective model is a fast variant and cannot be switched off, refuse turn with clear error; emit `ModelSelected` with agent-reported model, not requested id. | Default Cursor protocol is `legacy`. | TC-1318 |
| FR-1314 | Must | ACP `session/request_permission` paths. | Only structured path fields are checked; normalized component-wise workspace prefix ( `/proj-evil` ∉ `/proj` ). | Session ids/titles are not paths. | TC-1313, TC-1314 |
| FR-1315 | Must | ACP debug / stderr. | Trace via `DMUXAI_ACP_LOG` file only (not stderr). Child stderr drained; tail included in errors. | — | TC-1491 |
| FR-1316 | Must | User cancel during ACP turn. | `session/cancel`, brief grace, then process-group kill. | No orphan `cursor-agent acp`. | TC-1317 |
| FR-1317 | Must | `TurnPolicy::ReadOnly` internal turns. | ACP `session/set_mode` ask/plan (or equivalent) before prompt. | — | TC-1333 |
| FR-1318 | Must | CLI `dmuxai run` / `todo run`. | `--mode` is rejected with error (not silently ignored). | — | TC-1492 |
| FR-1319 | Must | Cursor ACP `session/new`/`load` result. | Read `models.currentModelId` (and `availableModels`); if fast, call `session/set_model` or `session/set_config_option` when advertised; refuse if still fast; `ModelSelected` uses agent-reported id. | — | TC-1318, TC-1319 |
| FR-1320 | Must | `session/load` / `session/resume`. | Params include `cwd` and `mcpServers: []`; load/resume failure surfaces a user-visible notice before `session/new`. | — | TC-1323 |
| FR-1321 | Must | `session/set_mode` for read-only turns. | JSON uses `modeId` (not `mode`); if set_mode fails or is unsupported, deny write/exec permissions for that turn (fail closed). | — | TC-1324 |
| FR-1322 | Must | Permission options from agent. | Prefer `allow_once`; never auto-select `allow_always`. | — | TC-1325 |
| FR-1323 | Must | User cancel during prompt or blocking RPC. | Cancel is polled independently of agent output; `session/cancel` is sent before kill. | — | TC-1317 |
| FR-1324 | Must | Permission denied with no assistant text. | Superseded by FR-1416: turn ends `Cancelled` (never `Completed` / `ok: true`). | — | TC-1491 |
| FR-1325 | Must | ACP tool UI noise. | Suppress generic `tool` / `Tool [completed]: tool` rows; keep meaningful tool activity (edit, execute with command). | — | TC-1315 |
| FR-1327 | Must | ACP child lifecycle. | Every exit path (success, error, cancel) terminates the agent process group; no orphans after session-open failures. | — | TC-1329 |
| FR-1328 | Must | Internal read-only turns. | Select `modeId` from `modes.availableModes`; apply `session/set_mode`; refuse the turn if no read-only mode applies (do not run writable agent mode). | — | TC-1324 |
| FR-1329 | Must | Permission denied. | Superseded by FR-1416: denied permission ends `Cancelled`, not `Completed` / `ok: true`. | — | TC-1491 |
| FR-1330 | Must | Cursor fast-model selection. | Non-fast switch only within the requested model family (e.g. `composer-2.5`), never unrelated models. | — | TC-1328 |

### 4.1 Round 5: Cursor/grok hardening (2026-10-22)

Live probe on Linux (cursor-agent, grok 1.0.67): Cursor `session/new` returns both `modes` and `configOptions` (`mode`: agent/plan/ask; `model`) and its only composer-2.5 entry is `composer-2.5[fast=true]` for every `--model` value; grok returns no `modes`, only `configOptions` (`model`, `reasoning_effort`), and still sends `session/request_permission` for edits under `--permission-mode plan`. `--permission-mode` is a root grok flag and must precede `agent`.

| ID | Priority | Preconditions / trigger | Required observable result | Failure / boundary | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1410 | Must | Cursor runner, no `protocol` set. | Default protocol is `legacy` stream-json with non-fast `composer-2.5`. | — | TC-1410 |
| FR-1411 | Must | Cursor `protocol = "acp"`, agent-reported model is a fast variant (`-fast` or `fast=true`). | Turn is refused with an error naming the reported model before any `session/prompt` is sent; child reaped. | No silent fallback to another model. | TC-1411, TC-1412 |
| FR-1412 | Must | Built-in grok, ACP. | Root options (`--permission-mode`, `-m`, `--reasoning-effort`) precede `agent stdio`; read-only turns add `--permission-mode plan`. Default protocol `acp`; `legacy` keeps streaming-json. | — | TC-1413, TC-1414 |
| FR-1413 | Must | Read-only internal turn. | Mode is applied via `session/set_mode` when `modes` is advertised, else `session/set_config_option` (`configId` with category `mode`) when advertised; if neither, the turn still runs but every write/exec/edit permission is denied (read_only_fail_closed is on for every ACP runner). | Never runs writable without the client-side deny. | TC-1415, TC-1416, TC-1417 |
| FR-1414 | Must | Any ACP `session/new`/`load`/`resume`. | `mcpServers` carries the runner's configured MCP servers (stdio entries from dmuxai MCP settings) instead of a hardcoded empty list. | Empty list when none configured. | TC-1418 |
| FR-1415 | Must | Requested model not in the allowed list for an ACP runner. | Turn fails with an error naming the model; `--model` is never dropped silently. | — | TC-1419 |
| FR-1416 | Must | Permission denied by policy or user cancel during an ACP turn. | Turn ends as `Cancelled` (stop reason `cancelled`) with a notice naming the denied tool; graph/task UI shows cancelled, not "Graph failed". | Agent crash still ends in `Error`. | TC-1420, TC-1421 |
| FR-1417 | Must | ACP `execute` tool call. | Activity row shows the command (`rawInput.command` or title), not the cwd. | — | TC-1422 |
| FR-1418 | Must | agy and opencode runners. | Keep existing headless modes (`agy --print` stream-json, `opencode run` jsonl); no ACP path or `*_protocol` switch for them. | — | TC-1423 |
| NFR-1410 | Must | Test fixtures. | Fake ACP agent can advertise `modes` only, `configOptions` only, both, or a fast-only model. | — | TC-1424, TC-1425 |


## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup | Action | Oracle | Location |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1410 | FR-1410 | unit | Empty settings | Load | Cursor protocol `legacy` | `src/settings.rs` |
| TC-1411 | FR-1411 | unit | `composer-2.5[fast=true]` / `composer-2.5-fast` | `is_fast_model` | true | `src/agent/acp/session.rs` |
| TC-1412 | FR-1411 | integration | Fake agent fast-only | Cursor ACP turn | Error, no prompt observed | `src/agent/cursor.rs` |
| TC-1413 | FR-1412 | unit | `grok_acp_argv` read-only | Build argv | `--permission-mode plan` before `agent` | `src/agent/acp_runner.rs` |
| TC-1414 | FR-1412 | unit | `grok_acp_argv` agent mode | Build argv | `agent` then `stdio` last, no `--permission-mode` | `src/agent/acp_runner.rs` |
| TC-1415 | FR-1413 | unit | `modes` advertised | `read_only_mode_request` | `session/set_mode` | `src/agent/acp/session.rs` |
| TC-1416 | FR-1413 | unit | `configOptions` mode only | `read_only_mode_request` | `session/set_config_option` | `src/agent/acp/session.rs` |
| TC-1417 | FR-1413 | unit | No mode support | Permission policy read-only | Edit denied | `src/agent/acp/permissions.rs` |
| TC-1418 | FR-1414 | unit | MCP settings with stdio server | `session_new_params` | `mcpServers` populated | `src/agent/acp/session.rs` |
| TC-1419 | FR-1415 | unit | Disallowed model | `acp_model_arg` | Err | `src/agent/acp_runner.rs` |
| TC-1420 | FR-1416 | integration | Fake agent denies outside-workspace permission | Cursor ACP turn | `Cancelled` + notice, no `Error` | `src/agent/cursor.rs` |
| TC-1421 | FR-1416 | unit | Cancelled graph step | `graph_stop_notice` | "Graph cancelled" | `src/app.rs` |
| TC-1422 | FR-1417 | unit | `execute` tool_call with command | Map update | Command in row | `src/agent/acp/updates.rs` |
| TC-1423 | FR-1418 | unit | Settings with `opencode_protocol` | Load | No ACP for opencode | `src/settings.rs` |
| TC-1424 | NFR-1410, FR-1413 | integration | Fake `FAKE_ACP_MODES` = modes / config / none | Read-only Cursor ACP turn | set_mode ask / set_config_option plan / none | `src/agent/cursor.rs` |
| TC-1425 | NFR-1410, FR-1413 | integration | Fake `FAKE_ACP_MODES=both` | Read-only Cursor ACP turn | set_mode preferred | `src/agent/cursor.rs` |
| TC-1480 | FR-1412, FR-1301 | manual | Built `dmuxai`, grok logged in, temp git workspace | `dmuxai run --agent grok "create hello.txt containing hi"` | File created in workspace; reply text shown | hands-on |
| TC-1481 | FR-1416, FR-1314 | manual | Same | Ask grok to write `/tmp/outside-<rand>.txt` | No file outside workspace; turn ends cancelled with a denial notice (not "Graph failed") | hands-on |
| TC-1482 | FR-1417 | manual | Same | Ask grok to run `ls -1 \| wc -l` | Reply has the count; activity row shows the command | hands-on |
| TC-1483 | FR-1410 | manual | cursor-agent logged in | `dmuxai run --agent cursor "say hi"` | Legacy stream-json with `composer-2.5` (non-fast) replies | hands-on |
| TC-1491 | NFR-1301, FR-1315, FR-1324, FR-1329 | review | PR diff | Code review + `DMUXAI_ACP_LOG=/tmp/x.log dmuxai run --agent grok "say hi"` | No ACP trace on stderr; trace in the log file; no Windows/WSL doc added; denied permission ends cancelled (see TC-1420) | review / hands-on |
| TC-1492 | FR-1318 | unit | CLI args | `dmuxai run x --mode ask` | Parse error, not ignored | `src/cli.rs` (`--mode` rejection asserts) |
| TC-1484 | FR-1411 | manual | `[agents] cursor_protocol = "acp"` | `dmuxai run --agent cursor "say hi"` | Refused before any prompt: error names the fast model | hands-on |
| TC-1300 | FR-1300, NFR-1300 | integration | Fake ACP binary as `DMUXAI_CURSOR_BIN` parent | Send prompt | Streaming text, `Completed`, session bound | `src/agent/acp.rs` |
| TC-1301 | FR-1301 | unit | grok preset + `acp` protocol | Build argv | Contains `agent`, `stdio`, `-m` | `src/custom_agent.rs` |
| TC-1302 | FR-1302 | unit | Fixture NDJSON | Parse updates | Expected `AgentEvent` sequence | `src/acp/` |
| TC-1303 | FR-1303 | integration | Fake agent | Cancel mid-turn | `Cancelled`, child gone | `src/agent/acp.rs` |
| TC-1304 | FR-1304 | integration | Fake agent exits 1 | Prompt | `Error` surfaced | `src/agent/acp.rs` |
| TC-1305 | FR-1300, FR-1301 | unit | Settings parse | Load toml | Cursor `legacy`, grok `acp` | `src/settings.rs` |
| TC-1306 | FR-1304 | unit | Permission payload outside workspace | Policy | Deny | `src/acp/permissions.rs` |
| TC-1307 | FR-1304 | unit | Permission with sensitive gate active | Policy | Gate path | `src/acp/permissions.rs` |
| TC-1308 | FR-1305 | integration | `protocol=legacy` + fake legacy cursor | Prompt | No `acp` in argv | `src/agent/cursor.rs` |
| TC-1309 | FR-1306 | unit | App snapshot / slash list | Inspect | No `/mode` | `src/app.rs` |
| TC-1333 | FR-1307, FR-1317 | integration | Judge turn | Start judge | Read-only argv/policy | `src/app.rs` |
| TC-1312 | FR-1309 | manual | Read docs | — | Linux-only stated | `docs/specs/SPEC-acp-runners.md` |
| TC-1313 | FR-1314 | unit | Permission JSON with sessionId | Policy | Allow in-workspace shell | `src/agent/acp/permissions.rs` |
| TC-1314 | FR-1314 | unit | `/proj` vs `/proj-evil` | Prefix check | Deny outside | `src/agent/acp/permissions.rs` |
| TC-1315 | FR-1302, FR-1325 | unit | `tool_call_update` payload | Map update | Title/kind/status | `src/agent/acp/updates.rs` |
| TC-1316 | FR-1301 | unit | `grok_acp_argv` | Build argv | `-m` before `stdio` | `src/agent/acp_runner.rs` |
| TC-1317 | FR-1316, FR-1323 | integration | Fake agent + cancel | Cancel turn | `session/cancel` observed | `tests/fixtures/fake_acp_agent.rs` |
| TC-1318 | FR-1313 | unit | Fast model payload | `cursor_acp_model_check` | Error | `src/agent/acp/session.rs` |
| TC-1319 | FR-1319 | unit | `currentModelId` fast vs requested | `cursor_acp_model_check` | Uses reported id | `src/agent/acp/session.rs` |
| TC-1323 | FR-1320 | unit | `session_load_params` | Inspect JSON | `cwd`, `mcpServers` | `src/agent/acp/session.rs` |
| TC-1324 | FR-1321, FR-1328 | unit | `session_set_mode_params` | Inspect JSON | `modeId` present | `src/agent/acp/session.rs` |
| TC-1325 | FR-1322 | unit | Grok-style options | `choose_permission_option` | `allow_once` wins | `src/agent/acp/permissions.rs` |
| TC-1328 | FR-1330 | unit | Mixed model list | `pick_non_fast_model` | Stays on composer family | `src/agent/acp/session.rs` |
| TC-1329 | FR-1327 | integration | Fake agent fast-only | ACP turn | Child reaped | `src/agent/cursor.rs` |
| TC-1331 | FR-1329 | integration | Fake agent permission outside | ACP turn | `Cancelled`, not `Completed` | `src/agent/cursor.rs` |
| TC-1332 | FR-1302 | unit | Duplicate `tool_call_update` ids | Map updates | Single activity row | `src/agent/acp/updates.rs` |

Deterministic ACP integration tests run under plain `cargo test`: the test helper builds the `fake-acp-agent` bin (`--features test-fixtures`) on first use.

### Live smoke (manual, Linux only)

1. Logged-in `cursor-agent`: `DMUXAI_CURSOR_BIN=cursor-agent protocol=acp` — send a prompt, see streaming reply, cancel once.
2. Logged-in `grok`: built-in grok with default ACP — model list and one prompt.
3. `protocol = "legacy"` for each — prior CLI behavior.

## 6. Integration and safety

| Area | Contract |
| --- | --- |
| Persistence | No schema change; legacy mode fields in JSON ignored. |
| Security | ACP permissions: allow in-workspace; deny outside; sensitive gates unchanged. Cursor ACP passes `--sandbox enabled` when supported (OQ-1326 for grok). Shell redirection outside the workspace is not parsed heuristically. |
| Platform | **Linux only** for FR-1300..FR-1309 and NFR-1301; do not extend Windows handling in this change. |

## 7. Implementation plan

See `docs/plans/PLAN-acp-runners.md` and `docs/tasks/TASKS-acp-runners.md`.

## 8. Supersedes (mode UX)

The following are superseded for **user-facing** Ask/Plan/Implement selection by FR-1306..FR-1307 and SPEC-remove-modes-chat-markdown FR-1308; internal read-only behavior remains via FR-1307 / FR-932:

- SPEC-agent-modes-copy (user mode picker copy)
- Mode-related rows in SPEC-permission-hardening and SPEC-cursor-windows-implement-guard **for user mode selection only** (Windows guard code may remain untouched)
