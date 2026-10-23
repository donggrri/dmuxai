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

## 4. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-800 | Must | Given cursor-agent `assistant` events, Then partial deltas (`timestamp_ms` without `model_call_id`) are shown as they arrive. A tool call ends the segment. A segment aggregate (`model_call_id`) adds only the tail beyond that segment's streamed partials. When the segment had no partials, it adds its whole text, even if it repeats an earlier segment. The final aggregate (neither key) is compared with the turn: as a whole-turn aggregate it adds the tail beyond everything shown; as a repeat of the last segment it adds only that segment's unstreamed tail, and nothing if that text was already shown. An aggregate that does not extend the streamed partials (for example re-formatted whitespace) adds nothing, because the segment is already visible. Full assistant messages repeating a segment across a tool call do not duplicate the paragraph, and paragraph separators collapse excess newlines to exactly one blank line | TC-800, TC-801, TC-1684, TC-1685, TC-1686 |
| FR-801 | Must | Given a stream without partial deltas (older cursor-agent, or no `--stream-partial-output`), Then each segment aggregate's text is shown once. The final aggregate adds only the part that is not on screen yet: nothing when it repeats the last segment; the unseen tail when it is a whole-turn aggregate | TC-801 |

## 5. Acceptance scenarios and test design

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-800 | FR-800 | unit (recorded) | `tests/fixtures/cursor/stream-partial-tool-segments.jsonl`: a real composer-2.5 Plan turn with tool calls between segments. Tool payloads, session/request ids and paths are stripped. Random tool `call_id`s are kept | Concatenated deltas equal the `result` text; the first segment appears once; one `Completed` | `src/agent/cursor.rs` `recorded_cursor_stream_text_appears_once` |
| TC-801 | FR-800, FR-801 | unit | synthetic events | Aggregates after partials add nothing, or only the unstreamed tail; without partials each segment shows once. Also covered: tool-call segment reset; identical consecutive segments; a whole-turn final aggregate after streamed partials; a final aggregate repeating the last segment or the whole turn; an unstreamed last segment; Korean tails; a non-extending aggregate; an error after partials | `src/agent/cursor.rs` `cursor_segment_aggregates_add_only_missing_text`, `cursor_aggregate_edge_cases` |
| TC-1684 | FR-800 | unit | realistic stream-json lines with partial deltas followed by a full assistant message repeating the paragraph and a tool call | Stream normalization emits the first status paragraph exactly once | `src/agent/cursor.rs` `tc_1684_no_duplicate_first_paragraph`, `tc_1684_no_duplicate_first_paragraph_after_tool_call` |
| TC-1685 | FR-800 | unit | realistic stream-json lines with paragraphs separated across chunks or tool calls | Consecutive paragraphs are separated by exactly one blank line (`\n\n`) with excess newlines collapsed | `src/agent/cursor.rs` `tc_1685_single_blank_line_between_paragraphs`, `tc_1685_single_blank_line_collapses_excess_newlines` |
| TC-1686 | FR-800 | unit | realistic stream-json lines with short repeated deltas | Short or prefix-matching deltas are kept as real text instead of being treated as repeats | `src/agent/cursor.rs` `tc_1686_short_repeated_deltas_are_kept` |

## 6. Verification

- **Review:** opencode-haiku (Cursor is not a reviewer for v0.2.1); see [REVIEW-cursor-stream-dedupe](../reviews/REVIEW-cursor-stream-dedupe.md).

- **TC-800 against the old logic:** reverting the fix makes TC-800 fail with "segment duplicated".
- **End to end with the real CLIs:** `dmuxai run "이 프로젝트 파악. README와 Cargo.toml을 읽고 세 문장으로 요약해줘." --json` showed no repeated sentence for:
  - `--agent cursor --mode plan`;
  - `--agent agy --mode plan`;
  - `--agent opencode-haiku`.
- **Dashboard chat:** it renders the same App transcript, so it is fixed at the same source.
