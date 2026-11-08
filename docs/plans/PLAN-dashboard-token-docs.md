# PLAN: dashboard-token-docs

Spec: [SPEC-dashboard-token-docs](../specs/SPEC-dashboard-token-docs.md)
Tasks: [TASKS-dashboard-token-docs](../tasks/TASKS-dashboard-token-docs.md)

## Order of work

1. Add `docs/specs/SPEC-dashboard-token-docs.md` with FR-2513..FR-2515 and TC-2513..TC-2515.
2. README.md: **Dashboard token** subsection after the existing dashboard bind note (FR-2513).
3. SECURITY.md: dashboard access token section (FR-2514).
4. `.github/ISSUE_TEMPLATE/bug_report.yml`: expand logs field description (FR-2515).
5. `tests/dashboard_token_docs.rs`: doc contract tests (TC-2513..TC-2515).
6. Plan, tasks, and review artifacts; run `python3 scripts/check-spec.py --all` and `python3 scripts/check-tc-links.py`.

## Risks

- Wording drift between README and SECURITY: keep a single list of required substrings in the test file.

## Rollback

Remove the new spec/plan/tasks/review files and revert README, SECURITY, the issue template, and the test file. No runtime behaviour changes.
