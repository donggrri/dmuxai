# SPEC: ACP runners and single agent mode

Status: draft
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

- AGY on ACP in this release
- Changing MCP propagation (each agent keeps native MCP config)
- Removing or refactoring existing Windows-specific code paths elsewhere in the repo
- Windows/WSL caveats or new Windows behavior for ACP or single agent mode
- Release workflow changes

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1300 | Cursor and grok ACP servers speak ACP v1 with blocking `session/prompt` completion (stop reason on response or terminal `session/update`). | DmuxAI | Live smoke on Linux |
| OQ-1301 | `grok agent stdio` accepts `-m`, optional `--reasoning-effort`, and `--always-approve` for agent-mode turns. | DmuxAI | Live smoke on Linux |

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
| FR-1300 | Must | `[cursor] protocol` unset or `acp`, Cursor binary resolvable. | Turns spawn `cursor-agent acp` (or `DMUXAI_CURSOR_BIN` + `acp`). Prompt via `session/prompt`. Session id persisted for resume. | `protocol = "legacy"` uses existing stream-json path. Child reaped on end/cancel/error. | TC-1300, TC-1305 |
| FR-1301 | Must | Built-in grok registered, `grok_protocol` unset or `acp`. | Turns spawn `grok agent stdio` with `-m <model>` and implement-equivalent approval flags; no streaming-json argv. | `protocol = "legacy"` uses SPEC-grok-runner argv. | TC-1301, TC-1305 |
| FR-1302 | Must | ACP turn in progress. | `session/update` maps to `TextDelta`, `Activity`, plan/tool events consistent with existing UI. Debug logs name ACP methods/stop reasons, never prompt bodies or secrets. | Malformed lines logged and skipped where safe. | TC-1302 |
| FR-1303 | Must | User cancels or child crashes. | `session/cancel` sent when possible; no orphan agent process after turn end. | Crash → `Error` event and non-zero handling. | TC-1303, TC-1304 |
| FR-1304 | Must | Agent sends `session/request_permission`. | Allow workspace read/write and normal tools; deny paths outside workspace; defer to existing sensitive-step gates when applicable; otherwise allow selected option. | Cancelled permission → turn ends cleanly. | TC-1306, TC-1307 |
| FR-1305 | Must | `protocol = "legacy"` for Cursor or grok. | Behavior matches pre-ACP release for that runner (no ACP spawn). | Default remains `acp` when key omitted. | TC-1308 |
| FR-1306 | Must | User sends prompt (any runner). | No `/mode` command; no mode picker; adapters run agent-mode CLI (or ACP) without Ask/Plan. | — | TC-1309 |
| FR-1307 | Must | Internal judge/plan/replan turn (FR-932). | `TurnPolicy::ReadOnly` on the request; adapter uses read-only CLI/ACP behavior; user agent policy unchanged after turn. | No read-only support → existing fallback paths. | TC-1310 |
| FR-1308 | Must | Config/API sends `mode` (`ask`/`plan`/`implement`), `[agents.custom.modes]`, or dashboard `set_mode`. | One startup or first-use warning per process; value ignored; no crash. | — | TC-1311 |
| FR-1309 | Must | Docs updated. | README and CHANGELOG describe ACP default, legacy switch, single agent mode, Linux-only support for this feature set. Older mode specs carry a superseded note. | — | TC-1312 |
| NFR-1300 | Must | ACP or legacy turn. | MCP merge behavior unchanged from baseline. | — | TC-1300 |
| NFR-1301 | Must | This spec’s ACP and agent-mode contracts. | Documented supported platform is Linux; no new Windows/WSL documentation for these features. | — | Review |

## 5. Acceptance scenarios

| Test ID | Requirement IDs | Type | Setup | Action | Oracle | Location |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1300 | FR-1300 | integration | Fake ACP binary as `DMUXAI_CURSOR_BIN` parent | Send prompt | Streaming text, `Completed`, session bound | `src/agent/acp.rs` |
| TC-1301 | FR-1301 | unit | grok preset + `acp` protocol | Build argv | Contains `agent`, `stdio`, `-m` | `src/custom_agent.rs` |
| TC-1302 | FR-1302 | unit | Fixture NDJSON | Parse updates | Expected `AgentEvent` sequence | `src/acp/` |
| TC-1303 | FR-1303 | integration | Fake agent | Cancel mid-turn | `Cancelled`, child gone | `src/agent/acp.rs` |
| TC-1304 | FR-1304 | integration | Fake agent exits 1 | Prompt | `Error` surfaced | `src/agent/acp.rs` |
| TC-1305 | FR-1300, FR-1301 | unit | Settings parse | Load toml | Defaults `acp` | `src/settings.rs` |
| TC-1306 | FR-1304 | unit | Permission payload outside workspace | Policy | Deny | `src/acp/permissions.rs` |
| TC-1307 | FR-1304 | unit | Permission with sensitive gate active | Policy | Gate path | `src/acp/permissions.rs` |
| TC-1308 | FR-1305 | integration | `protocol=legacy` + fake legacy cursor | Prompt | No `acp` in argv | `src/agent/cursor.rs` |
| TC-1309 | FR-1306 | unit | App snapshot / slash list | Inspect | No `/mode` | `src/app.rs` |
| TC-1310 | FR-1307 | integration | Judge turn | Start judge | Read-only argv/policy | `src/app.rs` |
| TC-1311 | FR-1308 | unit | TOML with `[modes]` | Load settings | Warning + implement args | `src/settings.rs` |
| TC-1312 | FR-1309 | manual | Read docs | — | Linux-only stated | `docs/specs/SPEC-acp-runners.md` |

### Live smoke (manual, Linux only)

1. Logged-in `cursor-agent`: `DMUXAI_CURSOR_BIN=cursor-agent protocol=acp` — send a prompt, see streaming reply, cancel once.
2. Logged-in `grok`: built-in grok with default ACP — model list and one prompt.
3. `protocol = "legacy"` for each — prior CLI behavior.

## 6. Integration and safety

| Area | Contract |
| --- | --- |
| Persistence | No schema change; legacy mode fields in JSON ignored. |
| Security | ACP permissions: allow in-workspace; deny outside; sensitive gates unchanged. |
| Platform | **Linux only** for FR-1300..FR-1309 and NFR-1301; do not extend Windows handling in this change. |

## 7. Implementation plan

See `docs/plans/PLAN-acp-runners.md` and `docs/tasks/TASKS-acp-runners.md`.

## 8. Supersedes (mode UX)

The following are superseded for **user-facing** Ask/Plan/Implement selection by FR-1306..FR-1308; internal read-only behavior remains via FR-1307 / FR-932:

- SPEC-agent-modes-copy (user mode picker copy)
- Mode-related rows in SPEC-permission-hardening and SPEC-cursor-windows-implement-guard **for user mode selection only** (Windows guard code may remain untouched)
