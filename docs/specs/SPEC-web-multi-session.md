# SPEC: Web dashboard for orchestrated headless runs and multi-session panes

Status: draft
Owner: DmuxAI
Created: 2026-11-07
Baseline: `b1ff818`
Related: GitHub issues #147 and #148; SPEC-concurrent-conversations
(FR-1620..FR-1625), SPEC-session-sidebar (FR-860..FR-863),
SPEC-dashboard-parity (FR-780..FR-782), SPEC-web-dashboard (FR-520..FR-530,
FR-527..FR-529), SPEC-web-dashboard-only (FR-1600..FR-1601),
SPEC-multi-workspace-host (FR-2300..FR-2314), SPEC-plan-gate,
SPEC-agent-api (FR-761..FR-767)

## 1. Decision summary

External orchestrators such as Grok Bot drive DmuxAI headlessly through many
parallel `dmuxai -C <workspace> run - --json` processes and through the
`dmuxai mcp` `run` tool, often across several worktrees and workspaces. Today
the web dashboard shows one session at a time, selected from the Sessions
sidebar, and a `serve` dashboard knows only one workspace, so those runs are
invisible or appear only as a run strip while the sidebar says "No
conversations yet" (#147) and the top pill says Idle (#148). This spec makes
every orchestrator-driven run discoverable in one run overview, grouped by
workspace and by an optional orchestration group, with live conversation and
history. It also adds split/grid panes that can pin those runs and conversations
side by side from one shared SSE stream.

## 2. Scope

### Shipped baseline (not a request to redesign)

- `dmuxai serve` is view-only (`snapshot.source == "files"`); `dmuxai --web`
  runs the full App with no TUI and accepts prompts and commands.
- The dashboard currently shows one live session at a time. The Sessions
  sidebar lists conversations for one focused workspace; each pane-like
  surface is really the single session panel.
- A headless `dmuxai run` writes state-change records to
  `<workspace>/.dmuxai/events.jsonl` and graph runs to
  `<workspace>/.dmuxai/graphs/*.json`, but there is no cross-workspace run
  registry and no dashboard-owned record of the invocation itself.
- The top status pill is derived from the current snapshot's agents and turns,
  so a headless run in another process does not make it say Running.
- `dmuxai run` supports `--json`, `--agent`, `--model`, `--graph`,
  `--approve-plan`, `--allow-sensitive`, and plan-gated exit code 7. MCP `run`
  spawns a child `dmuxai run --json`.
- The existing single `/api/events` EventSource and the workspace hub remain
  the only browser stream; there is no per-pane stream.

### In scope

- A shared per-user run registry that every headless invocation updates and
  the dashboard reads across workspaces.
- An overview listing all discovered runs with state, agent, model, workspace,
  start time, duration, caller label, and orchestration group; grouped by
  workspace and by orchestration request; live and historical.
