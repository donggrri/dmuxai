# TASKS: workspace-state-gitignore

Status: complete
Owner: DmuxAI
Created: 2026-11-07
Spec: [SPEC-workspace-state-gitignore](../specs/SPEC-workspace-state-gitignore.md)
Plan: [PLAN-workspace-state-gitignore](../plans/PLAN-workspace-state-gitignore.md)

Every task names its requirement IDs and the deterministic command that
verifies it. Run tests with `CARGO_TARGET_DIR` set to this worktree's `target`.

## Implementation

- [x] T1 — Add failing regression tests for the shared helper and representative
  creation paths. Requirements: FR-2501, FR-2502, FR-2503. Verify:
  `CARGO_TARGET_DIR=target cargo test tc_2501 -- --nocapture` (expected failure
  before T2).
- [x] T2 — Add `ensure_state_dir` / `ensure_state_path` in `src/workspace.rs`
  and route discovery, events, notify, attachments, clipboard, web uploads,
  graph store/tasks, and stage skills through it. Requirements: FR-2501,
  FR-2502, FR-2503. Verify:
  `CARGO_TARGET_DIR=target cargo test tc_2501 -- --nocapture` and the named
  module tests from TC-2502..TC-2507.
- [x] T3 — Document automatic `.dmuxai/.gitignore` behavior in `README.md`.
  Requirements: FR-2504. Verify:
  `CARGO_TARGET_DIR=target cargo test --test readme_install tc_2508`.
- [x] T4 — Record review evidence with exact commands and results.
  Requirements: all. Verify: review file exists and matches the run output.

## Review

- [x] Confirm root `.gitignore` is not read or written by the new code.
- [x] Confirm an existing `.dmuxai/.gitignore` is never overwritten or appended.
- [x] Confirm read-only paths still do not create `.dmuxai`.
- [x] Confirm no provider protocol, schema, permission, or process behavior
  changed.
