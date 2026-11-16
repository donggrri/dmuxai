# SPEC-dashboard-tab-trim: Fewer top-level dashboard tabs for watching runs

Status: proposed
Owner: DmuxAI Team
Created: 2026-11-16
Baseline: feat/dash-runs-view (PR #327, SPEC-headless-runs-dashboard); existing highest FR-2817; this spec adds FR/TC-2820–2828
Related: SPEC-headless-runs-dashboard, SPEC-web-dashboard, SPEC-graph-view, SPEC-dashboard-fullwidth; ts opinion round 2026-11-16 (opencode glm, cursor composer-2.5, agy hydrogen, grok-4.7)

## 1. Decision summary

The dashboard top bar grew to ten controls (Theme, Accent, Reset, Sessions, To-dos, Chat, Diff, Graph, Runs, Memory, Skills, System prompts). The owner mostly watches headless runs and graphs. All four reviewing agents agreed on keeping Runs, Diff and the status strip, folding Graph into the Runs detail, and moving configuration popovers behind a menu. This spec trims the top bar to **Runs | Diff | Chat | Workspace | More** without removing any feature, and fixes five defects found during the review.

## 2. Scope

### In scope

- Top bar: Runs, Diff, Chat, Workspace (opens the right sidebar with its Files / To-dos tabs), More (menu: Sessions, Memory, Skills, System prompts, Theme, Accent, Reset layout).
- Graph diagram inside the Runs detail of a run that has a graph, with a "Full graph" button that opens the existing large Graph view; the Graph view itself stays.
- Retarget the Chat "Open in Graph" link and the plan-approval auto-open.
- Defaults: `serve` → Runs (when headless runs exist, FR-2811); `--web` → Chat.
- Fixes: Reset also resets layout; `#runs-indicator` reflects active runs; System prompts editor shows the current prompt preview (FR-2827 removed: config writes on `serve` are allowed by design).

### Out of scope / non-goals

- Removing any view or API.
- Diff for headless-run edits (Diff is fed by session agent turns; headless file changes need a journal diffstat) — tracked as #328.
- New dependencies.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2820 | Under `--web`, plan approvals still need the large Graph view (Approve/Edit/Reject); the Runs detail shows the diagram and a "Full graph" button, not the approval controls. | DmuxAI Team | PR C |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Top bar | The row of view buttons in `index.html` header. | The status strip (run state, workspace, source, rev, live). |
| More menu | A popup menu button in the top bar holding secondary controls. | Browser context menus. |
| Workspace | The right sidebar (`#sidebar`) with Files and To-dos tabs. | The workspace picker in the status strip. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2820 | Must | When the dashboard loads, | the top bar shows exactly these view controls in order: Runs, Diff, Chat, Workspace, More; the old top-level Sessions, To-dos, Graph, Memory, Skills, System prompts buttons, Theme select, Accent picker and Reset button are not in the top bar. | Keyboard: every control reachable with Tab; Esc closes menus and views as before. | TC-2820 |
| FR-2821 | Must | When More is opened, | a menu lists Sessions, Memory (with its On/Off badge), Skills, System prompts, Theme, Accent and Reset layout; choosing an item opens the same sidebar / popover / control as the old button did. | Esc or a click outside closes the menu; the menu button has `aria-haspopup="menu"` and `aria-expanded`. | TC-2821 |
| FR-2822 | Must | When Workspace is clicked, | the right sidebar opens (or closes) and keeps its Files / To-dos tabs; the to-do count badge is shown on the Workspace button. | — | TC-2822 |
| FR-2823 | Must | When a run with a graph is selected in Runs, | the detail shows the graph diagram for that run (the same renderer as the Graph view, read-only) plus a "Full graph" button that opens the large Graph view focused on that graph. | Runs without a graph show no diagram and no button. | TC-2823 |
| FR-2824 | Must | When Chat "Open in Graph" is clicked or a plan awaiting approval auto-opens, | the large Graph view opens as before (the Graph view still exists without a top-bar button); when the graph belongs to a headless run listed in Runs, the Runs view selects that run instead and offers "Full graph". | The plan auto-open never hides the approval controls under `--web`. | TC-2824 |
| FR-2825 | Must | When Reset layout is chosen, | the accent returns to the snapshot default and the sidebar widths and open/closed state return to their defaults (the same as double-clicking each resize handle). | — | TC-2825 |
| FR-2826 | Must | While headless runs are active, | the Runs button shows a running indicator (`#runs-indicator` visible with the active count in its accessible label); it is hidden when none are active. | Updates on the runs poll / state-change, at most every 2 s. | TC-2826 |
| FR-2827 | Removed | (review finding: view-only guard for Memory / Skills / System prompts under `serve`) | **Removed** (2026-11-16): `serve` is view-only for runs and chat only; Memory writes (FR-727) and System prompt saves (FR-1706, TC-1720) are allowed on `serve` by design, so no guard is added. | — | — |
| FR-2828 | Must | When the System prompts editor opens for an agent that has a prompt, | the textarea stays empty (the full text is never sent, FR-1706) and its placeholder shows the existing 200-character preview from `GET /api/agent-prompts` as "Current: <preview>". | No prompt → the old placeholder. | TC-2828 |

## 5. Acceptance scenarios and test design

| TC | FR | Level | Setup | Action | Expected | Test location |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2820 | FR-2820 | browser | `serve` fixture | load dashboard | top-bar view controls are exactly Runs, Diff, Chat, Workspace, More | `tests/web_tab_trim.rs`: `tc_2820_top_bar_controls` |
| TC-2821 | FR-2821 | browser | `serve` fixture | open More, pick each item, Esc | each opens its sidebar/popover; Esc closes; aria attributes | `tests/web_tab_trim.rs`: `tc_2821_more_menu_items` |
| TC-2822 | FR-2822 | browser | fixture with to-dos | click Workspace | sidebar opens with Files/To-dos tabs; badge shows count | `tests/web_tab_trim.rs`: `tc_2822_workspace_button` |
| TC-2823 | FR-2823 | browser | run manifest with graph_id + graph fixture | select run | diagram rendered in detail; Full graph opens Graph view | `tests/web_runs_view.rs`: `tc_2823_runs_detail_graph_and_full_graph` |
| TC-2824 | FR-2824 | browser | chat transcript with Open in Graph; pending plan | click link / pending plan | Graph view opens; headless-run graph selects run in Runs | `tests/web_tab_trim.rs`: `tc_2824_open_in_graph_retarget` |
| TC-2825 | FR-2825 | browser | changed accent and sidebar width | Reset layout | accent and widths back to defaults | `tests/web_tab_trim.rs`: `tc_2825_reset_layout` |
| TC-2826 | FR-2826 | browser | active run manifest, then completed | watch Runs button | indicator visible with count, then hidden | `tests/web_runs_view.rs`: `tc_2826_runs_indicator` |
| TC-2828 | FR-2828 | browser | `serve` fixture with a configured agent prompt | open System prompts, look at that agent's editor | textarea empty, placeholder contains "Current:" and the preview, full text absent from the DOM | `tests/web_tab_trim.rs`: `tc_2828_system_prompt_preview_placeholder` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| UI | buttons move into More / Workspace; element ids of popovers and views are kept | old ids kept where tests or code rely on them; removed top-bar buttons only | TC-2820–TC-2822 |
| API | no API change (FR-2827 removed; FR-1706 keeps full prompt text out of responses) | — | TC-2828 |
| Storage | `dmuxai.centerView` values unchanged (`graph` still valid) | — | TC-2824 |

## 7. Implementation plan

| Step | Work | Files | FR | TC |
| --- | --- | --- | --- | --- |
| 1 | Small fixes: reset layout, runs indicator, prompt preview placeholder | `app.js`, `sidebar-resize.js`, `runs.js`, `agent-prompts.js` | FR-2825, FR-2826, FR-2828 | TC-2825, TC-2826, TC-2828 |
| 2 | Top bar: Workspace, More menu | `index.html`, `app.css`, `app.js` | FR-2820–FR-2822 | TC-2820–TC-2822 |
| 3 | Graph in Runs detail, retarget | `runs.js`, `app.js`, `session.js` | FR-2823, FR-2824 | TC-2823, TC-2824 |

## 8. Review and test exit criteria

- [ ] `python3 scripts/check-spec.py --all` and `python3 scripts/check-tc-links.py` pass.
- [ ] `scripts/gate.sh --browser` and `cargo test --test 'web_*' -- --ignored` pass; existing web tests changed only where a moved control's selector no longer exists (listed in the PR).
- [ ] Perf: trivial headless run within 10 % of main (#301).
