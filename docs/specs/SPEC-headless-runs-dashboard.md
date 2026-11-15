# SPEC-headless-runs-dashboard: Watch headless runs from the dashboard (view-only)

Status: proposed
Owner: DmuxAI Team
Created: 2026-11-15
Baseline: main b2e8b6b; existing highest FR-2693; this spec adds FR/TC-2800–2810, 2815–2816 (PR A); PR B adds FR/TC-2811–2814, 2817 (included)
Related: SPEC-run-journal, SPEC-agent-api, SPEC-web-dashboard, SPEC-graph-view, issues #147, #148, #301

## 1. Decision summary

A supervising agent (for example Grok Bot) drives dmuxai through headless `dmuxai run` / `run --graph` / MCP `run`. Today those runs are invisible in the browser: `run` starts no HTTP server, `serve` shows only the newest graph file, events carry no run id, and the run journal (SPEC-run-journal PR1a) is never written by real runs. This spec makes headless runs observable, read-only:

- PR A (backend): every headless run writes a run-journal manifest (active before dispatch, terminal at the end) plus a masked `progress.jsonl`; events carry `run_id`; the event tail is incremental; `serve` (and `--web`) expose `GET /api/runs`, `GET /api/runs/{id}` and `GET /api/runs/{id}/stream`.
- PR B (frontend): a Runs view lists active and recent runs and shows one run live (stream, graph nodes, approval banner with the CLI resume hint, final exit).

Controls (stop, approve from the browser) and the terminal-session panel are out of scope; the approval banner only shows the CLI command because a headless run waiting for approval (exit 7) has already exited.

## 2. Scope

### In scope

- Journal lifecycle for headless `run`, `run --graph`, `todo run` and MCP-spawned runs (origin `cli_headless` / `mcp`).
- Masked, size-capped progress copy per run, written whether or not `--progress` is set.
- `run_id` on event records; byte-offset incremental event tail.
- Read-only runs API on the dashboard server, behind the existing token.
- Runs view in the existing dashboard assets (no new dependencies).

### Out of scope / non-goals

- Browser stop/cancel/approve (research PR 7); terminal-session panel (research PR 6).
- TUI / `--web` chat turn journaling, child stderr capture, git diffstat (left to the larger PR1b work, SPEC-run-journal-live draft).
- Cost/token display (no data source yet).
- Orphan cleanup after SIGKILL; such runs show as `stale`.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2800 | A run is `stale` when its manifest is active but its pid no longer exists (Linux `/proc/<pid>`); on other platforms `stale` is always false. | DmuxAI Team | PR A |
| OQ-2801 | A run is `stalled` when active and its newest progress line is older than 90 s (three missed 30 s heartbeats). | DmuxAI Team | PR A |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Run | One headless `dmuxai run` process invocation, identified by a 16-hex `run_id`. | A graph node attempt or a conversation turn. |
| Run dir | `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs/<run_id>/` (SPEC-run-journal). | `.dmuxai/` workspace state. |
| Progress copy | `<run dir>/progress.jsonl`, masked copy of the `--progress` JSON lines. | stderr progress output. |

