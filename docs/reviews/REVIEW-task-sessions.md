# REVIEW: Task sessions (PR-B UI wiring and PR-C resume/delta)

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

## PR-C: Per-agent resume and delta handoff

Status: complete<br>
Reviewed: 2026-11-02

### Summary

Each agent keeps its own active provider binding across other agents' completed
turns. A later request resumes that binding with only the unseen completed turns
from other agents, inside the existing untrusted `[DmuxAI shared context]`
envelope. Oversized deltas and stale, retired or missing bindings keep the
existing fresh-session projection. Provider-rejected resume ids retire the
binding and retry the same turn once with the projection.

### Requirement review

| Requirement | Evidence | Result |
| --- | --- | --- |
| FR-2323 resume own session | `request_context_for` returns the active binding id when the unseen delta fits; `tc_2323_agent_resumes_own_session_after_other_agent_turn`. | Met |
| FR-2324 bounded delta | `unseen_delta_for` filters completed turns from other agents after `known_through_seq`; `ProjectedTurn.model_id` and `render_resume_prompt` label agent and model; queued/failed/cancelled turns are excluded by the completed-only query. | Met |
| FR-2325 fallback and rejection | 200-turn / 65,536-byte bounds, stale/missing binding fallback, and the app's `try_fresh_resume_retry` with `retire_binding_for_resume_rejection`; `tc_2325_delta_bounds_and_resume_fallbacks`, `tc_2325_resume_rejection_retries_fresh`. | Met |
| FR-2326 consume on completed | `apply_turn_event` advances `known_through_seq` only on `Completed`; `ProviderSessionBound` no longer advances it. `tc_2326_delta_is_consumed_only_on_completed` covers completed, failed, cancelled and interrupted. | Met |
| FR-2332 untrusted data | Delta rendering stays inside the existing envelope and only reads the active conversation; `tc_2332_delta_remains_untrusted_data`, `tc_2332_delta_excludes_other_tasks_and_secrets`. | Met |
| Superseded FR-027 | `SPEC-automatic-cross-agent-sharing.md` now points to FR-2323..FR-2325; its conversation test expects the own-session resume and unseen delta. | Met |

### Scope and regression review

- Binding rows and the transcript schema are unchanged; only deletion of the
  unused `TurnIdentity.context_through_seq` field was needed after the bind
  event stopped consuming the delta.
- Resume failure detection is text-based because the adapter protocol has no
  dedicated event. It covers custom-agent provider text and built-ins whose
  error text matches the same patterns; an unmatched provider error still takes
  the normal failure path.
- The fresh retry reuses the same turn id and the stored retry request, so no
  extra transcript row or user message is created.
- Summary/checkpoint behavior is unchanged; plain same-agent resume no longer
  re-sends the projection, as required by FR-2323.

### Test evidence

- `cargo test -j 2 --test conversation` - passed (35 tests).
- `cargo test -j 2 --bin dmuxai tc_232` - passed (2 tests).
- `cargo test -j 2 --bin dmuxai tc_2332` - passed (1 test).
- `cargo test -j 2 --bin dmuxai resume` - passed (12 tests).
- `cargo test -j 2 --bin dmuxai switching_agents` - passed (2 tests).
- `cargo test -j 2 --bin dmuxai provider_session` - passed (2 tests).

### Residual risks

- A provider error that means "resume rejected" but uses different wording is
  not recognized and reports a normal turn failure instead.
- A resumed agent whose provider silently ignores a delta cannot be detected;
  the next completed turn still advances `known_through_seq`.
- The delta is bounded by text bytes, not serialized envelope bytes, per
  FR-2325.
- Review diff/result rendering is out of scope (PR-E); FR-2332's envelope rule
  must be reused when that path lands.

### Rollback

Revert the conversation request-context enum, delta queries, renderer, retry
hook and tests, then restore the cross-agent-sharing FR-027 text. No data
migration is required; existing bindings remain readable and older code treats
them as before.
