# SPEC: dashboard token documentation

Status: implemented (docs only; #193)
Owner: DmuxAI
Created: 2026-11-07
Related: GitHub #193, SPEC-web-dashboard (FR-521, FR-522)

## 1. Decision summary

`dmuxai --web` and `dmuxai serve` print the dashboard URL including a per-start
access token as a query parameter. That line is intentional (the browser needs
the token), but users may not realize it is a credential or that it can linger
in scrollback, logs, screenshots, and browser history. Document the risk in
README and SECURITY.md and remind bug reporters to redact tokens before
pasting logs.

## 2. Scope

### In scope

- A **Dashboard token** subsection in README.md (per-start token, treat as secret,
  scrollback/logs/screenshots, browser history, `DMUXAI_WEB_TOKEN` cautions).
- A dashboard-token section in SECURITY.md with the same cautions.
- A bug-report template reminder to redact dashboard tokens.
- Doc tests that assert the required phrases are present.

### Out of scope / non-goals

- Changing how the URL is printed or stripping the token from the address bar in
  the browser (`history.replaceState`); tracked separately if desired.
- New CLI warnings at startup.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2513 | Must | Given README.md | A **Dashboard token** heading documents that `dmuxai --web` and `dmuxai serve` print a URL with a per-start access token; users must treat it as a secret; they should redact `?token=` before sharing logs or screenshots; opening the URL can store the token in browser history; and `DMUXAI_WEB_TOKEN` pins a fixed token with usage cautions | — | TC-2513 |
| FR-2514 | Must | Given SECURITY.md | A section documents the same dashboard-token risks (per-start credential, scrollback and logs, redact before sharing, browser history, `DMUXAI_WEB_TOKEN` must not be committed or pasted into public issues) | — | TC-2514 |
| FR-2515 | Must | Given `.github/ISSUE_TEMPLATE/bug_report.yml` | The logs/screenshots field description explicitly reminds reporters to redact dashboard access tokens (e.g. `?token=` URLs) in addition to API keys | — | TC-2515 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2513 | FR-2513 | unit (doc) | README.md | Read the Dashboard token section | Required phrases present | `tests/dashboard_token_docs.rs` `tc_2513_readme_documents_dashboard_token` |
| TC-2514 | FR-2514 | unit (doc) | SECURITY.md | Read the dashboard token section | Required phrases present | `tests/dashboard_token_docs.rs` `tc_2514_security_documents_dashboard_token` |
| TC-2515 | FR-2515 | unit (doc) | bug_report.yml | Read the logs field description | Mentions redacting dashboard tokens | `tests/dashboard_token_docs.rs` `tc_2515_bug_report_reminds_redact_dashboard_tokens` |
