# PLAN-jira-todos

Spec: [SPEC-jira-todos](../specs/SPEC-jira-todos.md)

1. `[jira]` settings and untrusted-workspace stripping (`src/settings.rs`) — FR-2400, FR-2401.
2. Read-only Jira search client with `/rest/api/3/search/jql` → `/rest/api/3/search` fallback and neutral errors (`src/jira.rs`) — FR-2402..FR-2405.
3. `GET /api/jira/issues` with a 60 s success cache and `?refresh=1` (`src/web/server.rs`, `src/main.rs`, `src/workspace_runtime.rs`) — FR-2406..FR-2408.
4. To-dos sidebar Jira group in a separate asset (`src/web/assets/jira-todos.js`, small hooks in `index.html`, `session.js`, `app.css`) — UX-2409..UX-2411.
5. README, API table, CHANGELOG.

## Risks

- Token leakage: token is only read from the env var at fetch time; `JiraClient` `Debug` redacts it; errors use fixed strings; tests assert absence (TC-2405, TC-2408).
- Repo config exfiltration: a cloned repo could point `base_url` at another host; repo `[jira]` is stripped until the workspace is trusted (TC-2401), and plain http is only allowed for loopback.
- Conflicts with the settings-screen PR: UI code lives in a new file and the To-dos panel only.

## Rollback

Remove the `[jira]` table or unset the token env var. No persistent data or schema changes.
