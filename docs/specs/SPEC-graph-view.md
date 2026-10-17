# SPEC: Dashboard node-graph view of graph runs

Status: Implemented in v0.2.0 (slice H)
Owner: DmuxAI
Created: 2026-10-17
Baseline: `1c1f692`
Related: SPEC-web-dashboard (FR-525, FR-529), SPEC-theme-colors (UX-600)

## 1. Decision summary

Today the dashboard lists graph-run nodes in a linear list, which hides
parallelism and dependencies. This feature adds a Graph view that draws the
run's DAG as embedded SVG, with no external library and no CDN:

- Nodes are laid out left to right in columns by dependency depth, and
  parallel nodes are stacked vertically in the same column, like the chosen
  mockup #5.
- Nodes are drawn as subway-map stations and edges as thick lines, and nodes are colored by state using the
  dashboard theme.
- The view updates live over the existing SSE stream.

Clicking a node opens a details panel. A List/Graph toggle chooses the view,
and the choice is remembered. Pan and zoom handle large graphs. The view also
works in `dmuxai serve` view-only mode.

## 2. Scope

### In scope

- A pure layout function (server-side Rust, exposed in the snapshot) that
  gives every node a layer and an order within the layer. It handles cycles
  and missing edge endpoints safely.
- Snapshot additions:
  - `graph.nodes[].deps`, the incoming node ids;
  - `graph.nodes[].layout {layer, order}`;
  - `graph.nodes[].detail {log_tail[], changed_files[]}` when available.
- The page:
  - an SVG graph view with state colors, a pulsing running node and subway-line
    edges drawn from dependency to dependent;
  - a details side panel;
  - the List/Graph toggle stored in `localStorage` (`dmuxai.graphView`);
  - wheel/drag pan and zoom with a "fit" button.

### Out of scope / non-goals

- Editing the graph from the browser, or reordering nodes.
- Persisting node logs to the graph files (`serve` shows details only when
  the run file carries them).

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Layer | Longest-path depth from the roots (roots = 0); drawn as a column, left to right | Attempt |
| Order | Position within a layer: stable by the plan's node order, then id | Execution order |
| Visual state | pending/ready -> gray; running -> accent and pulsing; waiting_gate -> amber; completed -> green; failed -> red; skipped -> muted | Graph phase |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-700 | Must | Given a graph plan with nodes and edges | `layout_graph` assigns `layer` = the longest dependency depth from the roots, and `order` = a stable position within the layer by plan order; nodes without dependencies are layer 0 | Edges to unknown nodes are ignored; a cycle never hangs or panics (the nodes in the cycle are placed after their acyclic predecessors, with a deterministic order); 0 nodes -> empty layout | TC-700, TC-701 |
| FR-701 | Must | Given `/api/state` or SSE with a graph | Each node carries `deps[]` (incoming ids, in plan order) and `layout {layer, order}`; the graph carries `layers` (the count) | Additive fields within `dmuxai.dashboard/v1` | TC-702 |
| FR-702 | Should | Given `dmuxai --web` | Each node carries `detail.log_tail` (the last 20 lines of that node's agent output, each truncated to 300 chars) and `detail.changed_files` (paths the node's turn reported writing, workspace-relative, at most 50) | `serve` mode, or no data -> empty arrays; never the token or absolute paths outside the workspace | TC-703 |
| UX-700 | Must | Given the page with a graph (design: mockup #5 "subway map", chosen by Dongjin) | A Graph view renders an inline `<svg>` subway map: one `<g class="node" data-id>` per node drawn as a circular station (completed: filled with a check mark; failed: filled with a cross; running: concentric pulsing ring; pending/ready: hollow ring; waiting_gate: amber ring; skipped: muted) labelled with the id above and runner · elapsed (or the state word) below, as in the mockup, with the role in the station tooltip, its aria-label and the side panel; edges are thick colored lines (`<path class="edge">`, colored by the source node's state) from dependency to dependent; a large segmented progress bar above the map (completed green, running striped blue, failed red, pending dark) shows completed/total as a percentage and the counts, with an ETA only when derivable from real timings (otherwise omitted); a legend (pending, running, completed, failed) sits below the map; state classes `state-<status>`; colors come from theme CSS variables; the running node pulses (CSS animation, disabled under `prefers-reduced-motion`) | No external resources; it works under the existing CSP | TC-704, TC-705 |
| UX-701 | Must | Given the Graph view | Clicking or pressing Enter on a node opens a side panel with runner, model, status, attempt, elapsed, job, the log tail and changed files; Escape or a close button closes it; the panel updates live | Keyboard accessible (nodes focusable, `role="button"`, `aria-label`) | TC-705 |
| UX-702 | Must | Given the page | A List/Graph toggle switches views and persists `dmuxai.graphView` in `localStorage`; the default is Graph when the graph has more than one layer, else List | `localStorage` unavailable -> works for the session | TC-705 |
| UX-703 | Should | Given a large graph | Mouse wheel zooms around the cursor (0.25x–3x), drag pans, and a "Fit" button resets the view; live updates keep the current pan/zoom | Touch pinch is not required | TC-705 |
| NFR-700 | Must | Given `dmuxai serve` (view-only) | The Graph view works the same with the file source | — | TC-702 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-700 | FR-700 | unit | diamond `a->b, a->c, b->d, c->d`, plus an isolated `e` and an edge to a missing node | `layout_graph` | `a:0/0`, `b:1/0`, `c:1/1`, `d:2/0`, `e:0/1`; the missing edge is ignored; the result is stable across runs | `src/web/layout.rs` `layout_assigns_layers_and_stable_order` |
| TC-701 | FR-700 | unit | a cycle `x->y->x` with root `r->x`; a self-loop; an empty plan | `layout_graph` | it terminates; every node gets a layer; `r` is 0; the order is deterministic; empty -> empty | `src/web/layout.rs` `layout_is_cycle_safe_and_handles_empty` |
| TC-702 | FR-701, NFR-700 | unit + integration | a `GraphRun` fixture with edges; the file source | `snapshot_from_run` and GET `/api/state` via `serve` | nodes have `deps` and `layout`; the graph has `layers` | `src/web/snapshot.rs` `snapshot_exposes_deps_and_layout`; `tests/web_serve.rs` (extend) |
| TC-703 | FR-702 | unit | App test helper with a running graph node that produced output lines and a write event | publish | `detail.log_tail` holds the last lines (at most 20, truncated); `changed_files` is relative and at most 50 | `src/app.rs` `dashboard_nodes_carry_log_tail_and_changed_files` |
| TC-704 | UX-700 | unit (JS-free structural) | the embedded `app.js`/`app.css`/`index.html` | string/structure checks | the SVG renderer creates `g.node[data-id]` with `circle` stations, `path.edge` lines, `state-` classes, the progress bar with a percentage, and no ETA text unless timings exist; CSS defines `@keyframes` for running plus a `prefers-reduced-motion` override; no `http(s)://` resource URLs | `src/web/assets.rs` `graph_view_svg_structure` |
| TC-705 | UX-700..UX-703 | e2e (headless browser, opt-in) | `dmuxai serve` with a demo diamond graph | load the page and read the DOM | `svg g.node` count == node count; the edge count == the dependency count; clicking a node shows the panel; the toggle persists across a reload | `tests/web_graph_view.rs` `#[ignore]` `graph_view_renders_in_headless_chrome` (skips if no Chrome); the README screenshot is captured the same way |
