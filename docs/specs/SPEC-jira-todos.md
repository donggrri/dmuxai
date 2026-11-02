# SPEC-jira-todos

Status: Proposed

Owner: dmuxai
Created: 2026-11-02
Baseline: `8ff5c1f`
Related: PLAN-jira-todos, TASKS-jira-todos, REVIEW-jira-todos

## 1. Decision summary

The web dashboard's To-dos sidebar gets a collapsible, read-only "Jira" group that lists Jira Cloud issues matched by a configurable JQL query. DmuxAI reads issues through the Jira Cloud REST API (`/rest/api/3/search/jql`, falling back to `/rest/api/3/search` on 404 or 410) with basic auth built from `[jira] email` and an API token that is read only from an environment variable. DmuxAI never writes to Jira. Each ticket can be opened in Jira, started as a new conversation with a prefilled (unsent) draft, or copied into a local to-do.

## 2. Scope

### In scope

- `[jira]` config table: `base_url`, `email`, `jql`, `max_results`, `token_env`.
- A Jira read client (search only) with endpoint fallback and neutral error mapping.
- `GET /api/jira/issues` behind the dashboard token, 60-second cache, `?refresh=1` bypass.
- To-dos sidebar "Jira" group: key badge, summary, status chip, refresh button, inline error banner, setup hint, three per-ticket actions.
- Docs note that ticket content sent to a model leaves the machine.

### Out of scope / non-goals

