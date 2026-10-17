# PLAN: Dashboard node-graph view (slice H)

Spec: [SPEC-graph-view.md](../specs/SPEC-graph-view.md). Baseline: `1c1f692`.

## Modules

| Module | Change | Owner |
| --- | --- | --- |
| `src/web/layout.rs` (new) | `NodeLayout { layer, order }`, `layout_graph(ids, edges)`: Kahn longest-path layering, stable order by plan order, unknown endpoints and self-loops ignored, cycle members placed after their placed predecessors in input order (always terminates) | opencode deepseek |
| `src/web/snapshot.rs` | Additive `NodeView.deps`, `NodeView.layout`, `NodeView.detail { log_tail, changed_files }` (serde default) and `GraphView.layers`, filled by `snapshot_from_run` | opencode deepseek |
| `src/app.rs` | Per-node bounded detail in `dmuxai --web`: last 20 output lines (300 chars each), up to 50 workspace-relative changed files; reset on new attempt or graph | AGY |
| `src/web/assets/*` | Subway-map SVG (mockup #5), segmented progress bar, legend, List/Graph toggle (`dmuxai.graphView`), side panel, pan/zoom/Fit, theme variables in both palettes, CSP-safe | opencode deepseek (glm provider was failing) |
| `tests/web_graph_view.rs` (new) | Opt-in headless Chrome DOM check | AGY |

## Layout and drawing

The layout is computed on the server so the page and tests share one
definition. The page maps `layer` to an x column and `order` to a y row and
draws each edge as a path from the dependency to the dependent with rounded
bends. Edge color follows the source node state. Running stations pulse with
CSS keyframes, which are disabled under `prefers-reduced-motion`.

## Compatibility

All snapshot fields are additive. Older graph files without edges lay out as
one column. `dmuxai serve` uses the same snapshot path, so the Graph view
works view-only.
