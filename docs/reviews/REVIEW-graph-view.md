# REVIEW: Dashboard node-graph view (slice H)

Spec: [SPEC-graph-view.md](../specs/SPEC-graph-view.md). Plan:
[PLAN-graph-view.md](../plans/PLAN-graph-view.md). Design: mockup #5
("subway map"), chosen by Dongjin.

## Compliance

| ID | Status | Evidence |
| --- | --- | --- |
| FR-700 | Met | `src/web/layout.rs` `layout_graph`; TC-700, TC-701 (cycle nodes have fixed layer/order) |
| FR-701 | Met | `NodeView.deps`, `NodeView.layout`, `GraphView.layers`; TC-702 and `tests/web_serve.rs` |
| FR-702 | Met | `NodeDetailTracker` in `src/app.rs`: per-node attribution by runner turn, reset per attempt, bounded buffers, token redaction, workspace-relative paths only; TC-703 |
| UX-700 | Met | Subway SVG, segmented progress bar, legend, theme variables in both palettes, reduced-motion; TC-704, TC-705 |
| UX-701 | Met | Side panel (runner, model, status, attempt, elapsed, job, log tail, changed files), Enter/Esc, focus returns to the station |
| UX-702 | Met | `dmuxai.graphView` in `localStorage`, default Graph when `layers > 1`; TC-705 reload check |
| UX-703 | Met | Wheel zoom 0.25x–3x around the cursor, drag pan, Fit; user pan/zoom kept across live updates and geometry changes |
| NFR-700 | Met | Same snapshot path in `dmuxai serve`; screenshot taken in serve mode |

## Who did what

| Runner | Work |
| --- | --- |
| opencode deepseek | `layout_graph`, snapshot fields, served-state test; first two rounds of the subway-map UI (took over from glm, whose provider failed upstream) |
| AGY | FR-702 node detail tracking, TC-703, opt-in headless Chrome test TC-705 |
| cursor (composer-2.5) | Cross-review of UI (12 findings) and of the node-detail code (12 findings); UI fix round; FR-702 fix round after AGY ran out of quota |
| Grok Bot (hand) | Spec wording (left-to-right layout, labels per mockup), plan/tasks/review docs, README section, screenshots |

## Cross-review 1 (cursor on the UI)

No blockers. Fixed: full SVG rebuild on every 1 s tick and in List view
(now rebuilt only when the revision or geometry changes; ticks update the
running stations' elapsed text in place); auto-fit after the user panned or
zoomed; fallback order when nodes lack `layout`; weak cycle assertions in
TC-701; Esc focus restore; role now in the station tooltip, aria-label and
panel. Accepted: TC-704 is a structural string check by design (the
behavioral check is TC-705); progress-bar widths use CSSOM `style.width`,
which the CSP allows; spec "arrow edges" reworded to subway lines to match the
chosen design.

## Cross-review 2 (cursor on node detail)

Blocker #1 (all parallel runners' output attributed to the last started node)
and majors #2 (no reset on a new attempt), #3 (unbounded partial line), #4
(no token redaction) and minor #5 (`is_write_tool` substring matches) were
fixed with tests; see `dashboard_nodes_carry_log_tail_and_changed_files`.

## Residual risks

- TC-705 needs Chrome and runs only with `--ignored`; CI runs the structural
  test only.
- In `serve` mode the side panel's log tail and changed files are empty
  (the file source has no agent stream), as specified.
- Very wide or tall graphs (100+ nodes in one layer) render correctly but the
  SVG gets tall; Fit keeps them viewable.