- Writing to Jira (transitions, comments, assignment, creation).
- Jira Server / Data Center auth (PAT bearer), OAuth, or storing the token anywhere.
- Auto-sending a prompt built from a ticket.
- Changing the existing MCP import panel or the `[todo] jira_base_url` link behaviour.
- The settings screen redesign (separate PR); this feature only adds files and markup inside the To-dos panel.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2400 | Jira Cloud keeps `/rest/api/3/search/jql`; the legacy `/rest/api/3/search` fallback is kept for sites that still serve it. | dmuxai | implementation |
| OQ-2401 | A repo `dmuxai.toml` `[jira]` table is honoured only for trusted workspaces, like other restricted keys. | dmuxai | implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Jira group | The collapsible "Jira" section in the To-dos sidebar. | The MCP "Import from Jira / Confluence" panel. |
| Token env | The environment variable named by `[jira] token_env` (default `DMUXAI_JIRA_TOKEN`). | Dashboard auth token. |
| Configured | `base_url` and `email` are both set and valid. | Token present. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2400 | Must | Given `[jira]` in user `dmuxai.toml` | settings expose `base_url` (absolute https URL, trailing slash trimmed; plain http allowed only for loopback hosts), `email`, `jql` (default `assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC`), `max_results` (default 50, clamped 1..=100), `token_env` (default `DMUXAI_JIRA_TOKEN`, must match `[A-Z_][A-Z0-9_]*`) | invalid values are ignored with a config warning that never contains a token; missing table = not configured | TC-2400 |
| FR-2401 | Must | Given a repo `dmuxai.toml` with `[jira]` in an untrusted workspace | the repo `[jira]` table is stripped and listed as a restricted key | trusted workspace keeps it; user config always applies | TC-2401 |
| FR-2402 | Must | Given configured Jira and the token env set | the client calls `GET {base_url}/rest/api/3/search/jql` with `jql`, `maxResults`, and `fields=summary,status,priority,issuetype,updated` and an `Authorization: Basic base64(email:token)` header | no request is made when not configured or the token env is unset/empty | TC-2402 |
| FR-2403 | Must | Given `/rest/api/3/search/jql` answers 404 or 410 | the client retries once with `GET {base_url}/rest/api/3/search` using the same query parameters | any other status is not retried | TC-2403 |
| FR-2404 | Must | Given a successful search | each issue maps to `key`, `summary`, `status`, `status_category` (Jira status category key), `priority`, `issuetype`, `updated`, and `url` = `{base_url}/browse/{key}` | missing optional fields map to empty strings; malformed JSON maps to error kind `bad_response` | TC-2404 |
| FR-2405 | Must | Given any failure | errors map to kinds `not_configured`, `missing_token`, `unauthorized` (401/403), `network` (connect/timeout), `http` (other status), `bad_response`, each with a fixed neutral message | messages, logs, and responses never contain the token, the Authorization header, or the response body | TC-2405 |
| FR-2406 | Must | Given the dashboard token | `GET /api/jira/issues` returns `{configured, issues, error, fetched_at}`; without the dashboard token it returns 401 like other `/api/*` routes | not configured returns 200 with `configured:false`, empty `issues`, and no outbound request | TC-2406 |
| FR-2407 | Must | Given a successful fetch less than 60 s old | `GET /api/jira/issues` serves the cached result without a new outbound request; `?refresh=1` forces a new fetch | errors are not cached | TC-2407 |
| FR-2408 | Must | Given any `/api/jira/issues` response or `/api/state` snapshot | the token value and `token_env` value are never present in the body | — | TC-2408 |
| UX-2409 | Must | Given the To-dos tab and Jira configured | a collapsible "Jira" group shows each ticket as a key badge, summary, and status chip, with a refresh button | not configured: group shows a one-line setup hint; errors show a small inline banner with the neutral message | TC-2409 |
| UX-2410 | Must | Given a ticket row | "Open in Jira" opens `url` in a new tab with `rel="noopener noreferrer"`; "Copy to to-do" creates a local to-do titled `KEY summary` with the Jira key set | the Jira group never issues writes to Jira | TC-2410 |
| UX-2411 | Must | Given a ticket row in an attached dashboard | "Start task" starts a new conversation and fills the composer draft with `KEY summary` without sending it | view-only `serve` disables the button | TC-2411 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2400 | FR-2400 | unit | temp user config with `[jira]` variants | load settings | defaults, trimming, clamping, invalid values warned | `src/settings.rs` `tc_2400_jira_config_defaults_and_validation` |
| TC-2401 | FR-2401 | unit | untrusted and trusted temp workspaces with repo `[jira]` | load settings | stripped when untrusted, kept when trusted | `src/settings.rs` `tc_2401_untrusted_repo_jira_table_stripped` |
| TC-2402 | FR-2402 | unit | fake local HTTP Jira server | search | path, query params, Basic header; no request without token | `src/jira.rs` `tc_2402_search_jql_request_shape` |
| TC-2403 | FR-2403 | unit | fake server answering 404 / 410 / 500 on `/search/jql` | search | falls back to `/search` only on 404/410 | `src/jira.rs` `tc_2403_fallback_to_legacy_search` |
| TC-2404 | FR-2404 | unit | fake server with full, sparse, and malformed bodies | search | mapped fields, browse URL, `bad_response` | `src/jira.rs` `tc_2404_issue_mapping` |
| TC-2405 | FR-2405 | unit | fake server 401/403/500, closed port, missing token | search | error kinds; messages exclude token and body | `src/jira.rs` `tc_2405_error_mapping_is_neutral` |
| TC-2406 | FR-2406 | unit | router with fake Jira service | GET with and without dashboard token; unconfigured | 200 / 401 / `configured:false` | `src/web/server.rs` `tc_2406_jira_issues_endpoint` |
| TC-2407 | FR-2407 | unit | router with counting fake server | GET twice, then `?refresh=1` | one request, then two | `src/web/server.rs` `tc_2407_jira_issues_cache_and_refresh` |
| TC-2408 | FR-2408 | unit | router with a distinctive fake token | GET `/api/jira/issues` (success and 401) and `/api/state` | token string absent | `src/web/server.rs` `tc_2408_jira_token_never_in_responses` |
| TC-2409 | UX-2409 | unit | embedded assets | inspect `index.html` / JS | Jira group markup, refresh button, banner, setup hint present | `src/web/server.rs` `tc_2409_jira_group_assets` |
| TC-2410 | UX-2410 | e2e | dashboard + fake Jira server, headless Chrome | click Open / Copy | new-tab link attrs; to-do created with key | `tests/web_jira_todos.rs` `tc_2410_jira_group_renders_and_copies` (ignored) |
| TC-2411 | UX-2411 | unit | embedded assets | inspect JS | Start task sends `new_conversation` and fills the draft without submitting | `src/web/server.rs` `tc_2411_start_task_prefills_draft` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change; cache is in memory only | Remove `[jira]` to disable | TC-2407 |
| Provider protocol | No agent protocol change | N/A | TC-2402 |
| Security / permissions | Token read from env only; never stored, logged, returned, or put in errors; repo `[jira]` restricted when untrusted; https required except loopback | Unset token env to disable | TC-2401, TC-2405, TC-2408 |
| Privacy | Ticket text sent to an agent leaves the machine; docs tell users to pick the model accordingly; DmuxAI never auto-sends ticket text | N/A | TC-2411 |
| Platform | Pure Rust HTTP (reqwest, rustls) | N/A | TC-2402 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | `[jira]` settings + trust stripping | `src/settings.rs` | FR-2400, FR-2401 | TC-2400, TC-2401 |
| 2 | Jira read client | `src/jira.rs`, `src/lib.rs` | FR-2402..FR-2405 | TC-2402..TC-2405 |
| 3 | Dashboard endpoint + cache | `src/web/server.rs`, `src/main.rs`, `src/workspace_runtime.rs` | FR-2406..FR-2408 | TC-2406..TC-2408 |
| 4 | To-dos sidebar Jira group | `src/web/assets/index.html`, `src/web/assets/jira-todos.js`, `src/web/assets/app.css` | UX-2409..UX-2411 | TC-2409..TC-2411 |
| 5 | Docs + CHANGELOG | `docs/`, `CHANGELOG.md` | FR-2400 | review |
