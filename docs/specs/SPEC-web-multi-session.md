# SPEC: Web dashboard for orchestrated headless runs and multi-session panes

Status: draft
Owner: DmuxAI
Created: 2026-11-07
Baseline: `b1ff818`
Related: GitHub issues #147, #148, and #202; SPEC-concurrent-conversations
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
side by side from one shared SSE stream. Panes are capped at nine on desktop;
narrow screens show one pane at a time with an accessible switcher. Run history
is retained indefinitely and is removed only by an explicit `dmuxai runs prune`
command. Orchestrators label runs with the final `--label` and `--session-group`
inputs (MCP `label` and `session_group`). The dashboard may also approve or
reject a plan-gated run and request a resume, under the security and
process-ownership contract in section 6.

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
- Split/grid panes: one pane by default, up to nine on desktop, add/remove/
  rearrange, per-pane session or run picker, and pinning a run from the overview
  into a pane.
- One shared SSE connection for all panes, with the existing polling fallback.
- Per-workspace layout persistence in `localStorage`, view-only versus
  interactive panes, performance limits, and accessibility.
- A narrow-viewport mode that shows one pane at a time with an accessible pane
  switcher.
- Plan approval and resume from the dashboard for runs discovered through the
  registry, including runs started by another process such as Grok Bot.
- Manual run-history pruning through `dmuxai runs prune`; no automatic
  count- or age-based retention.
- Plan mode as the non-writing run mode; the dashboard must not call such runs
  "read-only".

### Out of scope / non-goals

- Starting a new arbitrary run, cancelling a run, or sending prompts to a
  headless run from the dashboard. The only run control in scope is approving or
  rejecting a plan-gated run and requesting a resume under FR-2540..FR-2547.
- Approving sensitive steps (`--allow-sensitive`) from the web. Web approval
  never implies `--allow-sensitive`; a future separate explicit confirmation for
  sensitive steps is out of scope.
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

### Resolved decisions and remaining questions

OQ-2500 (maximum panes = nine), OQ-2501 (narrow screens use one pane with a
switcher), OQ-2502 (registry location; history kept forever and pruned only
manually), OQ-2503 (`--label` and `--session-group` are final), and OQ-2505
(dashboard plan approval and resume are in scope) are resolved and folded into
sections 2, 3, 4, and 6. OQ-2504 remains open; see section 9 along with the new
approval/resume questions.

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
| Plan hash | A stable digest of the exact plan (steps, runners/models, and mode) that was shown for approval. | The run id |
| Approval record | An atomic registry file keyed by run id and plan hash that records an approve or reject decision, the dashboard session, the timestamp, and the workspace. | The run record itself |
| Resumed run | A new run that continues the conversation of an approved run, linked by `resumed_from` and sharing the original session group. | A retry with a new unrelated run id |
| Audit event | An append-only record of an approval or rejection written to the run registry and the workspace event log. | A provider event or a transcript message |

### Headless run state model

A run record is created before the provider is spawned and is updated on every
observable transition. `run_id` is immutable. Terminal states never transition
again for that invocation; a retry, a resumed approval, or a new orchestrator
step gets a new `run_id`.

| From | Trigger | To | Notes |
| --- | --- | --- | --- |
| absent | Headless invocation accepted and provider about to start | `running` | Start time is recorded before the first provider event |
| `running` | Process exits 0 | `done` | Duration ends at the terminal timestamp |
| `running` | Process exits non-zero other than 7 or 130 | `failed` | Exit code and error kind are recorded |
| `running` | Plan approval or sensitive gate requires a decision (exit 7), or the graph phase is awaiting approval | `awaiting_approval` | Not terminal; an approval or rejection decision may follow |
| `awaiting_approval` | A valid, single-use approval record is accepted | `approved` | Approval semantics match CLI `--approve-plan` and never grant `--allow-sensitive` |
| `approved` | A resumed run is accepted and started | `resumed` | The original run links to the new run id via `resumed_from`; the new run has its own record and state |
| `awaiting_approval` | A rejection is recorded | `rejected` | Terminal for this invocation; no resume is started |
| `running` | SIGINT/SIGTERM cancellation or exit 130 | `cancelled` | Not reported as failed |
| `running` | Owner pid/lease is gone without a terminal update | `interrupted` | Recovered on dashboard read or registry sweep; never stays running forever |
| terminal or `awaiting_approval` | Explicit `dmuxai runs prune` | removed | Only the record and any approval records are removed; the workspace conversation and event log are untouched |

`done`, `failed`, `cancelled`, `interrupted`, `resumed`, and `rejected` are
terminal for the invocation. `approved` is a decision state that may only move to
`resumed`. An approval record for a stale plan hash never changes run state.