Lifecycle: `active` (before dispatch) → `completed` (exit 0) | `failed` (exit 1–6) | `awaiting_approval` (exit 7) | `cancelled` (exit 130). `awaiting_approval` runs whose plan was later approved and executed (FR-2816) become `approved` (terminal).

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2800 | Must | When a headless `run`, `run --graph` or `todo run` starts, | an `active` manifest is saved before the agent is dispatched, with a fresh 16-hex `run_id`, `origin` `cli_headless` (or `mcp` when the env var `DMUXAI_RUN_ORIGIN=mcp` is set by the MCP server for its child run), `pid` of the dmuxai process, `requested_runner`/`requested_model`, `dmuxai_version`; the `--json` outcome includes `run_id`. | Rejected-before-start runs (usage errors) write no manifest. | TC-2800 |
| FR-2801 | Must | When the headless run ends, | the manifest becomes `completed` (exit 0), `awaiting_approval` (exit 7), `cancelled` (exit 130) or `failed` (other), with `completed_at_ms`, `duration_ms`, `dmuxai_exit_code`, `error_kind`, masked `error_summary` (≤ 2 KiB), `resolved_runner`/`resolved_model` when known, `links.graph_id`, `links.conversation_id`, and `end_summary.plan_hash` when a pending plan was saved. | Unknown values stay explicit null. | TC-2801 |
| FR-2802 | Must | While the run executes, | every progress line (the same JSON as `--progress`) is appended, masked with `mask_sensitive_tokens`, to `<run dir>/progress.jsonl`, also when `--progress` is off; stderr output is unchanged. | The copy stops growing at 4 MiB (later lines dropped). | TC-2802 |
| FR-2803 | Must | When the journal cannot be written, | one warning is printed to stderr and the run's outcome, stdout JSON and exit code are unchanged. | No panic; progress copy failures are silent after the first warning. | TC-2803 |
| FR-2804 | Must | When a headless run appends events, | each event record carries `run_id` of that run; records without a run id omit the field; older lines without it still parse. | — | TC-2804 |
| FR-2805 | Must | When the dashboard tails `events.jsonl`, | it reads only bytes after its last offset (`events::read_since_from`), never the whole file each tick; a partial trailing line is not consumed until complete; a file shorter than the offset restarts from 0 filtered by `seq`. | Malformed lines are skipped as before. | TC-2805 |
| FR-2806 | Must | `GET /api/runs?limit=N` on `serve` or `--web`, | returns `{"runs":[...]}` newest first (default 50, max 200) with run_id, origin, status, runner/model, created_at_ms, elapsed_ms (to now when active), exit code, error_kind, graph_id, `stale`, `stalled`. | Missing runs dir → empty list; malformed manifests skipped; invalid limit → 400. | TC-2806 |
| FR-2807 | Must | `GET /api/runs/{id}`, | returns the manifest, `graph` (graph view from `.dmuxai/graphs/<graph_id>.json` when present), `approval` `{plan_hash, hint}` when `awaiting_approval`, and the last 200 progress lines. The hint is `dmuxai run --graph --approve-plan-id <plan_hash> -` (graph) or `dmuxai run --agent <runner> --approve-plan -` (direct), never `--allow-sensitive`. | Invalid id → 400; unknown → 404. | TC-2807 |
| FR-2808 | Must | `GET /api/runs/{id}/stream` (SSE), | sends `run` (manifest on change), `progress` (new progress lines) and `state-change` (events whose `run_id` matches), using incremental offsets; sends `end` once the run is terminal and drained. | Unknown id → 404. | TC-2808 |
| FR-2809 | Must | For every runs endpoint, | only GET is routed; the existing dashboard token check applies; payload strings are masked and never contain the dashboard token. | Missing/wrong token → 401 as other `/api` routes. | TC-2809 |
| FR-2810 | Should | When a trivial headless run is benchmarked against main, | wall time stays within 10 % (#301 rule); numbers are recorded in the PR; masking skips its regexes for lines with no secret-like literal and headless manifest saves stay atomic without fsync. | — | TC-2810 + PR bench |
| FR-2811 | Must | When the dashboard loads, | a "Runs" view toggle exists in the top bar; under `serve` the Runs view opens by default when no center view is saved and the conversation transcript is empty (it never covers an existing conversation). | — | TC-2811 |
| FR-2812 | Must | When the Runs view is open, | it lists runs with status badge, origin, runner/model, elapsed time, `stale`/`stalled` badges; it refreshes on SSE `state-change` and at most every 2 s by polling. | Empty list shows a hint on how to start `dmuxai serve` alongside headless runs. | TC-2812 |
| FR-2813 | Must | When a run is selected, | the detail shows the live stream (`EventSource` on `/api/runs/{id}/stream`), graph nodes with status, the approval banner with a copy button for the hint, and the final exit code / error kind. | Stream errors reconnect with backoff; terminal runs show history without reconnect loops. | TC-2813 |
| FR-2814 | Must | When run data is rendered, | text is inserted with `textContent` (never `innerHTML` of payload strings); no new JS/CSS dependencies. | — | TC-2814 |
| FR-2815 | Must | While a headless run is active (#326), | the manifest gets the resolved runner, resolved model and `links.graph_id` as soon as each is known (relaxed save only when a value changes, at most a handful of saves per run), so `GET /api/runs/{id}` shows the graph view and runner/model before the run ends. | — | TC-2815 |
| FR-2816 | Must | When a headless `run --graph --approve-plan-id <hash>` starts executing the approved plan (#325), | every earlier run manifest in the same repo with status `awaiting_approval` and `end_summary.plan_hash == <hash>` is changed to status `approved` with `end_summary.approved_by_run_id = <new run id>` (bounded scan of the most recent run dirs; only when `--approve-plan-id` is given). `approved` is a terminal status; the runs API no longer returns an approval hint for it. | — | TC-2816 |
| FR-2817 | Must | When the Runs view shows a run that is active or `approved` (#325, #326), | runner/model and the graph node list update while the run is active (detail refetched when the run's manifest changes, e.g. on the stream `run` event); an `approved` run shows an `approved` status pill and, instead of the approval banner, "Approved — continued in run <id>" where the id selects that run. | — | TC-2817 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2800 | FR-2800 | integration | temp workspace, fake custom agent that sleeps, isolated XDG dirs | start `dmuxai run --json`, read runs dir while the agent sleeps | one `active` manifest with pid, origin `cli_headless`; final JSON `run_id` equals it; `DMUXAI_RUN_ORIGIN=mcp` gives origin `mcp` | `tests/headless_runs_journal.rs`: `tc_2800_active_manifest_before_dispatch` |
| TC-2801 | FR-2801 | integration | fake agents that succeed / fail | run each | statuses `completed` / `failed`, exit codes, duration ≥ 0, masked error summary | `tests/headless_runs_journal.rs`: `tc_2801_terminal_manifest_status_and_exit` |
| TC-2802 | FR-2802 | integration | fake agent printing text and a token-like string | run without `--progress` | `progress.jsonl` exists with progress lines, secret masked; stderr has no progress lines | `tests/headless_runs_journal.rs`: `tc_2802_progress_copy_masked_without_flag` |
| TC-2803 | FR-2803 | integration | XDG_DATA_HOME is a regular file (unwritable) | run fake agent | exit 0, ok true, one journal warning on stderr | `tests/headless_runs_journal.rs`: `tc_2803_journal_failure_does_not_change_outcome` |
| TC-2804 | FR-2804 | unit+integration | event log with and without run id | append and parse | field present only when set; old lines parse; headless run events carry its run_id | `src/events.rs` tests: `tc_2804_event_record_run_id_optional`; `tests/headless_runs_journal.rs`: `tc_2804_headless_events_carry_run_id` |
| TC-2805 | FR-2805 | unit | events file grown in steps, partial line, truncation | call `read_since_from` repeatedly | only new records returned, offsets advance, partial line deferred, truncation restarts | `src/events.rs` tests: `tc_2805_incremental_tail_offsets` |
| TC-2806 | FR-2806 | integration | runs dir fixtures (active with live pid, active with dead pid, completed, malformed) | `GET /api/runs` with token | newest-first order, limit, stale flag, malformed skipped, bad limit 400 | `tests/serve_runs_api.rs`: `tc_2806_runs_list` |
| TC-2807 | FR-2807 | integration | awaiting_approval graph run fixture with graph file and progress lines | `GET /api/runs/{id}` | graph view, approval hint with `--approve-plan-id`, progress tail; 400/404 | `tests/serve_runs_api.rs`: `tc_2807_run_detail_graph_and_approval` |
| TC-2808 | FR-2808 | integration | active run fixture | open SSE, append progress line and event with matching / other run_id, then mark terminal | receives `progress`, matching `state-change` only, `run`, then `end` | `tests/serve_runs_api.rs`: `tc_2808_run_stream_sse` |
| TC-2809 | FR-2809 | integration | server with token; fixture containing a fake secret | requests without token, POST, with token | 401 without token, 405 on POST, masked payload without token string | `tests/serve_runs_api.rs`: `tc_2809_runs_api_read_only_and_masked` |
| TC-2810 | FR-2810, FR-2802 | unit | masking inputs with and without secret-like literals; relaxed manifest save | mask and save | secrets still masked, plain progress lines unchanged; relaxed save roundtrips with 0700/0600 | `src/run_journal.rs`: `tc_2810_mask_prefilter_keeps_masking`; bench numbers in the PR description |
| TC-2811 | FR-2811 | browser | `serve` on fixture workspace | load dashboard (empty transcript) | Runs toggle exists and Runs view is visible by default | `tests/web_runs_view.rs`: `tc_2811_runs_view_default_under_serve`, `tc_2811_runs_default_yields_to_conversation` |
| TC-2812 | FR-2812 | browser | runs fixtures incl. stale | load Runs view | rows show status, origin, runner/model, elapsed, stale badge | `tests/web_runs_view.rs`: `tc_2812_runs_list_rows` |
| TC-2813 | FR-2813 | browser | active run fixture, then progress appended, then awaiting_approval with graph | select run | stream lines appear live; graph nodes; approval banner with hint; final exit shown | `tests/web_runs_view.rs`: `tc_2813_run_detail_live_and_approval` |
| TC-2814 | FR-2814 | browser | fixture with `<img onerror>` in summary | render | no element injected; text shown literally | `tests/web_runs_view.rs`: `tc_2814_run_text_is_not_html` |
| TC-2815 | FR-2815 | integration | fake graph/agent run whose step blocks until the test reads the manifest | read manifest while the run is active | status `active`, `resolved_runner` and `links.graph_id` already set | `tests/headless_runs_journal.rs`: `tc_2815_active_manifest_has_graph_and_runner` |
| TC-2816 | FR-2816 | integration | an `awaiting_approval` manifest with `plan_hash` H (and one with another hash) | headless run with `--approve-plan-id H` | the H manifest becomes `approved` with `approved_by_run_id` = new run id; the other stays `awaiting_approval`; API detail of the approved one has no approval hint | `tests/headless_runs_journal.rs`: `tc_2816_approval_supersedes_awaiting_run`; `tests/run_journal_store.rs`: `tc_2816_store_supersede_awaiting_bounded_scan`; `tests/serve_runs_api.rs`: `tc_2816_approved_run_has_no_hint` |
| TC-2817 | FR-2817 | browser | active run fixture without graph, then manifest gains `graph_id` + runner/model; an `approved` fixture with `approved_by_run_id` | watch detail; select approved run, click the link | graph nodes and runner/model appear without reselecting; approved pill, no approval banner, link selects the continuing run | `tests/web_runs_view.rs`: `tc_2817_live_graph_and_approved_link` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Manifest | adds optional `pid`; statuses gain `awaiting_approval` and `approved`; origin gains `mcp` | PR1a readers ignore unknown fields (serde default) | TC-2800, TC-2801, TC-2816 |
| Events | optional `run_id` on records | omitted when none; old lines parse | TC-2804 |
| API | new GET routes only; token required | no change to existing routes | TC-2806–TC-2809 |
| Security | `mask_sensitive_tokens` on progress copy and error summary; no raw stderr served | — | TC-2802, TC-2809 |
| Performance | incremental tail; relaxed (non-fsync) atomic manifest saves; masking prefilter | #301 10 % rule | TC-2805, TC-2810, PR bench |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Journal wiring + progress copy | `src/run_journal.rs`, `src/cli.rs`, `tests/headless_runs_journal.rs` | FR-2800–FR-2803 | TC-2800–TC-2803 |
| 2 | Event run_id + incremental tail | `src/events.rs`, `src/cli.rs`, `src/web/server.rs` | FR-2804, FR-2805 | TC-2804, TC-2805 |
| 3 | Runs API | `src/web/runs_api.rs`, `src/web/server.rs`, `tests/serve_runs_api.rs` | FR-2806–FR-2809 | TC-2806–TC-2809 |
| 4 | Active manifest metadata sink (graph id, runner, model) | `src/app.rs`, `src/cli.rs`, `tests/headless_runs_journal.rs` | FR-2815 | TC-2815 |
| 5 | Approval supersede of stale awaiting runs | `src/run_journal.rs`, `src/cli.rs`, `src/web/runs_api.rs`, tests | FR-2816 | TC-2816 |
| 6 | Runs view (PR B) | `src/web/assets/runs.js`, `index.html`, `app.css`, `src/web/assets.rs`, `tests/web_runs_view.rs` | FR-2811–FR-2814 | TC-2811–TC-2814 |

## 8. Review and test exit criteria

- [ ] `python3 scripts/check-spec.py --all` and `python3 scripts/check-tc-links.py` pass.
- [ ] `scripts/gate.sh --browser` and `cargo test --test 'web_*' -- --ignored` pass.
- [ ] Bench numbers for FR-2810 recorded in the PR.