- Optional caller label and correlation/group flags on the CLI and MCP `run`.
- Live read-only conversation, activity, and step view for any selected run,
  including a running headless run under `dmuxai serve` (#147).
- A top status pill that reports live headless runs instead of Idle (#148).
- Distinct presentation of `running`, `done`, `failed`, `awaiting plan
  approval` (exit 7), `cancelled`, and interrupted/stale runs.
- Split/grid panes: one pane by default, add/remove/rearrange, per-pane
  session or run picker, and pinning a run from the overview into a pane.
- One shared SSE connection for all panes, with the existing polling fallback.
- Per-workspace layout persistence in `localStorage`, view-only versus
  interactive panes, performance limits, and accessibility.
- Plan mode as the non-writing run mode; the dashboard must not call such runs
  "read-only".

### Out of scope / non-goals

- Starting, resuming, cancelling, or approving a headless `dmuxai run` from the
  dashboard. The dashboard observes orchestrated runs only.
- Adding a general cross-user or remote multi-tenant run service; one OS user,
  one dashboard token, and the existing Host/Origin checks remain the trust
  boundary.
- Replacing SPEC-multi-workspace-host. A `serve` process still has one focused
  workspace App; cross-workspace runs are observed read-only through the
  registry, not by opening an App for every workspace.
- Running the same agent in two conversations at once. The per-agent and
  per-conversation concurrency rules in SPEC-concurrent-conversations are
  unchanged.
- Persisting pane layout on the server, in the conversation database, in URLs,
  or across browsers/devices.
- TUI pane layout or terminal split behavior.
- Changes to provider protocols, runner event formats, or the meaning of
  existing exit codes.
- Storing prompt text, transcript text, provider credentials, MCP secrets, or
  dashboard tokens in the run registry or pane layout.
- An approval button for plan-gated runs (see Open Questions).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2500 | **Maximum panes.** Proposal: six panes on desktop, with a client constant and server-advertised maximum. Should narrow screens use a lower limit? | DmuxAI | Spec approval |
| OQ-2501 | **Mobile behavior.** Proposal: below 900 px show one pane at a time with a pane switcher instead of shrinking every pane. Confirm or replace. | DmuxAI | Spec approval |
| OQ-2502 | **Cross-workspace registry location and retention.** Proposal: `$DMUXAI_STATE_DIR/runs`, else `$XDG_STATE_HOME/dmuxai/runs`, else `~/.local/state/dmuxai/runs`; keep at most 500 terminal runs or 30 days. Confirm. | DmuxAI | Spec approval |
| OQ-2503 | **Label flag naming.** Proposal: `--label` for a human label and `--session-group` for a correlation id; MCP `label` and `session_group`. Is `--correlation-id` or `--group` clearer? | DmuxAI | Spec approval |
| OQ-2504 | **Interactive input in multiple panes.** Proposal: only the focused pane accepts prompts and commands. Should two panes ever be interactive at once? | DmuxAI | Spec approval |
| OQ-2505 | **Plan approval from the dashboard.** Proposal: not in this spec; the dashboard shows awaiting-approval runs but cannot approve or resume them. Confirm the follow-up scope. | DmuxAI | Spec approval |

No implementation starts while a Must requirement depends on an unanswered
question. FR-2503, FR-2507, FR-2508, FR-2520, FR-2523, and FR-2526 use the
proposals above as their provisional contract until the questions are answered.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Orchestrator | An external process that calls DmuxAI headlessly, e.g. Grok Bot through the CLI or MCP. | A DmuxAI graph planner node |
| Headless run | One `dmuxai run`, `dmuxai todo run`, or MCP `run` invocation and the process it starts. | A dashboard-owned turn in the current App |
| Run record | The registry entry describing one headless run: identity, workspace, conversation, agent, model, label, group, state, timestamps, exit code. | The conversation transcript |
| Run registry | The per-user, cross-workspace directory of run records. | The per-workspace event log or `.dmuxai/graphs` |
| Orchestration group | The optional `session_group` correlation id shared by several runs that belong to one orchestrator task. | A workspace, a conversation, or a graph id |
| Caller label | The optional human-readable `label` supplied by the orchestrator for one run. | The run id |
| Run overview | The dashboard view listing all discovered runs, grouped and filterable. | The Sessions sidebar |
| Pinned run | A run selected into a pane and rendered read-only. | A dashboard-owned interactive turn |
| Pane | One independently rendered dashboard cell with its own selection, header, transcript/run view, and focus state. | The single session panel that exists today |
| Layout | The ordered set of panes for one workspace, including arrangement, count, and each pane's selection. | Server-side conversation storage |
| Focused pane | The one pane that owns keyboard input and the interactive composer. | The server's active conversation |
| View-only pane | A pane that renders but never accepts prompt or command input. | `dmuxai serve` as a whole, which forces every pane to be view-only |
| Shared stream | The single browser `/api/events` EventSource that feeds all panes and the overview. | Per-pane polling or per-pane EventSource |
| Plan mode | The non-writing mode used by runs that must not modify files. | A "read-only" run mode label |

### Headless run state model

A run record is created before the provider is spawned and is updated on every
observable transition. `run_id` is immutable. Terminal states never transition
again for that invocation; a retry or a new orchestrator step gets a new
`run_id`.

| From | Trigger | To | Notes |
| --- | --- | --- | --- |
| absent | Headless invocation accepted and provider about to start | `running` | Start time is recorded before the first provider event |
| `running` | Process exits 0 | `done` | Duration ends at the terminal timestamp |
| `running` | Process exits non-zero other than 7 or 130 | `failed` | Exit code and error kind are recorded |
| `running` | Plan approval or sensitive gate requires a decision (exit 7), or the graph phase is awaiting approval | `awaiting_approval` | Terminal for this invocation; the orchestrator decides what happens next |
| `running` | SIGINT/SIGTERM cancellation or exit 130 | `cancelled` | Not reported as failed |
| `running` | Owner pid/lease is gone without a terminal update | `interrupted` | Recovered on dashboard read or registry sweep; never stays running forever |
| terminal | Retention pruning | removed | Only the record is removed; the workspace conversation and event log are untouched |

Registry read failures, malformed records, and unknown schema versions hide only
the affected record. They must not hide valid runs, crash the dashboard, or
change any run's state.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2500 | Must | Given any headless `dmuxai run`, `dmuxai todo run`, or MCP `run` invocation | A run record is written to the shared per-user run registry before the provider is spawned and updated on every state transition and at termination; the record carries a unique immutable run id, workspace path, conversation id, agent, model, mode, label, session group, state, started/ended timestamps, duration, exit code, last event seq, owner pid, and lease time | A registry write failure is reported on stderr and never fails or cancels the run; a duplicate run id is replaced only by the same invocation's later update, never by another run | TC-2500 |
| FR-2501 | Must | Given a headless run with a provider outcome or signal | Its registry state follows the state model: `running` while live; `done`, `failed`, `awaiting_approval`, or `cancelled` at termination; `interrupted` when the owner disappears without a terminal update | A terminal state never regresses to `running`; exit 7 is `awaiting_approval`, exit 130 is `cancelled`, and a stale owner is not shown as running | TC-2501 |
| FR-2502 | Must | Given the dashboard with a valid token, when the run overview loads or updates | It lists every discoverable run from the current workspace and other workspaces together, with state, agent, model, workspace, start time, live or final duration, label, and session group; runs are grouped by workspace and by orchestration request, and ended runs remain available as history | The list is bounded to the configured maximum (initially 200 records); malformed or inaccessible records are skipped with an inline notice and do not hide other runs; an empty registry shows an explicit empty state | TC-2502, TC-2527 |
| FR-2503 | Must | Given an orchestrator passes `--label <text>` or `--session-group <id>` to `dmuxai run` or `dmuxai todo run`, or passes `label`/`session_group` to the MCP `run` tool | The values are validated, recorded on that run, echoed in the JSON result, and used to group runs; the same session group across different workspaces and sequential steps appears as one orchestration request | Omitted values are null and runs remain individually visible; overlong or control-character values are rejected or normalized before storage; labels never grant access or change grouping across users | TC-2503, TC-2527 |
| FR-2504 | Must | Given a run in the overview, when the user opens it | The dashboard shows that run's conversation transcript, activity, current step, and state, including a run owned by another workspace, and keeps them live while the run is `running` | A pinned or other-workspace run is read-only; an unavailable workspace or database shows an in-pane error without affecting other runs; the dashboard never writes to another workspace's conversation database or event log | TC-2504 |
| FR-2505 | Must | Given a headless run is running in the workspace while the dashboard is `dmuxai serve` or `dmuxai --web` | The run appears as a selectable session/run entry in the Sessions sidebar and in the run overview, and selecting it shows its live run/step and available transcript/activity; the sidebar does not say "No conversations yet" while such a run exists (#147) | When the run ends its entry remains selectable with a terminal state; a malformed run file is ignored without hiding the valid entry; `serve` stays view-only | TC-2505 |
| FR-2506 | Must | Given one or more live headless runs are visible in the current dashboard, when snapshots or registry updates arrive | The top status pill reports `Running` with the running agent or step and elapsed time and never reports `Idle` while any visible run is live (#148); it summarizes multiple live runs without dropping any from the count | When the last visible run ends the pill shows `Done`, `Failed`, `Awaiting approval`, or `Cancelled` as appropriate; a historical run alone does not make it say Running; an unreadable registry leaves the last good text and marks the connection stale instead of falsely showing Idle | TC-2506 |
| FR-2507 | Must | Given the dashboard starts in a workspace and the per-user registry contains runs from other workspaces | Runs from other workspaces are discoverable without switching the `serve` process to those workspaces; the registry is located through `DMUXAI_STATE_DIR`, else `XDG_STATE_HOME`, else the user's local state directory | A missing registry directory is an empty overview, not an error; entries whose workspace no longer exists are listed with an unavailable marker; path traversal and relative paths are rejected | TC-2502, TC-2504 |
| FR-2508 | Must | Given terminal runs exist in the registry, when the dashboard or a serve process restarts | Ended runs are still listed as history, ordered by start time with the newest first, and are distinguishable from live runs | Retention removes only the oldest records beyond the configured count or age; a corrupt or partial record is ignored and the rest of the registry remains readable; a missing index is rebuilt from individual records | TC-2502, TC-2510 |
| FR-2509 | Must | Given a run exits with code 7 or a graph run is in `AwaitingApproval` | The run is shown as `Awaiting plan approval`, visually and textually distinct from `running`, `done`, and `failed`, and the overview keeps it selectable | The dashboard does not auto-approve, resume, or mark it done; a later approval or retry is a separate run; a run that exits 7 is not reported as failed | TC-2507 |
| FR-2510 | Must | Given a run that must not modify workspace files | It is started and recorded in plan mode; the registry and dashboard display `plan` (or `graph-plan` for a graph run), never `read-only` as a run mode | Legacy `ask` or `--read-only` inputs are normalized to `plan` for display and grouping; the dashboard must not invent a write-capable mode for an approval-gated run | TC-2508 |
| FR-2511 | Must | Given any run record | Its start time is the time the invocation started before provider work, its duration is live for `running` runs and final for terminal runs, and its agent and model are the values actually used, including the graph orchestrator model for graph runs | Missing model metadata is shown as unknown rather than guessed; clock skew between hosts is tolerated and never produces a negative duration | TC-2509 |
| FR-2512 | Must | Given the dashboard, run overview, panes, and registry | Authentication and authorization are unchanged: the existing token, Host, and Origin checks protect every run and conversation endpoint; the registry directory is owner-only (0700) and its files are owner-only (0600) and atomically replaced | A missing or invalid token gets the existing unauthorized handling; no token, provider credential, MCP secret, prompt text, or transcript text is stored in the registry, pane layout, pane URLs, or run labels | TC-2510 |
| FR-2513 | Must | Given many live and historical runs, when registry or run state changes | The overview and every pane update through the single shared `/api/events` stream or its existing polling fallback; the page opens no per-run or per-pane EventSource | A slow registry or an unavailable workspace does not block other runs; a run with no new data keeps its last good state and shows a stale indicator rather than clearing | TC-2511 |
| INT-2500 | Must | Given the CLI and MCP run contracts | `dmuxai run` and `dmuxai todo run` accept optional label and session-group inputs, MCP `run` accepts the same as arguments, and the `--json`/MCP result additively includes `run_id`, `label`, `session_group`, and `state` | Existing arguments, exit codes, and JSON fields keep their current meaning; unknown fields are ignored by older consumers | TC-2503 |
| INT-2501 | Must | Given the registry schema | Each record declares a schema version and additive fields; readers ignore unknown fields and older records missing new optional fields | An unsupported newer schema version or malformed JSON hides only that record; no reader migrates or rewrites another process's record | TC-2510 |
| INT-2502 | Must | Given the dashboard HTTP API | Read-only run discovery and run-conversation endpoints serve registry entries and the selected run's transcript tail under the same token, Host, and Origin rules as the existing API; responses are bounded and contain no registry filesystem paths beyond the workspace path needed for display | Unknown run id returns 404; unavailable workspace or busy database returns a bounded error without hanging; no endpoint writes to the run's workspace | TC-2504, TC-2510 |
| BR-2500 | Must | Given a headless invocation and its run record | There is exactly one record per invocation, identified by an immutable run id; a terminal state is final for that id; an orchestration group may span workspaces but never merges two run records into one | A retry, a second orchestrator step, or a resumed approval gets a new run id; duplicate updates for one id are idempotent | TC-2501 |
| NFR-2500 | Must | Given the maximum listed runs, many live runs, and rapid updates | The overview and panes stay responsive: registry reads and rendering are coalesced, the client keeps a bounded run/snapshot buffer, and only the visible or focused work is rendered at full rate | A slow or hidden pane cannot grow memory without bound, block the shared stream, or delay the focused pane's updates; exceeding the listed-run maximum is handled by pagination or an explicit notice | TC-2502, TC-2511, TC-2525 |
| NFR-2501 | Must | Given concurrent headless runs in several workspaces | Registry writes are atomic and safe across processes; readers never observe a partially written record; pruning is safe while runs are being written | A crash during a write leaves either the previous record or the new complete record, never a corrupt file that hides other runs; permission or ownership problems disable the registry with a warning instead of weakening file modes | TC-2500 |
| UX-2500 | Must | Given keyboard-only or assistive-technology use and a first visit | The overview is reachable and operable by keyboard, every run row exposes state, workspace, agent, model, label, and group as text, live updates use `aria-live="polite"`, state is not conveyed by color alone, and the default theme is light | Routine deltas do not use assertive announcements; `prefers-reduced-motion` and the existing `[theme] reduced_motion` setting suppress spinners and transitions | TC-2512 |
| FR-2520 | Must | Given a workspace with the dashboard open, when the user first loads it or adds panes | The dashboard shows one pane by default and can add panes up to the advertised maximum N (initially 6), arranging them in a responsive grid; each pane has a header with its title, agent or run state, and a session/run picker | Adding beyond N is refused with a visible, non-blocking notice and the layout is unchanged; removing the last pane leaves an explicit empty pane rather than a blank page; panes can be reordered by drag and by keyboard move actions | TC-2520 |
| FR-2521 | Must | Given a pane, when the user chooses a conversation or run in its picker or pins a run from the overview | Only that pane switches to the chosen session or run and shows its transcript and live state; other panes keep their selections and continue updating; pinning a run does not start or approve it | An unknown, deleted, inaccessible, or other-workspace-unavailable selection produces an in-pane error with a retry or pick-another action; other panes are unaffected | TC-2521, TC-2527 |
| FR-2522 | Must | Given any number of panes, when snapshots or registry state-change events arrive | Every pane updates from the single shared `/api/events` stream; the page opens no additional EventSource per pane, and each pane applies only data belonging to its selected session or run | If SSE closes or becomes unstable the existing polling fallback feeds all panes; a pane with no matching data keeps its last good state and shows a stale or connection indicator rather than clearing | TC-2522 |
| FR-2523 | Must | Given a dashboard with multiple panes, when the user focuses a pane and types or sends a prompt or command | Only the focused pane accepts input; its composer, Stop, and session commands target that pane's conversation and agent, and unfocused panes render as read-only transcript or run views | In `dmuxai serve` (view-only) every pane is read-only and the existing view-only notices apply; input is never silently sent to a non-focused pane; removing or moving the focused pane transfers focus to a defined neighbor | TC-2523 |
| FR-2524 | Must | Given a pane showing a headless run, a run from another workspace, or a conversation owned by this `--web` process | A pinned headless or other-workspace run is view-only even in `dmuxai --web`; only a focused pane whose conversation is owned by the current App can be interactive; `serve` forces every pane to be view-only | A view-only pane never shows an enabled composer or Stop control, and a focus change does not grant it write access it did not have | TC-2523 |
| FR-2525 | Must | Given the user changes the pane layout, count, order, or per-pane selection | The layout is persisted in `localStorage` under a key scoped to the normalized workspace and restored on reload of the same workspace; another workspace loads its own layout | A missing, corrupt, or obsolete stored layout falls back to one pane; persisted data contains no token, secret, transcript content, or credential, and this feature adds no token to the page URL or history | TC-2524 |
| FR-2526 | Must | Given the maximum number of panes and rapid live updates | The page stays responsive with N panes: rendering is coalesced per animation frame, collapsed or idle panes may reduce rendering work while keeping state, and the client keeps a bounded event/snapshot buffer | Exceeding N panes is refused; a slow or hidden pane cannot grow memory without bound, block the shared stream, or delay the focused pane's updates; idle panes can be collapsed without losing their selection | TC-2525 |
| NFR-2520 | Must | Given the maximum pane count and a high-rate event stream | The focused pane remains responsive and the shared connection stays within the configured bounds for panes, buffered events, and rendered snapshots | Over-limit adds and over-budget buffers are refused or dropped in favor of the focused pane, never by crashing or freezing the page | TC-2525 |
| UX-2520 | Must | Given keyboard-only or assistive-technology use | Panes are reachable and switchable by keyboard (Tab and pane navigation keys), each pane header and picker has an accessible name, focus is visible, and live updates are announced with `aria-live="polite"` | `prefers-reduced-motion` and the existing `[theme] reduced_motion` setting suppress pane spinners and transitions; a pane without a live region does not cause repeated announcements | TC-2526 |
| UX-2521 | Should | Given a narrow viewport | The pane grid becomes one pane at a time with an accessible pane switcher rather than an unusably narrow grid | The layout and each pane's selection survive the switch; the focused pane remains keyboard reachable | TC-2526 |

## 5. Acceptance scenarios and test design

Use deterministic fixtures, a temporary state directory, a temporary workspace
database, fake agents, and the existing event log before any authenticated
external provider. Registry tests must not require a live provider.

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2500 | FR-2500, NFR-2501 | unit + integration | Temporary state dir and workspace; a fake headless invocation and a second writer | Start two runs, update one to terminal, read the registry concurrently, and inspect permissions | One complete record per run; updates are atomic; no partial JSON is read; directory is 0700 and files are 0600; a simulated write failure does not fail the run | `src/run_registry.rs` `tc_2500_registry_is_atomic_and_owner_only`; `tests/web_multi_session.rs` `tc_2500_registry_write_failure_does_not_fail_run` |
| TC-2501 | FR-2501, BR-2500 | unit | Registry records for exit 0, exit 1, exit 7, exit 130, and a dead owner pid | Apply each outcome and then attempt a second transition on a terminal record | States are `done`, `failed`, `awaiting_approval`, `cancelled`, `interrupted`; terminal states do not regress; run ids stay unique | `src/run_registry.rs` `tc_2501_state_mapping_and_terminal_immutability` |
| TC-2502 | FR-2502, FR-2507, FR-2508, NFR-2500 | integration + e2e (ignored, headless Chrome) | Registry with runs from three workspaces, two live and several terminal; one malformed record; more records than the listed maximum | Load the overview, group by workspace and group id, scroll history, and refresh after restart | All valid runs are listed with the required columns; groups are correct; history survives restart; malformed record is skipped with a notice; the list respects the maximum | `tests/web_multi_session.rs` `tc_2502_overview_groups_and_history`; browser `tc_2502_overview_renders` |
| TC-2503 | FR-2503, INT-2500 | unit + integration | CLI parser and MCP tool parser with label and session-group values | Parse valid, omitted, overlong, and control-character values; run two sequential steps with the same group; inspect `--json` and MCP results | Valid values are recorded and echoed; omitted values are null; invalid values are rejected or normalized; both steps share one group across workspaces | `src/cli.rs` `tc_2503_label_and_group_flags`; `src/mcp_server.rs` `tc_2503_mcp_run_label_and_group`; `tests/web_multi_session.rs` `tc_2503_group_spans_workspaces` |
| TC-2504 | FR-2504, FR-2507, INT-2502 | integration | Two workspaces with a live headless run in one and a stored run in the other; read-only API | Fetch the run list, open each run, stream an update, and attempt a write to the other workspace | Each run shows its transcript and step live; other-workspace data is read-only; unknown run id is 404; unavailable workspace returns a bounded error; no file in the other workspace is modified | `tests/web_multi_session.rs` `tc_2504_live_conversation_across_workspaces` |
| TC-2505 | FR-2505 | integration + e2e (ignored, headless Chrome) | `dmuxai serve` over a workspace whose event log and `.dmuxai/graphs` describe a running headless run, plus one stored conversation | Load `/api/state`, open the dashboard, and select the run in the sidebar and overview | The run is listed with its live step and transcript; the sidebar does not say "No conversations yet" (#147); `serve` remains view-only | `tests/web_multi_session.rs` `tc_2505_headless_run_visible_in_serve` |
| TC-2506 | FR-2506 | integration + e2e (ignored, headless Chrome) | `dmuxai serve` with one running headless run and no dashboard-owned turn; then a second live run; then both end | Load the page, add and remove live runs, and inspect the pill after each terminal state | The pill reads `Running` with agent/step and elapsed time, never `Idle` (#148); multiple runs are summarized; terminal states are correct; a historical run alone reads Idle | `tests/web_multi_session.rs` `tc_2506_status_pill_for_headless_runs` |
| TC-2507 | FR-2509 | unit + e2e (ignored, headless Chrome) | Run records from exit 7 and a graph run in `AwaitingApproval`; one failed run beside them | Load the overview and select each run | The approval-gated runs read `Awaiting plan approval` and are not `failed`; the dashboard offers no approve or resume control; the failed run remains distinct | `src/run_registry.rs` `tc_2507_exit_7_is_awaiting_approval`; browser `tc_2507_overview_shows_approval_state` |
| TC-2508 | FR-2510 | unit + asset | Plan-mode run record, legacy `ask` record, and legacy `--read-only` input | Normalize each record for display and inspect the overview renderer | All are displayed as `plan` or `graph-plan`; no run mode is rendered as `read-only`; grouping is unchanged | `src/run_registry.rs` `tc_2508_plan_mode_normalization`; `src/web/assets.rs` `tc_2508_overview_has_no_read_only_mode` |
| TC-2509 | FR-2511 | unit | Run records with known start/end, a live run, a graph run, and a record with missing model | Compute duration and render metadata at two clock readings | Live duration grows; terminal duration is final; graph model is the orchestrator model; missing model reads unknown; duration is never negative | `src/run_registry.rs` `tc_2509_run_timing_and_metadata` |
| TC-2510 | FR-2508, FR-2512, INT-2501 | unit + integration + asset | Registry with a malformed record, an unknown newer schema, and a secret-bearing label attempt; router with token/Host/Origin checks | Read the registry; call run endpoints without and with a token; inspect stored layout and URLs | Malformed and newer records are ignored; auth behavior matches today; registry and layout contain no token, secret, prompt, or transcript text; unknown schema does not crash the reader | `src/run_registry.rs` `tc_2510_schema_and_secret_boundaries`; `tests/web_multi_session.rs` `tc_2510_run_api_auth_unchanged`; `src/web/assets.rs` `tc_2510_layout_has_no_secret_fields` |
| TC-2511 | FR-2513, NFR-2500 | integration + e2e (ignored, headless Chrome) | Two workspaces with live runs; SSE connection; then force the SSE error path | Emit registry and conversation updates for both runs; close SSE; inspect the page | Both update through one stream; the polling fallback takes over; a run with no new data keeps its last good state and shows stale; no per-run EventSource is created | `src/web/assets.rs` `tc_2511_single_shared_event_source`; `tests/web_multi_session.rs` `tc_2511_overview_shared_fallback` |
| TC-2512 | UX-2500 | e2e (ignored, headless Chrome) | Overview with live, done, failed, awaiting, and cancelled runs; OS/browser set to light and reduced motion | Navigate only by keyboard; trigger live deltas; inspect live regions, state text, and theme | Every row and action is reachable with visible focus; states have text, not only color; updates use polite live regions; first visit is light; reduced motion is respected | `tests/web_multi_session.rs` `tc_2512_overview_keyboard_aria_and_light_theme` |
| TC-2520 | FR-2520 | e2e (ignored, headless Chrome) | Dashboard with two stored conversations and a fake live run | Load the dashboard, add panes to the maximum, attempt one more, reorder by drag and keyboard, remove a pane | One pane by default; panes appear in a grid; the over-limit attempt shows a notice and adds nothing; reordering works; removing the last pane leaves an explicit empty pane | `tests/web_multi_session.rs` `tc_2520_pane_grid_default_and_max` |
| TC-2521 | FR-2521 | e2e (ignored, headless Chrome) | Three panes showing different conversations and runs | Change pane 1 to a third conversation; pin a run into pane 2; choose a deleted run in pane 3 | Only the targeted pane switches; other panes keep their selections and keep updating; the deleted run shows an in-pane error; pinning does not start the run | `tests/web_multi_session.rs` `tc_2521_per_pane_selection_and_pin` |
| TC-2522 | FR-2522 | unit (asset) + e2e (ignored) | Embedded `session.js` and `app.js`; dashboard with three panes | Static check for EventSource construction; run snapshots for two sessions and assert each pane | Exactly one EventSource exists in the page; only the pane matching a session updates; no per-pane stream is created | `src/web/assets.rs` `tc_2522_single_shared_event_source`; `tests/web_multi_session.rs` `tc_2522_panes_route_shared_snapshots` |
| TC-2523 | FR-2523, FR-2524 | e2e (ignored, headless Chrome) | `dmuxai --web` with an interactive current-App conversation and a pinned headless run; then `dmuxai serve` | Focus each pane and type or send; inspect composer, Stop, and view-only notices | Only the focused interactive pane sends; the pinned headless pane stays view-only; in `serve` every pane is view-only; Stop targets only the focused conversation; no input goes to a non-focused pane | `tests/web_multi_session.rs` `tc_2523_focused_pane_and_view_only` |
| TC-2524 | FR-2525 | e2e (ignored, headless Chrome) | Workspace A with two panes and a custom selection; workspace B with no saved layout; a corrupt stored value | Reload A; open B; load with the corrupt value; inspect `localStorage` and the URL | A restores its panes, order, and selections; B starts with one pane; corrupt data falls back to one pane; no token, secret, or transcript text is stored | `tests/web_multi_session.rs` `tc_2524_layout_persistence_and_isolation` |
| TC-2525 | FR-2526, NFR-2520 | e2e (ignored, headless Chrome) | Six panes with a high-rate fake event stream; one pane collapsed; an attempt to add a seventh | Stream for 10 seconds while collapsing a pane and resizing the window | The focused pane stays responsive; memory stays bounded; the collapsed pane keeps state and catches up; the seventh pane is refused | `tests/web_multi_session.rs` `tc_2525_pane_backpressure_and_collapse` |
| TC-2526 | UX-2520, UX-2521 | e2e (ignored, headless Chrome) | Dashboard with three panes, a live update stream, a narrow viewport, and reduced motion | Navigate only by keyboard; resize to mobile width; trigger live deltas; inspect live regions and focus | Panes and pickers are keyboard reachable with visible focus; narrow width uses the pane switcher; updates use polite live regions; spinners respect reduced motion | `tests/web_multi_session.rs` `tc_2526_pane_keyboard_aria_and_responsive` |
| TC-2527 | FR-2502, FR-2503, FR-2521 | integration + e2e (ignored, headless Chrome) | Registry with three runs in one session group across two workspaces, plus an ungrouped run; overview and two panes | Group by orchestration request; pin one grouped run into a pane; filter or collapse the group | The three grouped runs appear under one orchestration request while remaining grouped by workspace; the ungrouped run stays separate; pinning shows that run in the chosen pane only | `tests/web_multi_session.rs` `tc_2527_orchestration_group_and_pin` |

Required coverage prompts: TC-2500/TC-2501 cover registry happy path, boundaries,
and terminal transitions; TC-2502/TC-2508 cover cross-workspace discovery,
history, and plan-mode vocabulary; TC-2503 covers labels and grouping; TC-2504
covers live cross-workspace conversation and read-only isolation; TC-2505 and
TC-2506 cover the #147 and #148 regressions; TC-2507 covers approval and
cancellation states; TC-2509/TC-2510 cover timing, schema, and security; TC-2511
covers stream failure and fallback; TC-2512 covers overview accessibility;
TC-2520/TC-2521 cover layout and per-pane selection; TC-2522/TC-2523 cover the
shared stream and input isolation; TC-2524/TC-2525 cover persistence and
performance; TC-2526 covers pane accessibility and narrow viewports; TC-2527
covers orchestration grouping and pinning.

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No conversation-database or server schema change. Add a per-user run registry under the state directory: one owner-only JSON record per headless run, plus optional derived index. Pane layout stays only in browser `localStorage`, keyed by normalized workspace and a layout version. | Removing the registry directory is safe: runs continue, the overview is empty, and the next run recreates the registry. Clearing the layout key falls back to one pane. No migration is required. | TC-2500, TC-2502, TC-2524 |
| Registry schema | Each record has a schema version and additive fields for run id, workspace path, conversation id, agent, model, mode, label, session group, state, timestamps, duration, exit code, last event seq, owner pid, and lease. Records never contain prompt text, transcript text, tokens, provider credentials, or MCP secrets. | Older readers ignore unknown fields; a newer schema version hides only that record. Records are never rewritten by another process except for pruning. | TC-2510 |
| Provider protocol | No change to runner protocols, NDJSON events, graph records, or exit-code meanings. The registry observes the existing headless outcome and event log. | An older run without registry fields is invisible to the overview but unchanged in the workspace. | TC-2501, TC-2509 |
| Cross-workspace reads | The dashboard reads other workspaces' conversation state only through bounded, read-only projections and never writes to their database, event log, graph files, or configuration. | An unavailable or busy workspace returns an error for that run only; the current workspace and other runs stay usable. | TC-2504 |
| Security / permissions | The existing dashboard token, Host, and Origin checks remain mandatory. Registry directory is 0700 and files are 0600, atomically written. No token, secret, prompt, or transcript text enters the registry, pane layout, pane URLs, or labels. | Must not weaken existing auth, cross-workspace isolation, or the view-only `serve` guarantee. A permission problem disables the registry with a warning rather than relaxing modes. | TC-2510 |
| Plan approval and non-writing runs | Exit 7 and graph `AwaitingApproval` are displayed as awaiting approval, not failed and not done. Non-writing runs are displayed as plan mode, never as a "read-only" mode. | Existing CLI/MCP inputs still work; legacy `ask` and `--read-only` are normalized only for display and grouping. | TC-2507, TC-2508 |
| Platform | Linux and other current desktop browsers; no terminal dependency for the overview or panes. | Narrow viewports use the pane switcher fallback; reduced motion follows OS and existing settings. | TC-2526 |

## 7. Implementation plan

Detailed plan: `../plans/PLAN-web-multi-session.md` (to be added before
implementation). The plan must reference requirement and test IDs and must not
introduce new behavior.

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add the per-user run registry: atomic owner-only writes, schema version, pruning, stale-owner recovery, and read-side validation | `src/run_registry.rs` (new), `src/state_path.rs`, `src/runtime_env.rs` | FR-2500, FR-2501, FR-2507, FR-2508, BR-2500, INT-2501, NFR-2501 | TC-2500, TC-2501, TC-2510 |
| 2 | Record runs from `run`, `todo run`, and MCP `run`; add label and session-group flags/args and additive JSON fields; map exit 7, 130, and plan mode | `src/cli.rs`, `src/mcp_server.rs`, `src/agent_api.rs`, `src/run_registry.rs` | FR-2500, FR-2501, FR-2503, FR-2509, FR-2510, FR-2511, INT-2500 | TC-2501, TC-2503, TC-2507, TC-2508, TC-2509 |
| 3 | Expose read-only run discovery and run-conversation endpoints under the existing auth and add registry data to snapshots/events | `src/web/server.rs`, `src/web/snapshot.rs`, `src/web/file_source.rs`, `src/web/hub.rs` | FR-2502, FR-2504, FR-2507, FR-2512, FR-2513, INT-2502 | TC-2502, TC-2504, TC-2510, TC-2511 |
| 4 | Add the run overview UI, grouping, state badges, history, approval state, and the #147 sidebar entry | `src/web/assets/app.js`, `src/web/assets/session.js`, `src/web/assets/session.css`, `src/web/assets/index.html` | FR-2502, FR-2505, FR-2508, FR-2509, FR-2511, UX-2500 | TC-2502, TC-2505, TC-2507, TC-2509, TC-2512, TC-2527 |
| 5 | Fix the status pill to include headless and multiple live runs (#148) | `src/web/assets/app.js` | FR-2506 | TC-2506 |
| 6 | Add pane/layout state, per-workspace `localStorage` persistence, add/remove/rearrange, and per-pane picker/pin | `src/web/assets/session.js`, `src/web/assets/app.css`, `src/web/assets/index.html` | FR-2520, FR-2521, FR-2525 | TC-2520, TC-2521, TC-2524, TC-2527 |
| 7 | Route the shared snapshot and registry stream to panes; enforce one EventSource, bounded buffers, and collapse/backpressure | `src/web/assets/app.js`, `src/web/assets/session.js` | FR-2513, FR-2522, FR-2526, NFR-2500, NFR-2520 | TC-2511, TC-2522, TC-2525 |
| 8 | Enforce focused-pane input and view-only pane rules, including `serve` | `src/web/assets/session.js`, `src/web/server.rs` | FR-2523, FR-2524 | TC-2523 |
| 9 | Keyboard navigation, live regions, light-theme default, reduced motion, and narrow-viewport fallback | `src/web/assets/session.js`, `src/web/assets/app.css`, `src/web/assets/index.html` | UX-2500, UX-2520, UX-2521 | TC-2512, TC-2526 |
| 10 | Add deterministic registry, API, asset, and browser tests; update traceability and review notes | `tests/web_multi_session.rs`, `src/web/assets.rs`, `src/run_registry.rs`, `src/cli.rs`, `src/mcp_server.rs` | All Must requirements | TC-2500..TC-2527 |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals, including #147 and #148.
- [ ] Required Rust checks pass: `cargo fmt --check`, `cargo test --all-targets`, and
      `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] `bash scripts/check-spec.sh docs/specs/SPEC-web-multi-session.md` passes.
- [ ] Browser tests run with `scripts/gate.sh --browser` (or the focused ignored
      `tests/web_multi_session.rs` cases) and light-theme screenshots are attached.
- [ ] External-provider smoke tests are marked run, skipped, or blocked with a
      reason; skipped is never reported as passing.
- [ ] `../reviews/REVIEW-web-multi-session.md` records commands, results,
      residual risk, and rollback notes, including the unresolved
      OQ-2500..OQ-2505 decisions.

## 9. Open questions

1. **Maximum panes (OQ-2500).** Is six panes the right desktop maximum, and
   should narrow screens use a lower limit? The provisional contract is six,
   advertised by the client and enforced on add.
2. **Mobile behavior (OQ-2501).** Should below 900 px switch to one pane at a
   time with a pane switcher instead of shrinking every pane? The provisional
   contract is the switcher.
3. **Cross-workspace registry location and retention (OQ-2502).** Is
   `$DMUXAI_STATE_DIR/runs`, else `$XDG_STATE_HOME/dmuxai/runs`, else
   `~/.local/state/dmuxai/runs` correct, and is 500 terminal runs or 30 days the
   right retention bound?
4. **Label flag naming (OQ-2503).** Are `--label` and `--session-group` the
   right names, or should the correlation id be `--correlation-id` or `--group`?
   The MCP arguments follow the chosen names.
5. **Interactive input in multiple panes (OQ-2504).** Should only the focused
   pane ever accept input, or may two panes be interactive at once? The
   provisional contract is focused-pane-only.
6. **Plan approval from the dashboard (OQ-2505).** Should a later spec let the
   dashboard approve or resume an awaiting-approval run? This spec only displays
   that state and never approves a run.
