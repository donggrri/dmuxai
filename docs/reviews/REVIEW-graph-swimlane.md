# Review: horizontal swimlane graph view (#61, PR #65)

Date: 2026-10-18 (KST)
Reviewers: opencode free models only: deepseek-v4.1-flash-free (primary), glm-5.3-flash-free and nemotron-3-ultra-free (second opinions)
Scope: `src/web/assets/app.js` (swimlane section), `src/graph/{types,executor,planner}.rs`, `src/web/{snapshot,hub}.rs`

The diff was reviewed in five chunks (estimate and geometry, two halves of the
renderer, the Rust timestamp plumbing), because whole-diff prompts came back
empty. Each reviewer was asked for at most four real bugs.

## Findings

| # | Chunk / reviewer | Finding | Resolution |
| --- | --- | --- | --- |
| 1 | Rust / deepseek | `DashboardHub` keeps a stale `finished_at_ms` when a node leaves a terminal state without an attempt bump (or its Pending frame was coalesced), so a running card gets a finish before its start | Fixed: unfinished nodes never carry a finish time. Test: `hub_clears_stale_finish_when_node_runs_again` (TC-883) |
| 2 | Rust / deepseek | A retried node's elapsed time spans all attempts, which inflates the same-role median estimate | Kept by design: the card spans the whole node (first start to final finish), and estimates are only a hint. Noted in the spec ("a retry keeps the first start") |
| 3 | Renderer / deepseek | One arrowhead is drawn per edge, so a node with several dependencies gets stacked heads whose color is just the last edge's | Fixed: one head per target, colored by the most blocking source state. Test: `graph_swimlane_one_head_per_target_in_headless_chrome` (TC-884) |
| 4 | Renderer / deepseek | The now tag and the future band are not clamped | Hardened: `Math.max(0, ...)` on both. Not reachable today (the svg is at least the timeline width) |
| 5 | Renderer / deepseek | The tick loop could spin if the step were 0, or use NaN spans | Not reachable (the step is at least 30s, the span at least 60s, and NaN ends the loop). Hardened anyway: a non-finite span falls back to 60s |
| 6 | Renderer / deepseek | The throttled rebuild in `updateRunningGraphCards` doesn't refresh the geometry key, so it double-renders | Rejected: the key is only lane and sub-row per card, which a rebuild doesn't change |
| 7 | Renderer / deepseek | `graphFitText` returns two characters ("a…") when only one fits | Fixed: a one-character slot shows just "…". Covered in TC-884 |
| 8 | Renderer / deepseek | The chip can overflow short cards | Rejected: the card's minimum width already includes the id, chip and running tail |
| 9 | Renderer / deepseek | Geometry key delimiter collisions; `autoFitGraph` with undefined width | Rejected: node ids are planner ids, and geometry always has a width and height |
| 10 | Geometry / nemotron | Direct lines and curves can pass over an unrelated card between source and target | Fixed: such arrows take the routed path through the row gaps. Test: `graph_swimlane_direct_arrow_avoids_cards_in_headless_chrome` (TC-885) |
| 11 | Geometry / nemotron | NaN from a missing `planEnd`; undefined `CARD_MIN_W`; `graphFreeColumn` falls back to a blocked column | Rejected: `graphEstimate` always sets `end` and `planEnd`, and `CARD_MIN_W` is defined. The free-column fallback only applies when every candidate is blocked; the arrow is then drawn as before |
| 12 | Estimate / glm | `sent_at_ms` is used without a type check | Hardened: it must be a number, otherwise `generated_at_ms` is used |
| 13 | Estimate / glm | "At most 10 ticks" admits 11 marks | Clarified: it means 10 intervals. The comment and the spec now say so (the test already allows 11 marks) |
| 14 | Estimate / glm, nemotron | Untimed legacy nodes share a window; cycles; 10k-node recursion depth; `graphMedian([])`; zero server timestamps | Rejected: overlapping cards stack in sub-rows, the planner rejects cycles, plans are far below the recursion limit, the median is guarded by its callers, and the server always sends real timestamps |

## Reviewer reliability (this session)

Long prompts (about 10k tokens) came back empty from deepseek and glm, or hung.
Two setup issues caused most of the failures: an inherited stdin that opencode
waited on, and a permission config that the free tier rejects ("free tier can
only be used from within OpenCode"). With stdin closed and the default
permissions, chunks of about 8KB answered in 30 to 330 seconds.
