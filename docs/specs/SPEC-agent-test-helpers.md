# SPEC: agent-test-helpers

Status: Implemented · Dogfood: #23
Owner: Cloud Agent
Created: 2026-10-17
Baseline: `main`
Related: dogfood #23, [SPEC-spec-index](SPEC-spec-index.md)

## 1. Decision summary

AI agents authoring `src/app.rs` tests often invent a nonexistent `App::handle_enter`.
The real submission entry point is `submit_prompt` (Enter in the TUI), with
`test_app()` and related helpers for setup. This change documents those helpers
for humans and agents, lists them on the test module, and adds a test-only
`handle_enter` alias that forwards to `submit_prompt` so mistaken generated code
still compiles.

## 2. Scope

### In scope

- `AGENTS.md` § Agent test helpers with short examples.
- Module-level documentation on `#[cfg(test)] mod tests` in `src/app.rs`.
- `#[cfg(test)] fn handle_enter` on `App` delegating to `submit_prompt`.
- Traceability rows in `docs/TEST-PLAN.md` and closure of dogfood #23.

### Out of scope / non-goals

- Changing production Enter handling or renaming `submit_prompt`.
- Moving test helpers out of `src/app.rs` into a shared crate module.
- Fixing historical SPEC prose that mentions `handle_enter` (alias preserves compile).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | None; dogfood #23 fully specifies the friction. | Cloud Agent | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| `submit_prompt` | Clears `app.input` and routes the prompt to agents/slash/graph paths. | TUI `handle_key` Enter dispatch. |
| `test_app` | Builds `App` with in-memory DB and recording adapters. | `App::with_runtime` in production startup. |
| `handle_enter` (test) | `#[cfg(test)]` alias on `App` for generated tests only. | Production API (does not exist). |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-510 | Must | Given an agent reads `AGENTS.md` before editing app tests | Then a section titled **Agent test helpers** documents helpers and states there is no production `handle_enter` | If the section is missing, agents repeat the friction | TC-510 |
| FR-511 | Must | Given a test sets `app.input` and submits | Then documentation shows calling `submit_prompt()` | Must not document `handle_enter` as the primary API | TC-510, TC-511 |
| FR-512 | Must | Given a new app integration test needs default setup | Then documentation describes `test_app()` and `SentRequests` | Empty input still no-ops on submit | TC-510, TC-516 |
| FR-513 | Must | Given persistence or send-failure scenarios | Then documentation describes `test_app_with_engine*` helpers | Unsupported mode uses `test_app_with_unsupported_mode` | TC-510 |
| FR-514 | Must | Given cancel assertions are required | Then documentation describes `test_app_runtime` and `CancelRequests` | N/A | TC-510 |
| FR-515 | Must | Given a developer opens `src/app.rs` test module | Then module docs list the same helpers and point to `AGENTS.md` | Missing module doc leaves discoverability gap | TC-512 |
| FR-516 | Must | Given `#[cfg(test)]` code calls `app.handle_enter()` with non-empty `app.input` | Then behavior matches `submit_prompt()` for the same input | Empty input is a no-op for both | TC-511 |
| FR-517 | Must | Given dogfood #23 is addressed | Then `docs/dogfood/README.md` marks #23 fixed with fix summary | Still open → friction remains | TC-517 |
| FR-518 | Must | Given traceability is maintained | Then `docs/TEST-PLAN.md` §1 lists FR/TC 510–519 rows for this SPEC | Missing rows break matrix | TC-518 |
| FR-519 | Must | Given this SPEC is added | Then FR/TC IDs 510–519 are unique across `docs/specs/SPEC-*.md` | Duplicate IDs fail CI | TC-513, TC-514 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-510 | FR-510–FR-514 | unit | Repository checkout | Read `AGENTS.md` | Section lists `submit_prompt`, `test_app`, and extended helpers | `tests/agent_test_helpers.rs` |
| TC-511 | FR-511, FR-516 | unit | `test_app()`, `app.input` set | `app.handle_enter()` | Same adapter send count as `submit_prompt()` | `src/app.rs` |
| TC-512 | FR-515 | unit | Repository checkout | Read `src/app.rs` test module doc | Mentions `submit_prompt` and `test_app` | `tests/agent_test_helpers.rs` |
| TC-513 | FR-519 | unit | All `SPEC-*.md` files | `cargo test every_spec_has_status_line` | This SPEC has `Status:` in first 10 lines | `tests/spec_index.rs` |
| TC-514 | FR-519 | unit | All `SPEC-*.md` files | `cargo test spec_ids_are_unique_across_specs` | IDs 510–519 appear only here | `tests/spec_index.rs` |
| TC-515 | FR-519 | unit | SPEC files on disk | `cargo test spec_index_script_lists_every_spec` | Index includes `SPEC-agent-test-helpers` | `tests/spec_index.rs` |
| TC-516 | FR-512 | unit | `test_app()`, Codex selected | `submit_prompt` after setting input | Request routed to selected agent | `src/app.rs` `submitting_routes_to_the_selected_agent` |
| TC-517 | FR-517 | manual | PR diff | Review dogfood log | #23 marked fixed | `docs/dogfood/README.md` |
| TC-518 | FR-518 | manual | PR diff | Review TEST-PLAN §1 | Rows for SPEC-agent-test-helpers | `docs/TEST-PLAN.md` |
| TC-519 | FR-510 | manual | Run generator | `bash scripts/spec-index.sh` | Row for this SPEC with ID ranges including FR-510..519 | `scripts/spec-index.sh` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change | N/A | TC-516 |
| Provider protocol | No adapter change | N/A | — |
| Security / permissions | Test-only alias; not compiled in release binaries | Remove alias if it hides real API mistakes | TC-511 |
| Platform | Documentation only + `cfg(test)` code | Linux/macOS/Windows test builds | TC-511 |

## 7. Implementation plan

Link to `../plans/PLAN-agent-test-helpers.md` and `../tasks/TASKS-agent-test-helpers.md`.

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Agent test helper docs + alias | `AGENTS.md`, `src/app.rs` | FR-510–FR-516 | TC-510–TC-512, TC-511 |
| 2 | SPEC + traceability + dogfood | `docs/specs/SPEC-agent-test-helpers.md`, `docs/TEST-PLAN.md`, `docs/dogfood/README.md` | FR-517–FR-519 | TC-513–TC-519 |

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, all targets test, and clippy.
- [x] External-provider smoke tests not required.
- [x] `../reviews/REVIEW-agent-test-helpers.md` records commands, result, residual risk, and rollback notes.
