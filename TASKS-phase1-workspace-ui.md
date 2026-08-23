# Tasks: Phase 1 workspace UI

Spec: `SPEC-phase1-workspace-ui.md`
Plan: `PLAN-phase1-workspace-ui.md`
Status: complete

- [x] T-UI-001: Add responsive workspace-shell rendering for UX-001 and
  UX-002. Test: `cargo test phase1_renders_the_workspace_shell_panels` and
  `cargo test phase1_collapses_context_panels_for_compact_terminals`.
- [x] T-UI-003: Connect `New task` sidebar hit testing to the existing
  conversation engine for FR-001. Test:
  `cargo test sidebar_new_task_creates_and_selects_a_conversation`.
- [x] T-UI-004: Connect recent-task sidebar hit testing to persisted selection
  for FR-002. Test: `cargo test sidebar_task_click_loads_the_selected_conversation`.
- [x] T-REG-001: Verify BR-001 and NFR-001. Tests: `cargo fmt --check`,
  `cargo test --all-targets`, and
  `cargo clippy --all-targets --all-features -- -D warnings`.
