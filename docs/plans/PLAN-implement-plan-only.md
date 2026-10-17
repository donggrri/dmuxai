# PLAN: Implement plan-only and AGY terminal lifecycle

Status: complete
Spec: [SPEC-implement-plan-only.md](../specs/SPEC-implement-plan-only.md)

1. Extend `implement_outcome` with AGY tool-based write detection, plan-only classification, and optional write requirement on `finish`. (FR-501, FR-503, TC-501, TC-503, TC-509)
2. Refactor AGY `stream_child` to defer terminal events until child exit, add post-`result` grace kill, and loop `run_turn` for plan-only retry with conversation resume. (FR-500, FR-502, FR-503, FR-506, TC-500, TC-504, TC-505)
3. Change Codex, Cursor, AGY, and custom concurrent `Send` responses from `Notice` to `Error`. (FR-504, TC-506)
4. Update `App::cancel_selected` to always invoke adapter cancel and clear tracked turns when busy or `active_turns` present. (FR-505, TC-507)
5. Author spec/plan/tasks/review, dogfood #24, TEST-PLAN row, run Rust gates. (all requirements)

## Risks and mitigations

- Plan-only heuristics may misclassify prose: mitigated by single retry then explicit failure.
- Grace kill after `result` could truncate slow cleanup: bounded to a few seconds; idle timeout still applies earlier in the stream.
- Deferred terminal keeps the app busy until exit: fixes the prior busy/idle mismatch.

## Rollback

Revert adapter and app changes; no persistence migration. Behavior returns to immediate terminal events and Notice-based concurrent sends.
