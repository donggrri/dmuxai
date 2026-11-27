# SPEC: runner-status

Status: Implemented

Owner: dmuxai
Created: 2026-11-27
Baseline: `77d4fbc`
Related: SPEC-cli-runners-models FR-2831, SPEC-runner-auth-error-kind, SPEC-graph-allowed-runners, SPEC-workspace-config-trust

## 1. Decision summary

Users cannot see whether the agent CLIs DmuxAI drives (cursor-agent, agy) are installed and signed in, and cannot switch a CLI off without editing `allowed_runners` by hand. This change adds an on-demand, side-effect-free status probe per runner (`dmuxai runners status`, `GET /api/runners/status`, an MCP tool, and a dashboard Runners panel) and a per-runner enable switch stored in the user config as `[runners.<id>] enabled`. Probes only run CLI commands that never start a model turn and never read credential files. Cursor and agy are implemented now; the probe table is keyed by runner id so opencode, codex and grok can be added later without changing the contract.

## 2. Scope

### In scope

- Probe cursor-agent and agy: installed, path, version, auth state, masked account label (cursor only), login hint, check time.
- `dmuxai runners status [--json] [--agent ID]`, `dmuxai runners enable|disable <id>`.
- User-config key `[runners.<id>] enabled` (default true) that removes a runner from headless run admission and graph routing.
- `GET /api/runners/status`, `POST /api/runners/enabled`, MCP tool `runners_status`.
- Dashboard Runners panel (More menu): status badge, Refresh, enable switch.

### Out of scope / non-goals

- Signing in or out, OAuth, reading or copying any credential, token or config file of a CLI.
- Quota or usage numbers (neither CLI has a cheap non-secret command; recorded in the research log).
- Probes for opencode, codex, grok and custom agents (they report `auth: "unknown"` with a hint; the probe table is the extension point).
- Hiding disabled runners from every dashboard picker (a disabled runner is shown as Disabled and refused at run time).
- Changing the plain `dmuxai runners` command (still spawns nothing).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3300 | `cursor-agent status --format json` exits 0 in both states and `isAuthenticated` is the auth bit; `status` does not reflect `CURSOR_API_KEY` (observed 2026-11-27, cursor-agent 2026.11.22). | dmuxai | Implementation |
| OQ-3301 | `agy models` exits non-zero and prints "Please sign in" when logged out (observed agy 1.5.18). | dmuxai | Implementation |
| OQ-3302 | Whether disabled runners should also disappear from dashboard agent pickers. | 동진 | Later |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Probe | A short read-only CLI invocation (`--version`, `status --format json`, `models`) with a 10 s timeout, stdin null, no prompt. | A model turn |
| Auth state | `ok`, `not_logged_in`, `unknown` (cannot tell, or no probe for this runner), `error` (probe failed or timed out). | Enabled |
| Enabled | The user's switch (`[runners.<id>] enabled`, default true). | Installed, authenticated, `allowed_runners` |
| Effective admission | `[graph] allowed_runners` admits the runner AND it is enabled. | Auth state |

