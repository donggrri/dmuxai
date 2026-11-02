# REVIEW-jira-todos

Spec: [SPEC-jira-todos](../specs/SPEC-jira-todos.md)

## Spec compliance

| Requirement | Result | Evidence |
| --- | --- | --- |
| FR-2400 `[jira]` config, defaults, validation (https; http only for loopback; no userinfo) | met | TC-2400 |
| FR-2401 repo `[jira]` stripped for untrusted workspaces | met | TC-2401 |
| FR-2402 `/rest/api/3/search/jql` GET with Basic auth | met | TC-2402 |
| FR-2403 fallback to `/rest/api/3/search` on 404/410 only | met | TC-2403 |
| FR-2404 issue mapping and browse URL | met | TC-2404 |
| FR-2405 neutral error kinds, no token/body in errors or `Debug` | met | TC-2405 |
| FR-2406 `GET /api/jira/issues` behind dashboard auth | met | TC-2406 |
| FR-2407 60 s success cache, `?refresh=1` | met | TC-2407 |
| FR-2408 token never in responses or `/api/state` | met | TC-2408 |
| UX-2409 Jira group, hint, banner, refresh | met | TC-2409, TC-2410 |
| UX-2410 Open in Jira (noopener) and Copy to to-do (`external_key`) | met | TC-2410 |
| UX-2411 Start task prefills an unsent draft; disabled when view-only | met | TC-2411 |

## Non-goals checked

- No Jira writes: the client only builds GET requests.
- No token persistence: read from the env var at fetch time only.
- Settings screen and MCP import panel untouched; UI lives in `jira-todos.js` and the To-dos panel.

## Residual risks

- Ticket text in a started task goes to whichever agent the user picks; documented in README, never auto-sent.
- Only Jira Cloud basic auth (email + API token) is supported.
- All automated tests use a local fake Jira server; no live Jira test.
