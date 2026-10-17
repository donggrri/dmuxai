# SPEC: cursor-stream-dedupe

Status: Implemented (v0.2.1)
Created: 2026-10-17
Related: v0.2.0 dogfood report (Linux, `dmuxai` release binary, Cursor runner, Plan turn "이 프로젝트 파악")

## 1. Decision summary

In v0.2.0, each Cursor text segment that came before a tool call was shown twice, in the TUI and the dashboard chat alike.

cursor-agent 2026.10.16 with `--stream-partial-output` streams a segment as partial `assistant` deltas (`timestamp_ms`). Before the next tool call, it then sends a **segment aggregate**: an `assistant` event with `model_call_id` and `timestamp_ms` that repeats the whole segment. The adapter treated anything with `timestamp_ms` as a delta, so it appended the segment a second time. The final whole-turn aggregate (no `timestamp_ms`) was already skipped.

The fix: segment aggregates contribute only text that was not streamed.

## 2. Scope

- In scope:
  - `src/agent/cursor.rs` `normalize_event`;
  - a recorded, sanitized cursor-agent stream fixture;
  - unit tests.
- Out of scope: other runners. AGY, Codex and the opencode custom agents were checked end to end and did not duplicate (see §5).

## 3. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-800 | Must | Given cursor-agent `assistant` events, Then partial deltas (`timestamp_ms` without `model_call_id`) are shown as they arrive. A segment aggregate (`model_call_id`) adds only the part of its text that the partials of that segment did not already contain. The final aggregate (neither key) is used only when no text was shown in the turn | TC-800, TC-801 |
| FR-801 | Must | Given a stream without partial deltas (older cursor-agent, or no `--stream-partial-output`), Then each segment aggregate's text is shown once and the final aggregate is not added again | TC-801 |

## 4. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-800 | FR-800 | unit (recorded) | `tests/fixtures/cursor/stream-partial-tool-segments.jsonl`: a real composer-2.5 Plan turn with tool calls between segments. Tool payloads, ids and paths are stripped | Concatenated deltas equal the `result` text; the first segment appears once; one `Completed` | `src/agent/cursor.rs` `recorded_cursor_stream_text_appears_once` |
| TC-801 | FR-800, FR-801 | unit | synthetic events | Aggregates after partials add nothing, or only the unstreamed tail; without partials the segment shows once and the final aggregate is ignored | `src/agent/cursor.rs` `cursor_segment_aggregates_add_only_missing_text` |

## 5. Verification

- **TC-800 against the old logic:** reverting the fix makes TC-800 fail with "segment duplicated".
- **End to end with the real CLIs:** `dmuxai run "이 프로젝트 파악. README와 Cargo.toml을 읽고 세 문장으로 요약해줘." --json` showed no repeated sentence for:
  - `--agent cursor --mode plan`;
  - `--agent agy --mode plan`;
  - `--agent opencode-haiku`.
- **Dashboard chat:** it renders the same App transcript, so it is fixed at the same source.
