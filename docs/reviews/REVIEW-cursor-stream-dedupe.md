# REVIEW: cursor-stream-dedupe (PR #39)

Reviewer: opencode-haiku (`openrouter/anthropic/claude-haiku-5.5`), run with `dmuxai run --agent opencode-haiku --mode ask`. Cursor is not a reviewer for v0.2.1 (Dongjin, 2026-10-17 23:47); an earlier Cursor review request was dropped.

## Round 1: REQUEST_CHANGES

| # | Finding | Resolution |
| --- | --- | --- |
| 1 | `segment_text` was reset only by a `model_call_id` aggregate, not by `tool_call`, so a later aggregate could be compared against two segments | Fixed: `tool_call` clears the segment. Covered by `cursor_aggregate_edge_cases` |
| 2 | A non-prefix aggregate is silently dropped; behaviour undefined | Defined in FR-800: the segment is already visible, so it adds nothing. Tested (re-formatted whitespace) |
| 3 | Missing cases: several segments without partials; failure; Korean boundary | Added in `cursor_aggregate_edge_cases` |
| 4 | No review record or test evidence | This file. `cargo test --all-targets --locked` output is in the PR |
| 5 | Spec says ids are stripped, but tool `call_id`s remain | Spec reworded: random tool `call_id`s are kept |
| 6 | TASKS marked a Cursor review as done | Now says opencode-haiku |
| 9 | The final aggregate was skipped whenever any text had been shown, so a last segment without partials would be lost | Fixed: segment and final aggregates share one rule (`aggregate_missing`). The fixture shows the final aggregate repeats only the last segment (445 of 488 chars) |

Nits 7 (rustfmt) and 8 (redundant assert): `cargo fmt --check` is clean, and the assert is kept because it gives a clearer failure message.