Status row fields: `id`, `label`, `probe` (bool, false when no probe exists), `enabled`, `installed` (bool or null), `path` (string or null), `version` (string or null), `auth` (state), `auth_method` (`login`, `api_key_env`, or null), `account` (masked email or null), `hint` (string or null), `error` (short sanitized string or null), `checked_at` (RFC 3339 UTC), `duration_ms`, `cached` (bool).

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3300 | Must | Given cursor-agent resolved from `DMUXAI_CURSOR_BIN` or PATH, when it is probed | DmuxAI runs `--version` and `status --format json` only; `isAuthenticated: true` gives `auth: "ok"`, `auth_method: "login"` and `account` as a masked email (`d***@domain`); `isAuthenticated: false` gives `not_logged_in` with hint ``Run `cursor-agent login` ``. No other field of the JSON (name, ids, team) is stored or returned. | Unparseable output or non-zero exit gives `error`; timeout gives `error` with `timed out`. | TC-3300, TC-3303 |
| FR-3301 | Must | Given cursor-agent reports not authenticated and `CURSOR_API_KEY` or `CURSOR_AUTH_TOKEN` is set in the environment | DmuxAI additionally runs `cursor-agent models` once; success gives `auth: "ok"`, `auth_method: "api_key_env"`, `account: null`; failure gives `not_logged_in` with a hint naming both `cursor-agent login` and `CURSOR_API_KEY`. | The variable value is never logged, returned or placed in argv. Without either variable `models` is not run. | TC-3303 |
| FR-3302 | Must | Given agy resolved from `DMUXAI_AGY_BIN` or PATH, when it is probed | DmuxAI runs `agy --version` and `agy models` only; exit 0 gives `ok` (`auth_method: "login"`, `account: null`); non-zero exit whose output contains `sign in` (case-insensitive) gives `not_logged_in` with hint ``Run `agy` without arguments and sign in``; any other non-zero exit or timeout gives `error`. | `agy models` output is not returned; only the state. | TC-3302, TC-3303 |
| FR-3303 | Must | Given a runner whose executable cannot be resolved | `installed: false`, `path: null`, `auth: "unknown"`, hint with install guidance, and no process is spawned. | A runner without a probe (opencode, codex, grok, custom) reports `probe: false`, `auth: "unknown"`, `installed` from the existing resolver and a hint that no status probe exists yet. | TC-3303 |
| FR-3304 | Must | Given any probe result | `account` is masked as first character + `***@` + domain; every `error` and `hint` string has emails removed, is at most 200 characters and contains no raw CLI output beyond the classified message. | A value that is not an email is dropped. | TC-3301 |
| FR-3305 | Must | Given two status requests for the same runner binary within 60 s | The second returns the cached row with `cached: true` and spawns nothing; `refresh` bypasses the cache but not more than once per 3 s per runner (the cached row is returned inside that window). Runners are probed concurrently. | A disabled runner is still probed on request. | TC-3304 |
| FR-3306 | Must | Given `dmuxai runners status [--json] [--agent ID]` | Prints one row per registered runner (or only ID) in `runners` order. JSON is `{ "ok": true, "runners": [row...] }`; text prints a table with id, enabled, installed, version, auth, account and hint. Exit 0 for every auth state. | Unknown `--agent` exits 3 with `unknown agent '<id>'`; an unknown flag exits 2; `runners status --help` prints usage. Plain `dmuxai runners [--json]` is unchanged and additionally lists `enabled`. | TC-3305 |
| FR-3307 | Must | Given `[runners.<id>] enabled = false` in the user config | The runner is refused by headless `run --agent` (exit 5, message ``runner '<id>' is disabled; run `dmuxai runners enable <id>` ``) and by graph routing, even when `[graph] allowed_runners` lists it; `runners --json` and `status --json` report `enabled: false` and `allowed: false`. The key is user-config only: in a workspace `dmuxai.toml` it is ignored with a config warning. | Invalid value type produces a config warning and enabled stays true. `cursor` and `cursor-agent` resolve to the same runner. | TC-3306, TC-3312 |
| FR-3308 | Must | Given `dmuxai runners enable|disable <id>` | Writes `[runners.<id>] enabled = false` (disable) or removes the key (enable) in the user config with a comment-preserving edit and an atomic write that keeps the file mode; prints the new state (`--json`: `{ "ok": true, "runner": id, "enabled": bool }`). Idempotent. | Unknown runner id exits 3; an unparseable user config or a non-table `runners` value exits 1 with the file untouched. | TC-3307, TC-3305 |
| FR-3309 | Must | Given the dashboard HTTP API | `GET /api/runners/status[?agent=ID][&refresh=1]` returns the FR-3306 JSON and is read-only. `POST /api/runners/enabled` with `{ "runner": ID, "enabled": bool }` performs the FR-3308 write and returns the updated row. Both sit behind the same auth and host middleware as other `/api/*` routes. | Unknown runner gives 400; no body field can reach a path or command. | TC-3308 |
| FR-3310 | Must | Given an MCP client | Tool `runners_status` (optional `agent`) returns the FR-3306 JSON using the cache; it has no write counterpart. | Unknown agent returns exit code 3 mapped like `models`. | TC-3309 |
| FR-3311 | Must | Given the dashboard More menu | A Runners entry opens a panel listing every runner with a status badge (Ready, Not logged in, Not installed, Unknown, Error, Disabled), version, masked account, hint, a Refresh button and an enable switch that calls FR-3309. The panel loads status only when opened or refreshed. | While loading the badge shows Checking; a failed request shows an error line and keeps the previous rows. Text is inserted with `textContent` only. | TC-3310 |
| NFR-3310 | Must | Given dashboard startup, a run, a graph node or plain `dmuxai runners` | No runner status probe process is spawned. Probes happen only for the commands and endpoints above. | None. | TC-3311 |
| NFR-3311 | Must | Given any probe | argv is limited to the sets in FR-3300..FR-3302, stdin is null, a 10 s timeout kills the child, and no credential, token or config file of a CLI is opened by DmuxAI. | None. | TC-3303 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3300 | FR-3300 | unit | status JSON samples: authenticated with extra fields, unauthenticated, garbage | parse | ok+masked email only / not_logged_in / error; no other field retained | `src/runner_status.rs` `tc_3300_cursor_status_parse` |
| TC-3301 | FR-3304 | unit | emails, non-emails, 500 char strings | mask and sanitize | `a***@d.com`; none dropped; truncated to 200 | `src/runner_status.rs` `tc_3301_account_masking_and_sanitize` |
| TC-3302 | FR-3302 | unit | agy outputs: ok list, `Please sign in...`, network error | classify | ok / not_logged_in / error | `src/runner_status.rs` `tc_3302_agy_models_classification` |
| TC-3303 | FR-3300, FR-3301, FR-3302, FR-3303, NFR-3311 | integration | fake cursor/agy scripts in a temp dir that record argv, covering logged in, logged out, api-key env, missing binary, hang | probe | expected rows; argv only from the allowed sets; `models` only in the api-key case; hang killed within timeout | `src/runner_status.rs` `tc_3303_probe_with_fake_binaries` |
| TC-3304 | FR-3305 | unit | fake binary counting invocations | probe twice, then refresh twice quickly | second cached; refresh within 3 s cached; count matches | `src/runner_status.rs` `tc_3304_cache_ttl_and_refresh_limit` |
| TC-3305 | FR-3306, FR-3308 | unit | argv vectors | parse and execute against fakes | subcommand/flags parsed; unknown agent exit 3; bad flag exit 2; JSON shape | `src/cli.rs` `tc_3305_runners_status_cli` |
| TC-3306 | FR-3307 | unit | user and workspace config text | load settings | user false honoured; workspace ignored with warning; admission = allowed AND enabled; alias handled | `src/settings.rs` `tc_3306_runner_enabled_user_only` |
| TC-3307 | FR-3308 | unit | toml with comments, existing table, no table, inline `runners = {}`, invalid | edit | comments kept; key set/removed; invalid refused unchanged | `src/settings.rs` `tc_3307_set_runner_enabled_in_toml` |
| TC-3308 | FR-3309 | integration | router with temp XDG, fake binaries | GET status, POST enabled, bad runner, no token | rows; file updated; 400; auth failure like `/api/state` | `src/web/server.rs` `tc_3308_runners_status_http` |
| TC-3309 | FR-3310 | unit | MCP stub | tools/list and call | tool listed with `agent` only; result mapped | `src/mcp_server.rs` `tc_3309_mcp_runners_status` |
| TC-3310 | FR-3311 | asset | embedded JS and HTML | inspect | endpoints, badge labels, `textContent`, no fetch outside open/refresh handlers, no `innerHTML` with row data | `src/web/assets.rs` `tc_3310_runners_panel_asset` |
| TC-3311 | NFR-3310 | integration | fake binaries that touch a marker file | `runners_json`, settings load, `DmuxSettings::load` | marker absent | `src/agent_api.rs` `tc_3311_runners_listing_spawns_nothing` |
| TC-3312 | FR-3307 | unit | disabled runner, `run --agent` admission | check admission | exit 5 policy with the enable hint | `src/cli.rs` `tc_3312_disabled_runner_refused` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| User config | New optional `[runners.<id>] enabled` bool, user config only | Absent key = enabled; removing the key restores behaviour; older binaries ignore unknown keys in the runners table only if they do not reject it, so rollback is deleting the key | TC-3306, TC-3307 |
| HTTP | New `GET /api/runners/status`, `POST /api/runners/enabled` | Additive; remove routes to roll back | TC-3308 |
| MCP | New read-only tool `runners_status` | Additive | TC-3309 |
| Safety | Probes use fixed argv, null stdin, 10 s timeout, no credential file access, masked account, sanitized strings | n/a | TC-3303, TC-3301 |
| Performance | No probe on any hot path; 60 s cache; refresh rate limit | n/a | TC-3304, TC-3311 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Probe core, parsers, cache | `src/runner_status.rs`, `src/lib.rs` | FR-3300..FR-3305, NFR-3311 | TC-3300..TC-3304 |
| 2 | Enabled setting, TOML editor, admission | `src/settings.rs`, `src/agent_prompt.rs`, `src/agent_api.rs` | FR-3307, FR-3308 | TC-3306, TC-3307, TC-3311, TC-3312 |
| 3 | CLI | `src/cli.rs` | FR-3306, FR-3308 | TC-3305 |
| 4 | HTTP and MCP | `src/web/server.rs`, `src/mcp_server.rs` | FR-3309, FR-3310 | TC-3308, TC-3309 |
| 5 | Dashboard panel | `src/web/assets/runners.js`, `index.html`, `app.css`, `assets.rs` | FR-3311 | TC-3310 |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-3300..FR-3311, NFR-3310, NFR-3311) map to passing tests.
- [x] `python3 scripts/check-spec.py --all` passes.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, full gate and ignored web tests pass.
