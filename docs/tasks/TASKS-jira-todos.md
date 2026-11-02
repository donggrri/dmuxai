# TASKS-jira-todos

| Task | Requirement IDs | Test command | Status |
| --- | --- | --- | --- |
| T1 `[jira]` settings + trust stripping | FR-2400, FR-2401 | `cargo test --all-features --lib tc_240` | done |
| T2 Jira read client | FR-2402..FR-2405 | `cargo test --all-features --lib jira` | done |
| T3 `/api/jira/issues` + cache | FR-2406..FR-2408 | `cargo test --all-features --lib tc_240` | done |
| T4 To-dos Jira group UI | UX-2409..UX-2411 | `cargo test --all-features --lib tc_241`; `cargo test --all-features --test web_jira_todos -- --ignored` | done |
| T5 Docs + CHANGELOG | FR-2400 | review | done |
