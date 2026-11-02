# REVIEW: Task sessions (PR-B UI wiring)

Status: complete<br>
Spec: `../specs/SPEC-task-sessions.md`<br>
Plan: `../plans/PLAN-task-sessions.md`<br>
Reviewed: 2026-11-02

## Summary

The conversation engine already persisted nullable `last_agent`,
`last_model_id`, and `last_reasoning_effort`. This change wires those settings
into the TUI and dashboard: picker changes write them, selecting a task restores
them after validation, and dashboard conversation entries expose them as
`settings`.

## Requirement review

| Requirement | Evidence | Result |
| --- | --- | --- |
| FR-2322 write | `persist_task_settings` is called after TUI agent/model/effort changes and dashboard `select_agent`, `select_model`, `set_effort`; a failed write only adds a notice. | Met |
| FR-2322 read | `restore_task_settings` runs from `restore_snapshot`; saved agents are checked against registered adapters and `allowed_runners`, models against `validate_model_for_runner` and the loaded catalog, and an unavailable agent/model falls back with one system notice. NULL settings keep the previous last-turn behavior. | Met |
| FR-2322 dashboard | `SessionConversation.settings` carries the saved settings or `null`, including other workspace groups. | Met |
| TC-2322 | `src/app.rs` `tc_2322_select_restores_task_picker` covers restore and the unknown-agent fallback; `tests/conversation.rs` round-trip tests pass. | Met |

## Scope and regression review

- No transcript, turn, binding, or provider protocol changes.
- Restoration does not write settings; only user picker/command changes write.
- Model and effort restoration reuse the existing picker validation and adapter
  selection paths, including the Cursor model pin.
- The dashboard snapshot keeps its existing shape; `settings` is additive and
  legacy payloads deserialize with `None`.

## Test evidence

- `cargo test -j 2 --bin dmuxai tc_2322` - passed (1 test).
- `cargo test -j 2 --test conversation tc_232` - passed (3 tests).
- `cargo test -j 2 --lib session_conversation_fields_roundtrip` - passed.
- `cargo fmt --check` - passed.
- `cargo clippy -j 2 --all-targets -- -D warnings` - passed.
- `cargo test -j 2 --lib tc_2322` matched no tests because the app test lives
  in the binary target; the binary command above is the effective check.

## Residual risks

- A saved model whose catalog has not loaded yet is accepted when the model
  policy allows it; availability is rechecked by the normal turn path.
- Effort is Codex-only and is ignored for other runners.

## Rollback

Revert the app wiring, snapshot field, test, task/review docs, and CHANGELOG
entry. The nullable conversation columns can remain unread.
