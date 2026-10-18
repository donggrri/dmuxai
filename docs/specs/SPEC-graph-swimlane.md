# SPEC: graph-swimlane

Status: Draft (v0.2.2)
Created: 2026-10-18
Related: issue #61; Dongjin 2026-10-18 (picked mockup B, "horizontal swimlane", over the subway map); SPEC-graph-view (UX-700..UX-703, superseded for the Graph drawing only); SPEC-theme-colors

## 1. Decision summary

The Graph view draws a run as a "subway map": columns by dependency depth, with station circles. It shows structure but not **who** runs each step or **when**. Dongjin chose mockup B ([image](../images/graph-swimlane-mockup.png), `graph-mocks/B-horizontal.html`): a **horizontal swimlane timeline**.

- One row per runner. Time runs left to right.
- Each step is a card placed at its start time, as wide as its duration.
- A "now" line, a striped tail on running steps, and estimated start times on pending steps.
- Dependency arrows between the cards.

It replaces the subway drawing inside the existing Graph view. The data (`NodeView` with `runner`, `started_at_ms`, `finished_at_ms`, `deps`) is already in the snapshot, so the server API does not change. The List/Graph toggle, the click-for-detail panel, live SSE updates and view-only `serve` keep working.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-880 | Must | The Graph view renders one lane per runner that has at least one node, in first-use order. Each lane has a label column showing the runner name and the model of its latest node. A time axis spans run start to max(now, latest estimated end), with tick labels at "nice" steps (30s, 1m, 2m, 5m, 10m, 15m, 30m, 1h), aiming for 6-10 ticks | TC-880, TC-882 |
| FR-881 | Must | Each node is a card in its runner's lane. It is placed at `started_at_ms`, and its width is proportional to its duration (finished, or now for running nodes), with a minimum width so the id, status and elapsed time stay readable. Cards that would overlap in one lane stack into sub-rows, so the lane grows instead of overlapping. Status styles: done, running (pulsing dot plus a striped tail at the right edge), failed (with attempt), pending (dashed) | TC-880, TC-882 |
| FR-882 | Must | Pending and ready nodes show an estimated start (`est. start +3m20s`) and an estimated duration (`~2m00s`). Estimated start = the latest estimated end of their dependencies, or now if that is earlier. Estimated duration = the median duration of finished nodes in this run with the same role, else of all finished nodes, else 2 minutes. The estimate is computed in the browser from the snapshot; nothing new is stored | TC-881 |
| FR-883 | Must | A vertical "now" line with a `now 9m00s` label sits at the current elapsed time while the run is active; the area to its right is lightly shaded. When the run has ended, the line sits at the end and reads `ended 12m40s` | TC-880, TC-882 |
| FR-884 | Must | Dependency arrows go from the right edge of a dependency card to the left edge of the dependent card, colored by the source state: solid for done, running and failed; dashed for pending. **No loop-back:** when the target's left edge is not at least 24px right of the source's right edge (short steps, same start), the arrow leaves the source's bottom (or top), runs in the gap between lanes, and enters the target from the left, never curving back over the source card | TC-881, TC-882 |
| FR-885 | Must | The List/Graph toggle (`dmuxai.graphView`), wheel zoom and drag pan, `Fit`, live updates and view-only mode behave as in SPEC-graph-view UX-702 and UX-703. Clicking a card, or Enter on it, opens the existing detail panel as an overlay drawer on the right of the graph panel. The graph keeps its full width and is not squeezed. Esc or the close button closes it | TC-882 |
| UX-880 | Must | Light theme: the pending card border and dashed pending arrows have at least 3:1 contrast against the panel background (WCAG 1.4.11). All graph text has at least 4.5:1 contrast. Dark theme keeps its current contrast. Both are checked by a test that computes contrast from the theme variables | TC-881, TC-882 |
| UX-881 | Must | Fit never shrinks graph text below 11px rendered. When the timeline does not fit the panel width at that floor, Fit stops at the floor and the canvas scrolls (or pans) horizontally instead of shrinking further | TC-882 |

### Design notes

- The renderer replaces the subway code in `app.js`, still as inline SVG with no external library, and theme colors stay CSS variables. Lane label column 170px; lane height 104px per sub-row; card height 56px.
- Times use the run's earliest `started_at_ms` as zero. Nodes without a start (pending) use FR-882 estimates. A run with no started node uses now as zero.
- Arrow routing (FR-884): when `x2 - x1 >= 24`, a cubic curve right then left. Otherwise an orthogonal path down (or up) to the lane gap, left of the target's x minus 12px, then into the target's left edge. This fixes the loop-back seen on short `plan → spec` steps.

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-880 | FR-880, FR-881, FR-883 | unit (asset) | app.js, app.css | Swimlane renderer functions (lanes, axis ticks, now line, running tail, sub-row stacking) exist; the subway station code is gone; the detail panel is an overlay | `src/web/assets.rs` `graph_swimlane_assets` |
| TC-881 | FR-882, FR-884, UX-880 | unit (node-free JS check via headless Chrome) | demo graph JSON | `graphEstimate()` gives the expected estimated starts and durations; the arrow router returns a non-loop-back path when the gap is < 24px; contrast computed from the light theme variables: pending ≥ 3:1, text ≥ 4.5:1 | `tests/web_graph_view.rs` `graph_swimlane_math_in_headless_chrome` |
| TC-882 | FR-880..FR-885, UX-880, UX-881 | e2e (ignored, headless Chrome) | `dmuxai serve` with the demo run (8 nodes, 4 runners, one running, one failed, three pending) | 4 lanes; cards in their lanes in time order; now line present; the running card has a tail; pending cards show "est. start"; no arrow path passes back over its source card; clicking a card opens the drawer and the svg width is unchanged; after Fit at 900px wide every text element is ≥ 11px; light and dark screenshots captured | `tests/web_graph_view.rs` `graph_swimlane_renders_in_headless_chrome` |
