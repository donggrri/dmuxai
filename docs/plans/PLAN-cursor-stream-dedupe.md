# PLAN: cursor-stream-dedupe

Spec: [SPEC-cursor-stream-dedupe](../specs/SPEC-cursor-stream-dedupe.md)

Status: implemented in v0.2.1

| Area | File | Change |
| --- | --- | --- |
| Adapter | `src/agent/cursor.rs` | `CursorStreamState.segment_text`; `normalize_event` treats `model_call_id` events as segment aggregates (emits only the unstreamed suffix) |
| Fixture | `tests/fixtures/cursor/stream-partial-tool-segments.jsonl` | Recorded composer-2.5 Plan stream, sanitized |
