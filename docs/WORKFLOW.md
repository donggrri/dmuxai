# Spec-first delivery workflow

## Purpose

This workflow makes a feature request traceable from intended behavior to
automated evidence. It is intentionally lightweight for a small Rust project,
but it does not allow implementation to become the only specification.

The authoritative sources are, in order:

1. An approved `docs/specs/SPEC-<slug>.md` for a new or changed feature.
2. `docs/specs/DMUXAI-BASELINE.md` for already shipped behavior not changed by
   that feature.
3. Existing tests only as evidence of implementation, never as permission to
   silently change intended behavior.

## Required flow

| Phase | Required output | Exit gate |
| --- | --- | --- |
| Spec | `docs/specs/SPEC-<slug>.md` | Scope, non-goals, measurable requirements, and test scenarios are complete. |
| Plan | `docs/plans/PLAN-<slug>.md` and `docs/tasks/TASKS-<slug>.md` | File-level plan and task-level verification are known. |
| Execute | Code and tests | Every task is implemented without expanding scope. |
| Review | `docs/reviews/REVIEW-<slug>.md` | Diff conforms to each requirement and records residual risks. |
| Test | Command output in review | Narrow tests and applicable repository gates pass. |

`docs/tasks/TASKS-<slug>.md` is deliberately separate from a plan: it is the execution
checklist, and every item must be independently verifiable.

## Writing a testable spec

Start with `docs/templates/FEATURE-SPEC.md`. Requirements must describe
observable contracts, not implementation wishes.

Good:

> **FR-012** — Given a selected agent has an active turn, when the user submits
> another prompt to that agent, the prompt remains in the input field and the
> UI displays that the agent is busy. No second provider process starts.

Weak:

> Handle duplicate prompts gracefully.

The good version identifies the state, action, UI outcome, persistence/process
outcome, and a direct test oracle. Prefer this shape:

| Field | What to state |
| --- | --- |
| ID | Stable requirement ID; do not recycle it after removal. |
| Priority | Must, Should, or Could. |
| Preconditions | State that must already be true. |
| Trigger | User action, provider event, or system event. |
| Expected result | Visible result and durable state change. |
| Failure behavior | Error message, rollback, retry, or no-op. |
| Evidence | Automated test ID or justified manual check. |

Use a scenario table with unique `TC-*` IDs. Each scenario has exactly one
primary expected result, making failures easy to diagnose. State setups must be
reproducible without a real provider whenever possible.

## Test selection rules

| Changed area | Required deterministic evidence | Optional/manual evidence |
| --- | --- | --- |
| Conversation engine or SQLite schema | integration tests using a temporary database; normal, stale, recovery, and rollback cases | database migration inspection |
| Agent adapter/protocol | JSON/NDJSON parser and command-construction tests; error and cancellation normalization | ignored authenticated-provider smoke test |
| TUI state/input | unit/state tests for keys, overlays, routing, and rendered output | interactive terminal smoke test |
| Chat selection/clipboard | Unicode grapheme and terminal-cell tests | OS clipboard smoke test |
| Process resolution | platform-independent resolution tests; Windows-specific command setup where applicable | installed CLI launch check |
| Installer/release | static script review and checksum-path test where feasible | Windows install/update/uninstall path |

Run `cargo fmt --check`, `cargo test --all-targets`, and
`cargo clippy --all-targets --all-features -- -D warnings` for code changes
unless the review documents why a gate cannot run. A blocked platform check is
not a pass; record the cause and the exact unrun command.

Run the documentation traceability gate whenever a spec changes:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/DMUXAI-BASELINE.md
```

## Review checklist

- Every changed or added behavior has a requirement ID and matching evidence.
- Every scenario marked Must has an automated test or a documented reason it
  cannot be automated.
- Tests assert outcomes and invariants rather than private helper calls.
- Error, cancellation, restart/recovery, and cross-agent isolation are checked
  when relevant.
- Provider prompt text never travels through a shell command string.
- Implement mode is the only mode that may write within the workspace; no change
  weakens the sandbox or approval policy.
- The review lists exact commands, date-independent results, and any skipped
  live smoke tests.

## Artifact status conventions

Use `draft`, `approved`, `in_progress`, `blocked`, or `complete` at the top of
each artifact. A review can be `complete` only after its listed test evidence
has passed. Keep completed artifacts as historical evidence; do not overwrite
them to describe a later feature.
