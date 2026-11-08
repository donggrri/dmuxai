# TASKS: dashboard-token-docs

Spec: [SPEC-dashboard-token-docs](../specs/SPEC-dashboard-token-docs.md)
Plan: [PLAN-dashboard-token-docs](../plans/PLAN-dashboard-token-docs.md)

| Done | Task | Files | Requirements | Verify |
| --- | --- | --- | --- | --- |
| [x] | Doc spec and artifacts | `docs/specs/SPEC-dashboard-token-docs.md`, plan, tasks, review | FR-2513..FR-2515 | `python3 scripts/check-spec.py docs/specs/SPEC-dashboard-token-docs.md` |
| [x] | README dashboard token note | `README.md` | FR-2513 | `cargo test --test dashboard_token_docs tc_2513` |
| [x] | SECURITY dashboard token note | `SECURITY.md` | FR-2514 | `cargo test --test dashboard_token_docs tc_2514` |
| [x] | Bug report redaction reminder | `.github/ISSUE_TEMPLATE/bug_report.yml` | FR-2515 | `cargo test --test dashboard_token_docs tc_2515` |
| [x] | Doc tests | `tests/dashboard_token_docs.rs` | TC-2513..TC-2515 | `cargo test --test dashboard_token_docs` |
