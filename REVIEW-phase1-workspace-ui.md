# Review: Phase 1 workspace UI

Status: approved for commit
Spec: `SPEC-phase1-workspace-ui.md`

## Spec compliance

| Requirement | Result | Evidence |
| --- | --- | --- |
| UX-001 | Met | Desktop rendering test verifies every named shell panel. |
| UX-002 | Met | Compact rendering test verifies sidebars are absent while Conversation remains. |
| FR-001 | Met | Sidebar click test verifies a persisted conversation is created and selected. |
| FR-002 | Met | Sidebar click test verifies selection uses the existing stored conversation. |
| BR-001 | Met | No persistence migration, adapter protocol, or process-launching code changed; full suite passed. |
| NFR-001 | Met | All new UI checks use the deterministic Ratatui test backend. |

## Non-goal check

Context and Activity are explicitly visual placeholders. No live diff/filesystem
browser, log drawer, theme settings, GUI application, or provider redesign was
added.

## Regression and residual risk

- Conversation persistence has no migration and records are neither rewritten
  nor deleted by the sidebar.
- Provider cancellation, per-agent isolation, and platform process behavior
  were not changed.
- If listing conversations fails during rendering, the sidebar currently shows
  an empty recent-task list; existing task selection remains unchanged. A
  user-visible persistence error state is deferred to a later phase.

## Test evidence

- `git diff --check`: passed.
- `cargo fmt --check`: passed.
- `cargo test --all-targets -j 1`: passed; 91 tests passed and 5
  credential-dependent provider tests remained ignored.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `scripts/check-spec.ps1` was not run because the repository does not contain
  that script.
