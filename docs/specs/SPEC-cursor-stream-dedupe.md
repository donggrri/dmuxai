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
| FR-800 | Must | Given cursor-agent `assistant` events, Then partial deltas (`timestamp_ms` without `model_call_id`) are shown as they arrive. A tool call ends the segment. Any aggregate, whether a segment aggregate (`model_call_id`) or the final aggregate (neither key, repeating the last segment), adds only text that is not on screen yet: the tail beyond that segment's streamed partials, or, when the segment had no partials, its text unless it was already shown. An aggregate that does not extend the streamed partials (for example re-formatted whitespace) adds nothing, because the segment is already visible | TC-800, TC-801 |
| FR-801 | Must | Given a stream without partial deltas (older cursor-agent, or no `--stream-partial-output`), Then each segment aggregate's text is shown once. The final aggregate adds only the part that is not on screen yet: nothing when it repeats the last segment; the unseen tail when it is a whole-turn aggregate | TC-801 |

## 4. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-800 | FR-800 | unit (recorded) | `tests/fixtures/cursor/stream-partial-tool-segments.jsonl`: a real composer-2.5 Plan turn with tool calls between segments. Tool payloads, session/request ids and paths are stripped. Random tool `call_id`s are kept | Concatenated deltas equal the `result` text; the first segment appears once; one `Completed` | `src/agent/cursor.rs` `recorded_cursor_stream_text_appears_once` |
| TC-801 | FR-800, FR-801 | unit | synthetic events | Aggregates after partials add nothing, or only the unstreamed tail; without partials each segment shows once. Also covered: tool-call segment reset; a final aggregate repeating the last segment or the whole turn; an unstreamed last segment; Korean tails; a non-extending aggregate; an error after partials | `src/agent/cursor.rs` `cursor_segment_aggregates_add_only_missing_text`, `cursor_aggregate_edge_cases` |

## 5. Verification

- **Review:** opencode-haiku (Cursor is not a reviewer for v0.2.1); see [REVIEW-cursor-stream-dedupe](../reviews/REVIEW-cursor-stream-dedupe.md).

- **TC-800 against the old logic:** reverting the fix makes TC-800 fail with "segment duplicated".
- **End to end with the real CLIs:** `dmuxai run "이 프로젝트 파악. README와 Cargo.toml을 읽고 세 문장으로 요약해줘." --json` showed no repeated sentence for:
  - `--agent cursor --mode plan`;
  - `--agent agy --mode plan`;
  - `--agent opencode-haiku`.
- **Dashboard chat:** it renders the same App transcript, so it is fixed at the same source.
