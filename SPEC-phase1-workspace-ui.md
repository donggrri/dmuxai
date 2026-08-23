# Feature Spec: Phase 1 workspace UI

Status: implemented and verified
Branch: `donggrri/phase1-ui`

## Goal

Provide a Codex-inspired terminal workspace shell while preserving the existing
conversation, provider, and process-launching behavior.

## Baseline and boundaries

This feature changes only the Ratatui presentation layer and sidebar
interaction. Conversation persistence, provider adapter protocol, and process
launching remain unchanged.

The repository does not currently contain `docs/WORKFLOW.md`,
`docs/specs/DMUXAI-BASELINE.md`, or `docs/templates/FEATURE-SPEC.md`; this
spec follows the repository delivery rules supplied for this change.

## Requirements

### UX-001: Desktop workspace shell

Actor: DmuxAI user.
Precondition: terminal width is at least 110 columns.
Trigger: the application renders.
Expected result: Workspace, Conversation, Context, Activity, Prompt, and
status regions are visible with a consistent dark visual hierarchy.
Persistence/error behavior: rendering performs no provider call and does not
mutate a conversation.

### UX-002: Compact layout

Actor: DmuxAI user.
Precondition: terminal width is below 76 columns.
Trigger: the application renders.
Expected result: Workspace and Context are hidden; Conversation and Prompt
remain usable.
Persistence/error behavior: no conversation state changes.

### FR-001: Create from the Workspace sidebar

Actor: DmuxAI user.
Precondition: Workspace sidebar is visible.
Trigger: user left-clicks `New task`.
Expected result: a new persisted conversation is created and selected.
Persistence/error behavior: the existing conversation persistence engine owns
the new record; failures retain the currently selected conversation.

### FR-002: Select a recent task from the Workspace sidebar

Actor: DmuxAI user.
Precondition: Workspace sidebar is visible and contains a rendered recent
conversation.
Trigger: user left-clicks that conversation row.
Expected result: that persisted conversation becomes active.
Persistence/error behavior: no record is overwritten; selection uses its
stored conversation ID.

### BR-001: Preserve existing controls and provider boundaries

Actor: DmuxAI user.
Precondition: application starts normally.
Trigger: user uses existing keyboard controls, conversation persistence, or
provider-backed execution.
Expected result: existing behavior remains available; Phase 1 introduces no
provider protocol, persistence schema, or process-launching change.
Persistence/error behavior: existing paths retain their current error and
cancellation behavior.

### NFR-001: Deterministic UI verification

The shell and sidebar interactions must be covered by deterministic Ratatui
rendering/state tests that require neither credentials nor a live provider.

## Acceptance scenarios

- Given a 120x30 terminal, when the app renders, then all named desktop shell
  panels from UX-001 are present.
- Given a 72x24 terminal, when the app renders, then Conversation is present
  and Context and Workspace are absent.
- Given a visible Workspace sidebar, when the user clicks `New task`, then the
  conversation count increases by one and the active conversation changes.
- Given two visible conversations, when the user clicks the first row, then
  the first conversation is selected without changing its title or ID.

## Non-goals

- Context is not yet a live filesystem/diff browser; its Phase 1 content is a
  visual placeholder.
- Activity is not yet a real log drawer or toggleable activity history.
- Theme configuration, a web/native GUI application, and a provider adapter
  redesign are out of scope.

## Test traceability

| Test ID | Requirement | Automated evidence |
| --- | --- | --- |
| T-UI-001 | UX-001, NFR-001 | `phase1_renders_the_workspace_shell_panels` |
| T-UI-002 | UX-002, NFR-001 | `phase1_collapses_context_panels_for_compact_terminals` |
| T-UI-003 | FR-001, NFR-001 | `sidebar_new_task_creates_and_selects_a_conversation` |
| T-UI-004 | FR-002, NFR-001 | `sidebar_task_click_loads_the_selected_conversation` |
| T-REG-001 | BR-001 | `cargo test --all-targets` |
