# TASKS: Agent test helpers

Spec: [SPEC-agent-test-helpers.md](../specs/SPEC-agent-test-helpers.md)

- [x] T1 — `AGENTS.md` § Agent test helpers (FR-510–FR-514) → `cargo test agents_md_documents_agent_test_helpers`
- [x] T2 — `src/app.rs` module doc + `handle_enter` alias (FR-515–FR-516) → `cargo test handle_enter_alias_submits_prompt`
- [x] T3 — `tests/agent_test_helpers.rs` (TC-510, TC-512) → same commands as T1–T2
- [x] T4 — SPEC + TEST-PLAN + dogfood #23 (FR-517–FR-519) → `cargo test spec_ids_are_unique_across_specs`
- [x] T5 — Review doc + `cargo test --all-targets` + `cargo clippy --all-targets -- -D warnings`