Registry read failures, malformed records, and unknown schema versions hide only
the affected record. They must not hide valid runs, crash the dashboard, or
change any run's state.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2500 | Must | Given any headless `dmuxai run`, `dmuxai todo run`, or MCP `run` invocation | A run record is written to the shared per-user run registry before the provider is spawned and updated on every state transition and at termination; the record carries a unique immutable run id, workspace path, conversation id, agent, model, mode, label, session group, state, started/ended timestamps, duration, exit code, last event seq, owner pid, and lease time | A registry write failure is reported on stderr and never fails or cancels the run; a duplicate run id is replaced only by the same invocation's later update, never by another run | TC-2500 |
| FR-2501 | Must | Given a headless run with a provider outcome or signal | Its registry state follows the state model: `running` while live; `done`, `failed`, `awaiting_approval`, or `cancelled` at termination; `interrupted` when the owner disappears without a terminal update; plan-gated runs may additionally move through `approved`, `resumed`, and `rejected` | A terminal state never regresses to `running`; exit 7 is `awaiting_approval`, exit 130 is `cancelled`, and a stale owner is not shown as running | TC-2501, TC-2541 |
| FR-2502 | Must | Given the dashboard with a valid token, when the run overview loads or updates | It lists every discoverable run from the current workspace and other workspaces together, with state, agent, model, workspace, start time, live or final duration, label, and session group; runs are grouped by workspace and by orchestration request, and ended runs remain available as history | The overview is paginated and each page is bounded (initially 200 records); malformed or inaccessible records are skipped with an inline notice and do not hide other runs; an empty registry shows an explicit empty state | TC-2502, TC-2527 |
| FR-2503 | Must | Given an orchestrator passes `--label <text>` or `--session-group <id>` (the final names) to `dmuxai run` or `dmuxai todo run`, or passes `label`/`session_group` to the MCP `run` tool | The values are validated, recorded on that run, echoed in the JSON result, and used to group runs; the same session group across different workspaces and sequential steps appears as one orchestration request | Omitted values are null and runs remain individually visible; overlong or control-character values are rejected or normalized before storage; labels never grant access or change grouping across users | TC-2503, TC-2527 |
| FR-2504 | Must | Given a run in the overview, when the user opens it | The dashboard shows that run's conversation transcript, activity, current step, and state, including a run owned by another workspace, and keeps them live while the run is `running` | A pinned or other-workspace run is read-only; an unavailable workspace or database shows an in-pane error without affecting other runs; the dashboard never writes to another workspace's conversation database or event log | TC-2504 |
| FR-2505 | Must | Given a headless run is running in the workspace while the dashboard is `dmuxai serve` or `dmuxai --web` | The run appears as a selectable session/run entry in the Sessions sidebar and in the run overview, and selecting it shows its live run/step and available transcript/activity; the sidebar does not say "No conversations yet" while such a run exists (#147) | When the run ends its entry remains selectable with a terminal state; a malformed run file is ignored without hiding the valid entry; `serve` stays view-only | TC-2505 |
| FR-2506 | Must | Given one or more live headless runs are visible in the current dashboard, when snapshots or registry updates arrive | The top status pill reports `Running` with the running agent or step and elapsed time and never reports `Idle` while any visible run is live (#148); it summarizes multiple live runs without dropping any from the count | When the last visible run ends the pill shows `Done`, `Failed`, `Awaiting approval`, or `Cancelled` as appropriate; a historical run alone does not make it say Running; an unreadable registry leaves the last good text and marks the connection stale instead of falsely showing Idle | TC-2506 |
| FR-2507 | Must | Given the dashboard starts in a workspace and the per-user registry contains runs from other workspaces | Runs from other workspaces are discoverable without switching the `serve` process to those workspaces; the registry is located through `DMUXAI_STATE_DIR`, else `XDG_STATE_HOME`, else the user's local state directory | A missing registry directory is an empty overview, not an error; entries whose workspace no longer exists are listed with an unavailable marker; path traversal and relative paths are rejected | TC-2502, TC-2504 |
| FR-2508 | Must | Given terminal runs exist in the registry, when the dashboard or a serve process restarts | Ended runs are still listed as history, ordered by start time with the newest first, and are distinguishable from live runs; history is retained indefinitely with no automatic count- or age-based pruning | A corrupt or partial record is ignored and the rest of the registry remains readable; a missing index is rebuilt from individual records; records are removed only by the explicit manual prune command in FR-2514 | TC-2502, TC-2510, TC-2513 |
| FR-2509 | Must | Given a run exits with code 7 or a graph run is in `AwaitingApproval` | The run is shown as `Awaiting plan approval`, visually and textually distinct from `running`, `done`, and `failed`, and the overview keeps it selectable | The dashboard never auto-approves or auto-resumes; it acts only on an explicit confirmed user action under FR-2540..FR-2547; a later approval or retry is a separate run; a run that exits 7 is not reported as failed | TC-2507, TC-2540 |
| FR-2510 | Must | Given a run that must not modify workspace files | It is started and recorded in plan mode; the registry and dashboard display `plan` (or `graph-plan` for a graph run), never `read-only` as a run mode | Legacy `ask` or `--read-only` inputs are normalized to `plan` for display and grouping; the dashboard must not invent a write-capable mode for an approval-gated run | TC-2508 |
| FR-2511 | Must | Given any run record | Its start time is the time the invocation started before provider work, its duration is live for `running` runs and final for terminal runs, and its agent and model are the values actually used, including the graph orchestrator model for graph runs | Missing model metadata is shown as unknown rather than guessed; clock skew between hosts is tolerated and never produces a negative duration | TC-2509 |
| FR-2512 | Must | Given the dashboard, run overview, panes, and registry | Authentication and authorization are unchanged: the existing token, Host, and Origin checks protect every run and conversation endpoint; the registry directory is owner-only (0700) and its files are owner-only (0600) and atomically replaced | A missing or invalid token gets the existing unauthorized handling; no token, provider credential, MCP secret, prompt text, or transcript text is stored in the registry, pane layout, pane URLs, or run labels | TC-2510 |
| FR-2513 | Must | Given many live and historical runs, when registry or run state changes | The overview and every pane update through the single shared `/api/events` stream or its existing polling fallback; the page opens no per-run or per-pane EventSource | A slow registry or an unavailable workspace does not block other runs; a run with no new data keeps its last good state and shows a stale indicator rather than clearing | TC-2511 |
| FR-2514 | Must | Given an operator wants to remove run history, when they invoke `dmuxai runs prune --before <date>` or `dmuxai runs prune --keep <n>`, optionally with `--dry-run` | The command removes matching terminal run records and their approval records from the registry, reports how many records would be or were removed, and never removes live or `awaiting_approval` runs; `--keep <n>` retains the n newest terminal records and removes older ones | `--before` and `--keep` are mutually exclusive; an invalid date, non-numeric count, missing selector, or unreadable registry exits non-zero without deleting anything; `--dry-run` never deletes; the workspace conversation and event log are never modified | TC-2513 |
| INT-2500 | Must | Given the CLI and MCP run contracts | `dmuxai run` and `dmuxai todo run` accept the final optional `--label` and `--session-group` inputs, MCP `run` accepts `label` and `session_group`, and the `--json`/MCP result additively includes `run_id`, `label`, `session_group`, and `state` | Existing arguments, exit codes, and JSON fields keep their current meaning; unknown fields are ignored by older consumers | TC-2503 |
| INT-2501 | Must | Given the registry schema | Each record declares a schema version and additive fields; readers ignore unknown fields and older records missing new optional fields | An unsupported newer schema version or malformed JSON hides only that record; no reader migrates or rewrites another process's record | TC-2510 |
| INT-2502 | Must | Given the dashboard HTTP API | Read-only run discovery and run-conversation endpoints serve registry entries and the selected run's transcript tail under the same token, Host, and Origin rules as the existing API; responses are bounded and contain no registry filesystem paths beyond the workspace path needed for display | Unknown run id returns 404; unavailable workspace or busy database returns a bounded error without hanging; no endpoint writes to the run's workspace | TC-2504, TC-2510 |
| BR-2500 | Must | Given a headless invocation and its run record | There is exactly one record per invocation, identified by an immutable run id; a terminal state is final for that id; an orchestration group may span workspaces but never merges two run records into one | A retry, a second orchestrator step, or a resumed approval gets a new run id; duplicate updates for one id are idempotent | TC-2501 |
| NFR-2500 | Must | Given the maximum listed runs, many live runs, and rapid updates | The overview and panes stay responsive: registry reads and rendering are coalesced, the client keeps a bounded run/snapshot buffer, and only the visible or focused work is rendered at full rate | A slow or hidden pane cannot grow memory without bound, block the shared stream, or delay the focused pane's updates; exceeding the listed-run maximum is handled by pagination or an explicit notice | TC-2502, TC-2511, TC-2525 |
| NFR-2501 | Must | Given concurrent headless runs in several workspaces | Registry writes are atomic and safe across processes; readers never observe a partially written record; pruning is safe while runs are being written | A crash during a write leaves either the previous record or the new complete record, never a corrupt file that hides other runs; permission or ownership problems disable the registry with a warning instead of weakening file modes | TC-2500 |
| NFR-2502 | Must | Given history is retained forever and runs keep accumulating | Each run record is bounded in size (no prompt or transcript text), the overview paginates instead of loading the whole registry, and the dashboard may show a non-blocking warning when the registry exceeds a configured size or record count | The warning never deletes data or blocks the overview; an oversized or unreadable registry still serves bounded pages and keeps valid records readable; disk growth is remediated only by the explicit prune command in FR-2514 | TC-2513 |
| UX-2500 | Must | Given keyboard-only or assistive-technology use and a first visit | The overview is reachable and operable by keyboard, every run row exposes state, workspace, agent, model, label, and group as text, live updates use `aria-live="polite"`, state is not conveyed by color alone, and the default theme is light | Routine deltas do not use assertive announcements; `prefers-reduced-motion` and the existing `[theme] reduced_motion` setting suppress spinners and transitions | TC-2512 |
| FR-2520 | Must | Given a workspace with the dashboard open, when the user first loads it or adds panes | The dashboard shows one pane by default and can add panes up to the advertised maximum N (initially 9), arranging them in a responsive grid; each pane has a header with its title, agent or run state, and a session/run picker | Adding beyond N is refused with a visible, non-blocking notice and the layout is unchanged; removing the last pane leaves an explicit empty pane rather than a blank page; panes can be reordered by drag and by keyboard move actions | TC-2520 |
| FR-2521 | Must | Given a pane, when the user chooses a conversation or run in its picker or pins a run from the overview | Only that pane switches to the chosen session or run and shows its transcript and live state; other panes keep their selections and continue updating; pinning a run does not start or approve it | An unknown, deleted, inaccessible, or other-workspace-unavailable selection produces an in-pane error with a retry or pick-another action; other panes are unaffected | TC-2521, TC-2527 |
| FR-2522 | Must | Given any number of panes, when snapshots or registry state-change events arrive | Every pane updates from the single shared `/api/events` stream; the page opens no additional EventSource per pane, and each pane applies only data belonging to its selected session or run | If SSE closes or becomes unstable the existing polling fallback feeds all panes; a pane with no matching data keeps its last good state and shows a stale or connection indicator rather than clearing | TC-2522 |
| FR-2523 | Must | Given a dashboard with multiple panes, when the user focuses a pane and types or sends a prompt or command | Only the focused pane accepts input; its composer, Stop, and session commands target that pane's conversation and agent, and unfocused panes render as read-only transcript or run views | In `dmuxai serve` (view-only) every pane is read-only and the existing view-only notices apply; input is never silently sent to a non-focused pane; removing or moving the focused pane transfers focus to a defined neighbor | TC-2523 |
| FR-2524 | Must | Given a pane showing a headless run, a run from another workspace, or a conversation owned by this `--web` process | A pinned headless or other-workspace run is view-only even in `dmuxai --web`; only a focused pane whose conversation is owned by the current App can be interactive; `serve` forces every pane to be view-only | A view-only pane never shows an enabled composer or Stop control, and a focus change does not grant it write access it did not have | TC-2523 |
| FR-2525 | Must | Given the user changes the pane layout, count, order, or per-pane selection | The layout is persisted in `localStorage` under a key scoped to the normalized workspace and restored on reload of the same workspace; another workspace loads its own layout | A missing, corrupt, or obsolete stored layout falls back to one pane; persisted data contains no token, secret, transcript content, or credential, and this feature adds no token to the page URL or history | TC-2524 |
| FR-2526 | Must | Given the maximum number of panes and rapid live updates | The page stays responsive with N panes: rendering is coalesced per animation frame, collapsed or idle panes may reduce rendering work while keeping state, and the client keeps a bounded event/snapshot buffer | Exceeding N panes is refused; a slow or hidden pane cannot grow memory without bound, block the shared stream, or delay the focused pane's updates; idle panes can be collapsed without losing their selection | TC-2525 |
| NFR-2520 | Must | Given the maximum pane count of nine and a high-rate event stream | The focused pane remains responsive and the shared connection stays within the configured bounds for panes, buffered events, and rendered snapshots | Over-limit adds and over-budget buffers are refused or dropped in favor of the focused pane, never by crashing or freezing the page | TC-2525 |
| UX-2520 | Must | Given keyboard-only or assistive-technology use | Panes are reachable and switchable by keyboard (Tab and pane navigation keys), each pane header and picker has an accessible name, focus is visible, and live updates are announced with `aria-live="polite"` | `prefers-reduced-motion` and the existing `[theme] reduced_motion` setting suppress pane spinners and transitions; a pane without a live region does not cause repeated announcements | TC-2526 |
| UX-2521 | Must | Given a narrow viewport | The pane grid becomes one pane at a time with an accessible pane switcher rather than an unusably narrow grid | The layout and each pane's selection survive the switch; the focused pane remains keyboard reachable; the switcher exposes every pane by name and state | TC-2526 |
| FR-2540 | Must | Given an `awaiting_approval` run and a user with a valid dashboard token, when they choose approve or reject | The action requires an explicit click in a confirmation dialog that shows the full plan (steps, runners/models, and mode); the decision is sent by a POST-only endpoint protected by the existing token, CSRF protection, and Host/Origin checks, and carries a per-plan nonce and plan hash | A GET or other non-POST request is rejected; missing or invalid token, CSRF, Host, or Origin gets the existing unauthorized handling; a stale, missing, or changed plan hash is rejected with a conflict and changes no state; dismissing the dialog sends nothing | TC-2540 |
| FR-2541 | Must | Given a confirmed approval or rejection | An audit event is recorded for every decision with the dashboard session, timestamp, run id, plan hash, workspace, and decision, both in the run registry and in the workspace event log | If the mandatory registry audit cannot be written the decision is not applied and the error is shown; a workspace event-log failure is surfaced and does not silently drop the registry audit; audit records contain no prompt or transcript text | TC-2541 |
| FR-2542 | Must | Given a web approval of a plan-gated run | Its approval semantics are identical to CLI `--approve-plan`; web approval never implies `--allow-sensitive` and never grants sensitive-step execution | A plan that requires sensitive steps stays blocked; the web action does not set or imply `--allow-sensitive`; approving sensitive steps from the web is a non-goal and no control offers it | TC-2540, TC-2542 |
| FR-2543 | Must | Given an `awaiting_approval` run started by another process, such as Grok Bot, and discovered through the registry | The dashboard writes an atomic approval record in the registry keyed by run id and plan hash; the run record moves to `approved` or `rejected`, and the original orchestrator observes the outcome through the registry state | A run that exited with code 7 is not waiting in-process, so the dashboard never blocks on the original process; a missing, malformed, or stale approval record changes no run state; concurrent readers never observe a partial approval record | TC-2542 |
| FR-2544 | Must | Given an approved run, when a resume is requested | Resume starts a new run that continues the same conversation with approval, gets a new immutable run id, is linked to the original through `resumed_from`, and shares the original session group | Continuing the conversation depends on GitHub issue #202; if #202 is unavailable, resume re-runs the original prompt with approval and records in the new run that it is not a true continuation; the original prompt is read from the workspace conversation and never stored in the registry; if the conversation cannot be loaded, resume fails without creating a run or mutating the original | TC-2543 |
| FR-2545 | Must | Given an approved run, when deciding which process executes the resumed run | The dashboard host spawns the resumed run in the original workspace only in `dmuxai --web` mode or when spawning is explicitly enabled; view-only `dmuxai serve` may record the approval but must not spawn; an orchestrator may poll the approval record and resume the run itself | `serve` never spawns a resumed run; an unavailable or busy workspace, a missing runner, or a spawn failure is recorded and surfaced without changing the approval decision; the original orchestrator sees the resulting run state through the registry | TC-2543 |
| FR-2546 | Must | Given an `awaiting_approval` run, when a rejection is confirmed | The rejection is recorded and the run moves from `awaiting_approval` to `rejected`; no resume is started | A rejected run is terminal for that invocation and is not shown as approved, resumed, failed, or still awaiting; a duplicate rejection is idempotent and produces no second transition | TC-2541 |
| FR-2547 | Must | Given an approval record for a run, when one or more approvals arrive, including concurrently | Each approval is single-use and idempotent: the first valid approval wins, later duplicate approvals are no-ops, and concurrent approvals of the same run produce exactly one resumed run | Two different plan hashes for the same run cannot both be approved; an approval for a plan hash that no longer matches the run is rejected; exactly one `resumed_from` run is created per approved plan | TC-2542 |
| INT-2503 | Must | Given the `dmuxai runs prune` command | It accepts exactly one of `--before <date>` or `--keep <n>`, plus optional `--dry-run`, uses the same registry location as the readers, and reports a bounded summary without requiring a dashboard token | Unknown arguments, both selectors at once, or neither selector exit non-zero with usage text; `--dry-run` performs no writes; exit codes follow the existing CLI conventions | TC-2513 |
| INT-2540 | Must | Given the approval/resume IPC | The approval record is an atomic, owner-only file in the registry keyed by run id and plan hash with a schema version and additive fields for decision, dashboard session, timestamp, workspace, nonce, and optional resume linkage; run records additively carry `resumed_from` and the new state values | Older readers ignore unknown fields; a malformed or newer-schema approval record hides only that decision; no process rewrites another process's approval record except the single-use claim and explicit prune | TC-2542, TC-2543 |
| INT-2541 | Must | Given the dashboard approval, rejection, and resume endpoints | They are POST-only, use the existing token, CSRF, Host, and Origin rules, accept a plan hash and nonce, and return bounded JSON; they never expose registry filesystem paths beyond the workspace path needed for display | Missing or stale plan hash returns a conflict; unknown run id returns 404; a view-only `serve` may record a decision but a resume request that would spawn is refused with an explicit error; no endpoint writes prompt or transcript text into the registry | TC-2540, TC-2543 |
| UX-2540 | Must | Given keyboard-only or assistive-technology use, when the user opens the plan confirmation dialog | The dialog shows the full plan (steps, runners/models, and mode), has an accessible name, traps focus, exposes explicit approve, reject, and cancel actions, and announces the resulting state; `approved`, `resumed`, and `rejected` are conveyed by text and not by color alone | Escape or cancel dismisses without sending; focus returns to the invoking control; a stale-plan error is announced and offers a refresh; reduced motion suppresses dialog transitions | TC-2544 |

## 5. Acceptance scenarios and test design

Use deterministic fixtures, a temporary state directory, a temporary workspace
database, fake agents, and the existing event log before any authenticated
external provider. Registry tests must not require a live provider.

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2500 | FR-2500, NFR-2501 | unit + integration | Temporary state dir and workspace; a fake headless invocation and a second writer | Start two runs, update one to terminal, read the registry concurrently, and inspect permissions | One complete record per run; updates are atomic; no partial JSON is read; directory is 0700 and files are 0600; a simulated write failure does not fail the run | `src/run_registry.rs` `tc_2500_registry_is_atomic_and_owner_only`; `tests/web_multi_session.rs` `tc_2500_registry_write_failure_does_not_fail_run` |
| TC-2501 | FR-2501, BR-2500 | unit | Registry records for exit 0, exit 1, exit 7, exit 130, and a dead owner pid | Apply each outcome and then attempt a second transition on a terminal record | States are `done`, `failed`, `awaiting_approval`, `cancelled`, `interrupted`; terminal states do not regress; run ids stay unique | `src/run_registry.rs` `tc_2501_state_mapping_and_terminal_immutability` |
| TC-2502 | FR-2502, FR-2507, FR-2508, NFR-2500 | integration + e2e (ignored, headless Chrome) | Registry with runs from three workspaces, two live and several terminal; one malformed record; more records than one overview page | Load the overview, group by workspace and group id, scroll history, paginate, and refresh after restart | All valid runs are listed with the required columns; groups are correct; history survives restart with no automatic pruning; malformed record is skipped with a notice; each page respects the bound and pagination reaches the rest | `tests/web_multi_session.rs` `tc_2502_overview_groups_and_history`; browser `tc_2502_overview_renders` |
| TC-2503 | FR-2503, INT-2500 | unit + integration | CLI parser and MCP tool parser with label and session-group values | Parse valid, omitted, overlong, and control-character values; run two sequential steps with the same group; inspect `--json` and MCP results | Valid values are recorded and echoed; omitted values are null; invalid values are rejected or normalized; both steps share one group across workspaces | `src/cli.rs` `tc_2503_label_and_group_flags`; `src/mcp_server.rs` `tc_2503_mcp_run_label_and_group`; `tests/web_multi_session.rs` `tc_2503_group_spans_workspaces` |
| TC-2504 | FR-2504, FR-2507, INT-2502 | integration | Two workspaces with a live headless run in one and a stored run in the other; read-only API | Fetch the run list, open each run, stream an update, and attempt a write to the other workspace | Each run shows its transcript and step live; other-workspace data is read-only; unknown run id is 404; unavailable workspace returns a bounded error; no file in the other workspace is modified | `tests/web_multi_session.rs` `tc_2504_live_conversation_across_workspaces` |
| TC-2505 | FR-2505 | integration + e2e (ignored, headless Chrome) | `dmuxai serve` over a workspace whose event log and `.dmuxai/graphs` describe a running headless run, plus one stored conversation | Load `/api/state`, open the dashboard, and select the run in the sidebar and overview | The run is listed with its live step and transcript; the sidebar does not say "No conversations yet" (#147); `serve` remains view-only | `tests/web_multi_session.rs` `tc_2505_headless_run_visible_in_serve` |
| TC-2506 | FR-2506 | integration + e2e (ignored, headless Chrome) | `dmuxai serve` with one running headless run and no dashboard-owned turn; then a second live run; then both end | Load the page, add and remove live runs, and inspect the pill after each terminal state | The pill reads `Running` with agent/step and elapsed time, never `Idle` (#148); multiple runs are summarized; terminal states are correct; a historical run alone reads Idle | `tests/web_multi_session.rs` `tc_2506_status_pill_for_headless_runs` |
| TC-2507 | FR-2509, FR-2540 | unit + e2e (ignored, headless Chrome) | Run records from exit 7 and a graph run in `AwaitingApproval`; one failed run beside them | Load the overview and select each run | The approval-gated runs read `Awaiting plan approval` and are not `failed`; the dashboard shows approve and reject only behind the confirmation dialog and never auto-approves; the failed run remains distinct | `src/run_registry.rs` `tc_2507_exit_7_is_awaiting_approval`; browser `tc_2507_overview_shows_approval_state` |
| TC-2508 | FR-2510 | unit + asset | Plan-mode run record, legacy `ask` record, and legacy `--read-only` input | Normalize each record for display and inspect the overview renderer | All are displayed as `plan` or `graph-plan`; no run mode is rendered as `read-only`; grouping is unchanged | `src/run_registry.rs` `tc_2508_plan_mode_normalization`; `src/web/assets.rs` `tc_2508_overview_has_no_read_only_mode` |
| TC-2509 | FR-2511 | unit | Run records with known start/end, a live run, a graph run, and a record with missing model | Compute duration and render metadata at two clock readings | Live duration grows; terminal duration is final; graph model is the orchestrator model; missing model reads unknown; duration is never negative | `src/run_registry.rs` `tc_2509_run_timing_and_metadata` |
| TC-2510 | FR-2508, FR-2512, INT-2501 | unit + integration + asset | Registry with a malformed record, an unknown newer schema, and a secret-bearing label attempt; router with token/Host/Origin checks | Read the registry; call run endpoints without and with a token; inspect stored layout and URLs | Malformed and newer records are ignored; auth behavior matches today; registry and layout contain no token, secret, prompt, or transcript text; unknown schema does not crash the reader | `src/run_registry.rs` `tc_2510_schema_and_secret_boundaries`; `tests/web_multi_session.rs` `tc_2510_run_api_auth_unchanged`; `src/web/assets.rs` `tc_2510_layout_has_no_secret_fields` |
| TC-2511 | FR-2513, NFR-2500 | integration + e2e (ignored, headless Chrome) | Two workspaces with live runs; SSE connection; then force the SSE error path | Emit registry and conversation updates for both runs; close SSE; inspect the page | Both update through one stream; the polling fallback takes over; a run with no new data keeps its last good state and shows stale; no per-run EventSource is created | `src/web/assets.rs` `tc_2511_single_shared_event_source`; `tests/web_multi_session.rs` `tc_2511_overview_shared_fallback` |
| TC-2512 | UX-2500 | e2e (ignored, headless Chrome) | Overview with live, done, failed, awaiting, and cancelled runs; OS/browser set to light and reduced motion | Navigate only by keyboard; trigger live deltas; inspect live regions, state text, and theme | Every row and action is reachable with visible focus; states have text, not only color; updates use polite live regions; first visit is light; reduced motion is respected | `tests/web_multi_session.rs` `tc_2512_overview_keyboard_aria_and_light_theme` |
| TC-2513 | FR-2508, FR-2514, INT-2503, NFR-2502 | unit + integration | Registry with live, awaiting, and terminal records of known ages; an oversized registry; a temp state dir | Run `runs prune --before`, `--keep`, and `--dry-run`; then inspect the overview after pruning; attempt invalid selectors | `--before` and `--keep` remove only matching terminal records and their approvals; live and awaiting records remain; `--dry-run` reports without deleting; invalid selectors delete nothing and exit non-zero; history stays unbounded without an explicit prune; the overview paginates and warns over the size threshold | `src/run_registry.rs` `tc_2513_manual_prune_and_disk_growth`; `src/cli.rs` `tc_2513_runs_prune_args` |
| TC-2520 | FR-2520 | e2e (ignored, headless Chrome) | Dashboard with two stored conversations and a fake live run | Load the dashboard, add panes to nine, attempt a tenth, reorder by drag and keyboard, remove a pane | One pane by default; panes appear in a grid; the tenth pane is refused with a notice and adds nothing; reordering works; removing the last pane leaves an explicit empty pane | `tests/web_multi_session.rs` `tc_2520_pane_grid_default_and_max` |
| TC-2521 | FR-2521 | e2e (ignored, headless Chrome) | Three panes showing different conversations and runs | Change pane 1 to a third conversation; pin a run into pane 2; choose a deleted run in pane 3 | Only the targeted pane switches; other panes keep their selections and keep updating; the deleted run shows an in-pane error; pinning does not start the run | `tests/web_multi_session.rs` `tc_2521_per_pane_selection_and_pin` |
| TC-2522 | FR-2522 | unit (asset) + e2e (ignored) | Embedded `session.js` and `app.js`; dashboard with three panes | Static check for EventSource construction; run snapshots for two sessions and assert each pane | Exactly one EventSource exists in the page; only the pane matching a session updates; no per-pane stream is created | `src/web/assets.rs` `tc_2522_single_shared_event_source`; `tests/web_multi_session.rs` `tc_2522_panes_route_shared_snapshots` |
| TC-2523 | FR-2523, FR-2524 | e2e (ignored, headless Chrome) | `dmuxai --web` with an interactive current-App conversation and a pinned headless run; then `dmuxai serve` | Focus each pane and type or send; inspect composer, Stop, and view-only notices | Only the focused interactive pane sends; the pinned headless pane stays view-only; in `serve` every pane is view-only; Stop targets only the focused conversation; no input goes to a non-focused pane | `tests/web_multi_session.rs` `tc_2523_focused_pane_and_view_only` |
| TC-2524 | FR-2525 | e2e (ignored, headless Chrome) | Workspace A with two panes and a custom selection; workspace B with no saved layout; a corrupt stored value | Reload A; open B; load with the corrupt value; inspect `localStorage` and the URL | A restores its panes, order, and selections; B starts with one pane; corrupt data falls back to one pane; no token, secret, or transcript text is stored | `tests/web_multi_session.rs` `tc_2524_layout_persistence_and_isolation` |
| TC-2525 | FR-2526, NFR-2520 | e2e (ignored, headless Chrome) | Nine panes with a high-rate fake event stream; one pane collapsed; an attempt to add a tenth | Stream for 10 seconds while collapsing a pane and resizing the window | The focused pane stays responsive; memory stays bounded; the collapsed pane keeps state and catches up; the tenth pane is refused | `tests/web_multi_session.rs` `tc_2525_pane_backpressure_and_collapse` |
| TC-2526 | UX-2520, UX-2521 | e2e (ignored, headless Chrome) | Dashboard with three panes, a live update stream, a narrow viewport, and reduced motion | Navigate only by keyboard; resize to mobile width; trigger live deltas; inspect live regions and focus | Panes and pickers are keyboard reachable with visible focus; narrow width uses the pane switcher; updates use polite live regions; spinners respect reduced motion | `tests/web_multi_session.rs` `tc_2526_pane_keyboard_aria_and_responsive` |
| TC-2527 | FR-2502, FR-2503, FR-2521 | integration + e2e (ignored, headless Chrome) | Registry with three runs in one session group across two workspaces, plus an ungrouped run; overview and two panes | Group by orchestration request; pin one grouped run into a pane; filter or collapse the group | The three grouped runs appear under one orchestration request while remaining grouped by workspace; the ungrouped run stays separate; pinning shows that run in the chosen pane only | `tests/web_multi_session.rs` `tc_2527_orchestration_group_and_pin` |
| TC-2540 | FR-2509, FR-2540, FR-2542, INT-2541 | integration + e2e (ignored, headless Chrome) | An `awaiting_approval` run with a known plan hash; router with token, CSRF, Host, and Origin checks; a stale and a changed plan hash | Attempt GET, missing token, missing CSRF, bad Origin, stale hash, changed hash, and a valid POST after confirming the dialog; attempt a sensitive plan | Non-POST and unauthorized requests change nothing; stale or changed hashes return a conflict; the valid POST records approval; dismissing the dialog sends nothing; sensitive plans stay blocked and no `--allow-sensitive` grant appears | `src/web/server.rs` `tc_2540_approval_endpoint_security_and_plan_hash`; `tests/web_multi_session.rs` `tc_2540_approval_dialog_and_stale_plan` |
| TC-2541 | FR-2501, FR-2541, FR-2546 | unit + integration | Registry and workspace event log with an `awaiting_approval` run; a simulated audit write failure | Approve one run, reject another, repeat a rejection, and inspect both audit sinks and run states | Each decision writes one audit event with session, timestamp, run id, plan hash, workspace, and decision; approval moves to `approved`, rejection to `rejected`; duplicate rejection is idempotent; a failed registry audit blocks the decision | `src/run_registry.rs` `tc_2541_approval_audit_and_rejection`; `tests/web_multi_session.rs` `tc_2541_workspace_event_log_audit` |
| TC-2542 | FR-2542, FR-2543, FR-2547, INT-2540 | unit + integration | Approval records for a run started by another process; two concurrent approvers; duplicate approvals; two different plan hashes | Write concurrent approvals, replay a duplicate, approve a stale hash, and let the original orchestrator poll the registry | Exactly one approval wins and exactly one resume is requested; duplicate and stale approvals are no-ops; the orchestrator sees `approved` through the registry; sensitive steps are never granted | `src/run_registry.rs` `tc_2542_single_use_concurrent_approval`; `tests/web_multi_session.rs` `tc_2542_cross_process_approval_and_orchestrator_poll` |
| TC-2543 | FR-2544, FR-2545, INT-2540, INT-2541 | integration + e2e (ignored, headless Chrome) | Approved run with a stored conversation; #202 available and unavailable; `dmuxai --web`; view-only `serve`; an explicitly enabled spawner | Resume from `--web`, request resume from `serve`, resume without #202, and let an orchestrator resume itself | `--web` spawns a new run in the original workspace with a new run id, `resumed_from`, and the same session group; `serve` records the approval but spawns nothing; without #202 the run re-runs the original prompt and marks it not a true continuation; the orchestrator sees the resulting state | `tests/web_multi_session.rs` `tc_2543_resume_spawn_and_serve_view_only`; `src/run_registry.rs` `tc_2543_resumed_from_and_conversation_continue` |
| TC-2544 | UX-2540 | e2e (ignored, headless Chrome) | Overview with an `awaiting_approval` run; OS/browser set to reduced motion | Open the confirmation dialog by keyboard, inspect the plan details, cancel with Escape, reopen, approve, and inspect the state badge | The dialog shows steps, runners/models, and mode; focus is trapped and returns on close; approve, reject, and cancel are named; stale-plan errors are announced; approved, resumed, and rejected use text as well as color; reduced motion is respected | `tests/web_multi_session.rs` `tc_2544_approval_dialog_keyboard_and_state` |

Required coverage prompts: TC-2500/TC-2501 cover registry happy path, boundaries,
and terminal transitions; TC-2502/TC-2508 cover cross-workspace discovery,
history, and plan-mode vocabulary; TC-2503 covers labels and grouping; TC-2504
covers live cross-workspace conversation and read-only isolation; TC-2505 and
TC-2506 cover the #147 and #148 regressions; TC-2507 covers approval and
cancellation states; TC-2509/TC-2510 cover timing, schema, and security; TC-2511
covers stream failure and fallback; TC-2512 covers overview accessibility;
TC-2513 covers indefinite history, manual pruning, and disk-growth bounds;
TC-2520/TC-2521 cover layout, the nine-pane cap, and per-pane selection;
TC-2522/TC-2523 cover the shared stream and input isolation; TC-2524/TC-2525
cover persistence and performance at nine panes; TC-2526 covers pane
accessibility and narrow viewports; TC-2527 covers orchestration grouping and
pinning; TC-2540/TC-2541 cover approval security, audit, and rejection;
TC-2542 covers cross-process single-use approval; TC-2543 covers resume spawn
ownership and the #202 fallback; TC-2544 covers approval-dialog accessibility.

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No conversation-database or server schema change. Add a per-user run registry under the state directory: one owner-only JSON record per headless run, optional approval records keyed by run id and plan hash, plus an optional derived index. Pane layout stays only in browser `localStorage`, keyed by normalized workspace and a layout version. Run history is kept forever and removed only by `dmuxai runs prune`. | Removing the registry directory is safe: runs continue, the overview is empty, and the next run recreates the registry. Clearing the layout key falls back to one pane. No migration is required; prune is the only destructive operation and supports `--dry-run`. | TC-2500, TC-2502, TC-2513, TC-2524 |
| Registry schema | Each record has a schema version and additive fields for run id, workspace path, conversation id, agent, model, mode, label, session group, state, timestamps, duration, exit code, last event seq, owner pid, lease, and `resumed_from`. Approval records additively carry run id, plan hash, nonce, decision, dashboard session, timestamp, workspace, and optional resume linkage. Records never contain prompt text, transcript text, tokens, provider credentials, or MCP secrets. | Older readers ignore unknown fields; a newer schema version hides only that record. Records are never rewritten by another process except for the single-use approval claim and explicit pruning. | TC-2510, TC-2542 |
| Provider protocol | No change to runner protocols, NDJSON events, graph records, or exit-code meanings. The registry observes the existing headless outcome and event log. | An older run without registry fields is invisible to the overview but unchanged in the workspace. | TC-2501, TC-2509 |
| Cross-workspace reads | The dashboard reads other workspaces' conversation state only through bounded, read-only projections and never writes to their database, graph files, or configuration. Approval decisions write only the registry approval record and, for an allowed resume, a new run in the original workspace. | An unavailable or busy workspace returns an error for that run only; the current workspace and other runs stay usable; a failed workspace audit is surfaced. | TC-2504, TC-2541 |
| Security / permissions | The existing dashboard token, Host, and Origin checks remain mandatory. Approval and rejection are POST-only, CSRF-protected, and carry a per-plan nonce and plan hash. Registry directory is 0700 and files are 0600, atomically written. No token, secret, prompt, or transcript text enters the registry, pane layout, pane URLs, or labels. | Must not weaken existing auth, cross-workspace isolation, or the view-only `serve` guarantee. A permission problem disables the registry with a warning rather than relaxing modes; a stale or changed plan hash is rejected. | TC-2510, TC-2540 |
| Plan approval and non-writing runs | Exit 7 and graph `AwaitingApproval` are displayed as awaiting approval, not failed and not done. An explicit confirmation dialog may approve or reject the plan; approval matches CLI `--approve-plan`, never implies `--allow-sensitive`, and is audited. Non-writing runs are displayed as plan mode, never as a "read-only" mode. | Existing CLI/MCP inputs still work; legacy `ask` and `--read-only` are normalized only for display and grouping. Sensitive-step approval from the web is a non-goal. | TC-2507, TC-2508, TC-2540, TC-2541 |
| Approval and resume IPC | Headless runs exit with code 7 instead of waiting. The dashboard writes an atomic approval record keyed by run id and plan hash; the original orchestrator polls the registry for the outcome. Resume starts a new run linked by `resumed_from`, sharing the session group and using #202 conversation continue when available; without #202 it re-runs the original prompt and records that it is not a true continuation. | Removing an unused approval record changes nothing; a malformed record hides only that decision. A view-only `serve` records decisions but never spawns a resume; orchestrators may resume themselves. | TC-2542, TC-2543 |
| Resume process ownership | Only the dashboard host in `dmuxai --web` mode or an explicitly enabled spawner starts the resumed run in the original workspace. `serve` is record-only. A spawn failure is recorded without changing the approval. | Disabling spawning leaves approvals usable by an external orchestrator; the original orchestrator observes the resulting state through the registry. | TC-2543 |
| Platform | Linux and other current desktop browsers; no terminal dependency for the overview or panes. | Narrow viewports use the pane switcher fallback; reduced motion follows OS and existing settings. | TC-2526 |

## 7. Implementation plan

Detailed plan: `../plans/PLAN-web-multi-session.md` (to be added before
implementation). The plan must reference requirement and test IDs and must not
introduce new behavior.

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add the per-user run registry: atomic owner-only writes, schema version, stale-owner recovery, read-side validation, and the manual `runs prune` command | `src/run_registry.rs` (new), `src/state_path.rs`, `src/runtime_env.rs`, `src/cli.rs` | FR-2500, FR-2501, FR-2507, FR-2508, FR-2514, BR-2500, INT-2501, INT-2503, NFR-2501, NFR-2502 | TC-2500, TC-2501, TC-2510, TC-2513 |
| 2 | Record runs from `run`, `todo run`, and MCP `run`; add label and session-group flags/args and additive JSON fields; map exit 7, 130, and plan mode | `src/cli.rs`, `src/mcp_server.rs`, `src/agent_api.rs`, `src/run_registry.rs` | FR-2500, FR-2501, FR-2503, FR-2509, FR-2510, FR-2511, INT-2500 | TC-2501, TC-2503, TC-2507, TC-2508, TC-2509 |
| 3 | Expose read-only run discovery and run-conversation endpoints under the existing auth and add registry data to snapshots/events | `src/web/server.rs`, `src/web/snapshot.rs`, `src/web/file_source.rs`, `src/web/hub.rs` | FR-2502, FR-2504, FR-2507, FR-2512, FR-2513, INT-2502 | TC-2502, TC-2504, TC-2510, TC-2511 |
| 4 | Add the run overview UI, grouping, state badges, history, approval state, and the #147 sidebar entry | `src/web/assets/app.js`, `src/web/assets/session.js`, `src/web/assets/session.css`, `src/web/assets/index.html` | FR-2502, FR-2505, FR-2508, FR-2509, FR-2511, UX-2500 | TC-2502, TC-2505, TC-2507, TC-2509, TC-2512, TC-2527 |
| 5 | Fix the status pill to include headless and multiple live runs (#148) | `src/web/assets/app.js` | FR-2506 | TC-2506 |
| 6 | Add pane/layout state, per-workspace `localStorage` persistence, add/remove/rearrange, and per-pane picker/pin with a nine-pane cap | `src/web/assets/session.js`, `src/web/assets/app.css`, `src/web/assets/index.html` | FR-2520, FR-2521, FR-2525 | TC-2520, TC-2521, TC-2524, TC-2527 |
| 7 | Route the shared snapshot and registry stream to panes; enforce one EventSource, bounded buffers, and collapse/backpressure | `src/web/assets/app.js`, `src/web/assets/session.js` | FR-2513, FR-2522, FR-2526, NFR-2500, NFR-2520 | TC-2511, TC-2522, TC-2525 |
| 8 | Enforce focused-pane input and view-only pane rules, including `serve` | `src/web/assets/session.js`, `src/web/server.rs` | FR-2523, FR-2524 | TC-2523 |
| 9 | Keyboard navigation, live regions, light-theme default, reduced motion, and the narrow-viewport one-pane switcher | `src/web/assets/session.js`, `src/web/assets/app.css`, `src/web/assets/index.html` | UX-2500, UX-2520, UX-2521 | TC-2512, TC-2526 |
| 10 | Add the approval/rejection endpoints, confirmation dialog, plan hash/nonce checks, audit events, and registry state transitions | `src/web/server.rs`, `src/web/assets/app.js`, `src/web/assets/session.js`, `src/run_registry.rs` | FR-2540, FR-2541, FR-2542, FR-2543, FR-2546, FR-2547, INT-2540, INT-2541, UX-2540 | TC-2540, TC-2541, TC-2542, TC-2544 |
| 11 | Implement resume spawning ownership, `resumed_from` linkage, session-group reuse, and the #202 conversation-continue fallback | `src/run_registry.rs`, `src/agent_api.rs`, `src/web/server.rs`, `src/cli.rs` | FR-2544, FR-2545, INT-2540 | TC-2543 |
| 12 | Add deterministic registry, API, asset, and browser tests; update traceability and review notes | `tests/web_multi_session.rs`, `src/web/assets.rs`, `src/run_registry.rs`, `src/cli.rs`, `src/mcp_server.rs` | All Must requirements | TC-2500..TC-2544 |

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
      residual risk, and rollback notes, including the resolved decisions
      OQ-2500..OQ-2503 and OQ-2505 and the remaining questions in section 9.

## 9. Open questions

1. **Interactive input in multiple panes (OQ-2504).** Should only the focused
   pane ever accept input, or may two panes be interactive at once? The current
   contract is focused-pane-only (FR-2523).
2. **Resume spawning under `serve` (OQ-2506).** Should a view-only `serve`
   process ever be allowed to spawn a resumed run, or must it always record the
   approval and leave execution to `dmuxai --web` or an orchestrator? The
   current contract forbids spawning from `serve` (FR-2545).
3. **Default runner for resume (OQ-2507).** When a resume is requested, should
   the new run reuse the original run's agent and model, or require the user or
   orchestrator to choose? The proposal is to reuse the original values when
   available and refuse with an explicit error when they are missing.
4. **Approval record lifetime (OQ-2508).** Should an unused approval record
   persist until the run is manually pruned, or expire after a configured time?
   The proposal is to keep it with the run until `dmuxai runs prune` removes
   both.
