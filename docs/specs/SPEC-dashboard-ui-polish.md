# SPEC-dashboard-ui-polish: Run status pill, Runs detail layout, stream noise, and readable labels

Status: proposed
Owner: DmuxAI Team
Created: 2026-11-18
Baseline: main 2b39996 (after #331); existing highest FR/TC-2839; this spec adds FR/TC-2840–2846
Related: SPEC-headless-runs-dashboard, SPEC-dashboard-tab-trim, SPEC-running-indicators, SPEC-web-dashboard; issue #148

## 1. Decision summary

A screenshot review of the dashboard during headless runs (fake-agent sandbox, light and dark, desktop and mobile) found four defects in the Runs experience. The top status pill keeps saying `idle` while headless runs are active (#148). The empty Graph box is drawn for runs without a graph because a `display: flex` rule overrides the `hidden` attribute, which on mobile pushes the log below the fold. The live stream shows duplicate and empty lines and is not ordered by time. Status values are shown as raw API tokens (`awaiting_approval`, `process_exit`). This spec fixes these in the dashboard assets only; JSON, SSE and CLI values stay unchanged.

## 2. Scope

### In scope

- The top status pill (`#run-status`) also reflects headless runs from the existing `GET /api/runs` poll (FR-2826).
- The Runs detail Graph section stays hidden for runs without a graph.
- Runs live stream: drop empty `agent` transition lines, drop `activity · output` heartbeat lines that repeat a `text` progress line, keep lines in time order, and lay lines out as time / kind / text columns.
- Human-readable labels for run status, graph node status, error kind and flags in the dashboard UI.
- Stale runs (manifest `stale: true`) do not count as active in the Runs indicator or pill and their elapsed time does not tick.

### Out of scope / non-goals

- Any change to `/api/runs`, the run SSE stream, `--progress` JSONL, run manifests or the events log (status strings stay `awaiting_approval`, `process_exit`, …).
- Splitting a `text` progress summary that the progress throttle already coalesced from several deltas (`src/progress.rs`); the dashboard shows each summary as sent. Message boundaries inside one summary are a server follow-up.
- The `#conn` connection badge semantics (it keeps showing connection state only).
- New dependencies or a new endpoint.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2840 | Session runs (Chat / `--web`) take precedence over headless runs in the pill; headless state is shown only when no session agent is running and no session turn awaits approval. | DmuxAI Team | this PR |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Status pill | `#run-status` in the status strip. | `#conn` (live / connecting / offline connection badge). |
| Live headless run | A run in `GET /api/runs` with status `active` or `running` and `stale` not true. | A stale run whose process is gone but whose manifest still says `active`. |
| Newest headless run | The run with the largest `created_at_ms` in `GET /api/runs`. | The selected run in the Runs detail. |
| Display label | The text shown for a status token, for example `Awaiting approval`. | The raw token kept in JSON, CSS class names and `data-status`. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2840 | Must | No session agent is running and no session turn awaits approval, and the latest `GET /api/runs` result (the FR-2826 poll, at most every 2 s) is known, | the pill shows `Running · N headless run(s)` with the spinner and class `run-status-running` when N ≥ 1 live headless runs exist; otherwise `Awaiting approval` (class `run-status-awaiting-approval`) when the newest headless run is `awaiting_approval`; otherwise `✗ Failed` (class `run-status-failed`) when the newest headless run is `failed`; otherwise the existing session-based text (`Idle`, `✓ Done`, …). The pill updates within one poll after the list changes. `#conn` text and classes never change because of run state. | A failed `/api/runs` fetch keeps the last known headless summary; with no headless runs the pill behaves exactly as before. Session running / approval states win (OQ-2840). | TC-2840 |
| FR-2841 | Must | When a run without a graph is selected in Runs, | the Graph section (`#runs-graph-section`) and the Full graph button are not displayed (computed `display: none`), so the stream box is the first box after the header and approval banner and fills the remaining height, on desktop and at ≤ 900 px. | Runs with a graph still show the section (FR-2823 unchanged). | TC-2841 |
| FR-2842 | Must | When the Runs stream renders a `state-change` event, | an `agent` transition with an empty message (for example `pending`, `started`) is not rendered, and no stream line contains an empty segment or a trailing ` · `. | Transitions with a message (for example `running · starting`) still render. | TC-2842 |
| FR-2843 | Must | When an `activity` / `output` state-change arrives whose message text is contained in a `text` progress line of the same run, | it is not rendered; if the matching `text` line arrives after the `activity` line, the earlier `activity` line is removed. | An `activity` / `output` line with no matching `text` line stays visible (labelled `output`). Other activity kinds (tool, file_edit) are unchanged. | TC-2843 |
| FR-2844 | Must | When stream lines arrive (progress tail and state-change events in any order), | each line is placed by its `ts_ms` (stable for equal times; lines without `ts_ms` go last) and rendered as three columns: time, kind label, text; long text wraps inside the text column and never under the time. | The 2000-line cap (RUNS_STREAM_MAX_LINES) still applies; text is set via `textContent` only (FR-2814). | TC-2844 |
| FR-2845 | Must | When the dashboard shows a run status, graph node status, error kind or run flag, | it shows a display label: `active`→`Active`, `running`→`Running`, `awaiting_approval`→`Awaiting approval`, `completed`→`Completed`, `failed`→`Failed`, `cancelled`→`Cancelled`, `approved`→`Approved`, `pending`→`Pending`, `process_exit`→`Process exit`, `stale`→`Stale`, `stalled`→`Stalled`; any other token shows with `_` replaced by spaces and the first letter upper-cased. CSS classes (`run-status-<token>`) and a `data-status` attribute keep the raw token; badges are not forced to lower case. | API / SSE / CLI JSON values are unchanged. | TC-2845 |
| FR-2846 | Should | When a run in `GET /api/runs` has `stale: true`, | it is not counted in `#runs-indicator` or by FR-2840, and its elapsed cell and detail elapsed line do not tick (they show the server `elapsed_ms`). | The stale flag badge still shows (FR-2812). | TC-2846 |

## 5. Acceptance scenarios and test design

| TC | FR | Level | Setup | Action | Expected | Test location |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2840 | FR-2840 | browser (ignored) + unit | `serve` fixture with one live active run manifest, then newest `awaiting_approval`, then newest `failed` | load dashboard, watch `#run-status` and `#conn` | pill shows Running · 1 headless run, then Awaiting approval, then ✗ Failed; `#conn` text is a connection word only | `tests/web_runs_view.rs`: `tc_2840_status_pill_headless_runs`; `src/web/assets.rs`: `tc_2840_status_pill_reads_headless_summary` |
| TC-2841 | FR-2841 | browser (ignored) + unit | run manifest without graph | select run | `#runs-graph-section` computed display is `none`; stream wrap top is directly below the header/banner | `tests/web_runs_view.rs`: `tc_2841_no_graph_section_hidden`; `src/web/assets.rs`: `tc_2841_hidden_beats_display_rules` |
| TC-2842 | FR-2842 | browser (ignored) | events log with `agent` pending/started (no message) and running/starting | select run | no `pending`/`started` line, no line ending with ` · `; `starting` line present | `tests/web_runs_view.rs`: `tc_2842_stream_drops_empty_agent_lines` |
| TC-2843 | FR-2843 | browser (ignored) | progress `text` line and `activity`/`output` event with the same text, plus one unmatched output | select run | matched output absent, unmatched output present | `tests/web_runs_view.rs`: `tc_2843_stream_dedupes_output_heartbeat` |
| TC-2844 | FR-2844 | browser (ignored) | progress and events with interleaved `ts_ms` | select run | rendered order follows `ts_ms`; each line has time, kind, text cells | `tests/web_runs_view.rs`: `tc_2844_stream_time_order_and_columns` |
| TC-2845 | FR-2845 | browser (ignored) + unit | runs with `awaiting_approval`, `failed` + `error_kind: process_exit` | load Runs, select failed run | badge text `Awaiting approval`, class keeps `run-status-awaiting_approval`, `data-status` raw; detail shows `Error kind: Process exit` | `tests/web_runs_view.rs`: `tc_2845_human_status_labels`; `src/web/assets.rs`: `tc_2845_status_label_map` |
| TC-2846 | FR-2846 | browser (ignored) | one stale active run, no live runs | load dashboard | `#runs-indicator` hidden; stale row elapsed stays at server value | `tests/web_runs_view.rs`: `tc_2846_stale_runs_not_live` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| UI | `runs.js`, `app.js`, `app.css` only; element ids unchanged; badge text changes from raw token to display label | revert the asset commit; tests that asserted raw badge text are updated to the label or to `data-status` | TC-2845 |
| API | none; `GET /api/runs` reused (no extra poll beyond FR-2826 cadence) | — | TC-2840 |
| Security | stream text still set via `textContent`; no new HTML sinks | — | TC-2814, TC-2844 |

## 7. Implementation plan

| Step | Work | Files | FR | TC |
| --- | --- | --- | --- | --- |
| 1 | `[hidden]` fix for runs detail; label helper; stale handling | `app.css`, `runs.js` | FR-2841, FR-2845, FR-2846 | TC-2841, TC-2845, TC-2846 |
| 2 | Stream model: filter, dedupe, ts order, columns | `runs.js`, `app.css` | FR-2842–FR-2844 | TC-2842–TC-2844 |
| 3 | Pill reads headless summary exported by `runs.js` | `runs.js`, `app.js` | FR-2840 | TC-2840 |

## 8. Review and test exit criteria

- [ ] `python3 scripts/check-spec.py --all` and `python3 scripts/check-tc-links.py --base origin/main` pass.
- [ ] `cargo test --locked --all-targets` passes; `cargo test --test web_runs_view -- --ignored` passes locally with Chrome.
- [ ] Before/after screenshots (desktop light running stream, failed, approval; mobile light running stream).
