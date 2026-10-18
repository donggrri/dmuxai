# SPEC: Dashboard full parity and TUI retirement

Status: Draft (v0.2.4; gap list awaiting Dongjin's review, no implementation yet)
Owner: DmuxAI
Created: 2026-10-18
Baseline: `origin/main` f1b36ae (v0.2.2 in progress: includes #65 swimlane and #66 grok runner; #70 TUI picker width is merged (f1b36ae); #71 resizable sidebars is an open v0.2.2 PR)
Related: SPEC-dashboard-parity (v0.2.0 slice G2, FR-780..784; the slash-command matrix this spec extends), SPEC-web-dashboard (`serve`, FR-520..530), SPEC-agent-api (headless `run`, FR-760..773), SPEC-session-sidebar, SPEC-workspace-dir, SPEC-grok-runner (PR #66), SPEC-plan-gate (PR #67)

> File name: `SPEC-dashboard-parity.md` already exists and is the implemented
> v0.2.0 contract (its TC-780..787 are referenced by tests), so this follow-up
> lives in `SPEC-dashboard-full-parity.md`. IDs use the FR-1000 block so they
> cannot collide with v0.2.3 specs still being written.

## 1. Decision summary

Dongjin, 2026-10-18 KST: "모든 TUI 기능이 대시보드에서 구현이 된다면 TUI는
사용하지 않고 제거하는 방향으로 간다", scheduled for the release after v0.2.3
("그 기능은 다음 릴리즈에").

v0.2.4 does three things, in order:

1. **Headless interactive serve.** `dmuxai serve` runs the full session core
   (agents, graph, to-do runs, memory, notify, conversation store) without a
   terminal, so the dashboard can do everything `dmuxai --web` does today.
   Today `serve` has no session: prompts and session commands answer 409
   and Docs are read-only; only the to-do board (CRUD, import) and memory are
   writable there (§3.1).
2. **Close every gap in §3** by moving the behavior into the shared Rust core
   (App/session layer, `src/web/*`) and adding the dashboard control. No
   feature logic is written only in JavaScript.
3. **Retire the TUI** behind a deprecation step: one release where the TUI
   still starts but prints a notice, then removal of the ratatui code paths,
   TUI-only settings and the TUI-only parity rows.

## 2. Scope

### In scope

- New run mode: interactive `dmuxai serve` (FR-1000..1004).
- Extraction of the App event loop into a terminal-independent session host
  (FR-1001, NFR-1000).
- Every row marked **missing** or **partial** in §3, unless Dongjin drops it
  in review.
- Parity guard: a test that fails when a TUI feature row has no dashboard
  mapping (extends FR-784 from slash commands to keys, mouse, panels and
  settings) (FR-1020).
- Features of the in-flight PRs #66 (grok runner) and #67 (plan gate) and the
  rest of v0.2.3 (diff review sidebar, world model) must be on the dashboard
  before removal (FR-1021).
- TUI deprecation and removal (FR-1030..1032).

### Out of scope / non-goals

- Implementation in this PR (spec, plan, tasks and roadmap only).
- Remote multi-user sessions; still one session host per workspace.
- A native desktop wrapper (the Windows launcher keeps opening the browser).
- Terminal-only affordances that have a native browser equivalent: mouse text
  selection + copy, wheel scroll, Backspace/typing, paste-ending-in-newline
  auto-submit, the `(drag top border)` input resize (the textarea already has
  `resize: vertical`). These are listed as `n/a` in §3 and need no work.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1000 | `dmuxai serve` becomes interactive by default and `serve --read-only` keeps today's behavior (no session; to-dos and memory writable, Docs read-only). Alternative: a new `dmuxai web` subcommand and `serve` unchanged. Proposed: default interactive. | Dongjin | Spec approval |
| OQ-1001 | `dmuxai` with no arguments: after removal it starts `serve` and opens the browser (like the Windows launcher), or prints usage. Proposed: start `serve` and print the URL; open the browser only with `--open`. | Dongjin | Before FR-1031 |
| OQ-1002 | Deprecation length: one release (v0.2.4 deprecates, v0.2.5 removes) or remove in v0.2.4 once the parity test is green. Proposed: deprecate in v0.2.4, remove in the next release. | Dongjin | Before FR-1030 |
| OQ-1003 | Keyboard shortcuts in the browser: F2/F3 are often taken by the browser/OS. Proposed: `Alt+M` model, `Alt+C` conversations, `Esc` twice or `Ctrl+.` cancel, `Ctrl+O` details; all shown in the palette hints. | Dongjin | UX-1000 |
| OQ-1004 | Clicking a file link in the transcript: open it in the Docs preview (in-browser) rather than the OS app the TUI launches. Proposed: Docs preview, with "Open in OS app" only on a loopback bind. | Dongjin | FR-1012 |

## 3. Gap audit (TUI vs dashboard), baseline f1b36ae

Status: **full** = the dashboard does the same thing today; **partial** = the
dashboard can reach it but with less behavior; **missing** = no dashboard
equivalent; **n/a** = terminal-only affordance with a native browser
equivalent, or a TUI-only concern that disappears with the TUI.

### 3.0 What each mode does today without the TUI

| Mode | Runs without a terminal? | What works | What does not |
| --- | --- | --- | --- |
| `dmuxai` (no args) | No: exits 1 with `dmuxai needs an interactive terminal (TTY)` when stdin or stdout is not a TTY (`src/main.rs:607-612`) | TUI only; a hub feeds the event log, no HTTP server (`:656-661`) | no dashboard at all |
| `dmuxai --web` | No: same TTY check, then `TerminalSession::start` and `App::run(terminal)` (`:663-664`); the dashboard server lives inside the TUI process (`attach_web_dashboard`, `:730-829`) | everything in §3.2-3.6 marked full: prompts (`POST /api/prompt` 202), session commands (`POST /api/command` 200), pickers, conversations, to-dos incl. Run, memory, writable Docs, workspace switch (Unix) | it cannot be started headless; quitting the TUI stops the dashboard |
| `dmuxai serve` | Yes (`run_serve`, `:437-566`): `SnapshotSource::Files` (`:515`), polls `.dmuxai` graph runs every 500 ms (`:552-559`), prints the URL, writes `web.json` | graph view of persisted runs, to-do create/edit/delete and MCP import, memory get/save/approve/reject, event log, **read-only** Docs preview (`:467`) | no App, no agents, no session: prompts and session commands 409 `requires dmuxai --web` (`src/web/server.rs:929-931`, `:970-972`), to-do Run 409 (`:875-876`), workspace browse/switch 403 (`:1228-1229`, `:1341-1342`), no webhook/command hooks, no custom agents (incl. grok) in the snapshot |
| `dmuxai run` / `todo run` | Yes: `App::run_headless` in-process (`src/app.rs:4749`), one prompt or to-do, exit codes | full agent and graph execution for one job | no HTTP server, no dashboard |
| `dmuxai status` / `events` / `todo add\|list` / `mcp` / `mcp-memory` / `init` | Yes | read state, MCP over stdio | no prompts from a browser |

So the only way to drive agents from the browser today is `dmuxai --web`, and that needs the TUI in a real terminal. That is gap R1.

### 3.1 Run modes

| # | TUI / CLI feature | TUI source | Dashboard today | Status | Gap → requirement |
| --- | --- | --- | --- | --- | --- |
| R1 | Interactive session without a terminal | `src/main.rs:606` `run_tui` always builds a ratatui `Terminal`; `src/app.rs:1187-1279` `App::run` mixes the core loop with `event::poll` and `terminal.draw` | `dmuxai serve` (`src/main.rs:437-566`) has **no session**: `SnapshotSource::Files`, `commands: None` (`:491`), Docs `read_only = true` (`:467`), no App and no agents (to-do CRUD/import and memory save/approve/reject do work, `:486-512`); `POST /api/prompt` and `/api/command` answer 409 "requires dmuxai --web" (`src/web/server.rs:929-931`, `:970-972`); to-do Run 409 (`:875-876`) | **missing** | headless interactive serve, FR-1000..1004 |
| R2 | `dmuxai --web` (TUI + dashboard) | `src/main.rs:164-165` parse, `:354` dispatch, `:730-829` `attach_web_dashboard` | the only interactive dashboard mode; exits 1 without a TTY (`src/main.rs:607-612`) | partial | becomes `serve`, FR-1000, FR-1031 |
| R3 | Dashboard command channel | `src/main.rs:738-747`: the `DashboardCommand` channel is created only when the to-do DB opens | if the to-do DB fails, prompts and pickers are 409 even under `--web` | partial (coupling bug) | FR-1003 |
| R4 | Headless one-shot CLI: `run`, `todo add/list/run`, `status`, `events`, `mcp`, `mcp-memory`, `init` | `src/main.rs:349-356` dispatch, `src/cli.rs:552-613` `run_headless`, `src/app.rs:4749` `App::run_headless` | not a TUI feature; already terminal-free | full | keep; `run_headless` is the model for FR-1001 |
| R5 | Workspace switch (dashboard dialog, re-exec) and directory browse | no TUI switcher (`-C` only, `src/main.rs:296-317`); re-exec `:678-724` | works only under `--web`; `serve` answers 403 on directory browse and switch (`server.rs:1228-1229`, `:1341-1342`). `GET /api/workspace` stays 200 with `can_switch` false (`:1199`) | partial | FR-1000 (switch re-execs `serve`) |

### 3.2 Slash commands (`SLASH_COMMANDS`, `src/app.rs:113-189`, plus `/graph` at `:2860`)

All are mapped in `src/web/parity.rs:20-136` and reachable from the Ctrl+K
palette (`src/web/assets/session.js:647-697`).

| # | Command | TUI source | Dashboard | Status | Gap |
| --- | --- | --- | --- | --- | --- |
| S1 | `/agent` | `app.rs:2280` | Agent picker `session.js:266` | full | — |
| S2 | `/mode` | `app.rs:2355` | Mode picker `session.js:269` | full | — |
| S3 | `/effort` | `app.rs:2583` | Effort picker `session.js:287` | full | — |
| S4 | `/status` | `SessionCommand::Status` | Status button `session.js:299` | full | — |
| S5 | `/quit` | `SessionCommand::Quit` (`app.rs:1863`) | Quit button + confirm `session.js:303` | full | semantics in serve, FR-1004 |
| S6 | `/new` | `create_conversation` | New button `session.js:298`, sessions sidebar | full | — |
| S7 | `/conversations` | `app.rs:2286` | sessions sidebar (SPEC-session-sidebar) | full | — |
| S8 | `/resume <id>` | `select_conversation` | sidebar Load (`resume`) `session.js:592` | full | — |
| S9 | `/summarize` | `app.rs:7252` `summary_prompt` | palette | full | — |
| S10 | `/compact [focus]` | alias of S9 | palette | full | — |
| S11 | `/share` | `app.rs:3281` | Share menu `session.js:301` | full | — |
| S12 | `/image <path>` | `app.rs:2661` `attach_image_path` | palette asks for a **workspace path** (`session.js:671`), which the core attaches like the TUI; no file picker, drop or paste upload | partial | browser upload / drop / paste, FR-1010 |
| S13 | `/details` | `app.rs:3757` `toggle_all_activity_details` | Details button `session.js:300` flips the flag and the button state, but `session_view` always sends each activity's collapsed line (`app.rs:1590-1592`), so nothing expands in the browser | partial | FR-1011 |
| S14 | `/remember` | `app.rs:4341` | palette + Memory panel (`memory.js`) | full | — |
| S15 | `/memory [approve\|reject]` | `app.rs:4386` | palette + Memory panel approve/reject routes (`server.rs:1495-1502`) | full | — |
| S16 | `/graph [status\|continue\|override]` | `app.rs:2860-2872`, `:4105-4150` | palette only (`session.js:688-695`); the graph view has no Continue button or override form, and its gate hint says `Type /graph continue in the TUI to proceed.` (`app.js:1690-1700`); the `Allowed runners:` line exists only as the `/graph` system message | partial | gate controls in the graph view, FR-1014 |
| S17 | `//text` (send a literal leading slash) | `app.rs:2750` | handled by the same core path via `POST /api/prompt` | full | — |

### 3.3 Key bindings (`App::handle_key`, `src/app.rs:1996-2058`, pickers `:2060-2638`)

| # | Key | TUI action | Dashboard | Status | Gap |
| --- | --- | --- | --- | --- | --- |
| K1 | Enter | submit (`app.rs:2043`) | Enter sends (`session.js:393`) | full | — |
| K2 | Shift+Enter | not a TUI binding: `handle_key` submits on Enter regardless of Shift (`app.rs:2043`); multiline only via paste. `parity.rs:26-30` lists it as a TUI key by mistake | dashboard inserts a newline | n/a | fix the `parity.rs` row label in FR-1020 |
| K3 | Esc | close overlay | closes palette/pickers/sidebar (`session.js:358,396,760,815`) | full | — |
| K4 | Ctrl+C | cancel the selected agent's turn (`app.rs:1997`, `:3168`) | Cancel button only (`session.js:295`) | partial | shortcut, UX-1000 |
| K5 | Ctrl+O | toggle the **last** activity group (`app.rs:2001`, `:3746`) | Details toggles **all** groups (`app.rs:1862`) | partial | per-group expand + shortcut, FR-1011, UX-1000 |
| K6 | F2 | model picker (`app.rs:2041`, `:2455`) | Model picker click only | partial | shortcut, UX-1000 |
| K7 | F3 | conversation picker (`app.rs:2042`) | sessions sidebar click only | partial | shortcut, UX-1000 |
| K8 | Ctrl+V / Alt+V | attach clipboard image (`app.rs:2030-2036`, `:2704`, `src/clipboard_image.rs`) | none | **missing** | FR-1010 |
| K9 | `/` + Up/Down/Tab/Enter | slash preview (`app.rs:2060-2091`, draw `:6463`) | `/` in an empty box opens the palette (`session.js:336-341`), filter and arrows (`:788-823`); Tab does not run a row | full | — |
| K10 | Picker Up/Down/Enter/Esc; Tab in the Codex model picker jumps to effort (`app.rs:2517-2559`) | native `<select>` pickers; effort is its own picker | full | — |
| K11 | Backspace, typing | edit the input | native textarea | n/a | — |

### 3.4 Mouse and paste (`App::handle_mouse` `src/app.rs:2144-2208`, `handle_paste` `:1977-1994`)

| # | Feature | TUI source | Dashboard | Status | Gap |
| --- | --- | --- | --- | --- | --- |
| M1 | Drag-select chat text, copy to clipboard, "Copied N characters" notice | `app.rs:2230-2250`, `src/chat_selection.rs`, `src/clipboard.rs:8` | native browser selection and copy | n/a | — |
| M2 | Click a file path/link in the chat to open it | `app.rs:2153`, `:4487` `open_file_link_at`, `:6607` | transcript is plain text (`session.js:483-551`, `textContent`); no links | **missing** | FR-1012 |
| M3 | Drag the input's top border to resize | `app.rs:2258` | textarea `resize: vertical` (`session.css:30`) | n/a | — |
| M4 | Wheel scroll | `app.rs:2198-2205` | native scroll with stick-to-bottom (`session.js:317`) | n/a | — |
| M5 | Paste a single image path → attach it | `app.rs:1979-1985` | pasted as text | **missing** | FR-1010 |
| M6 | Paste ending with a newline auto-submits | `app.rs:1987-1993` | — | n/a | — |

### 3.5 Panels and display (`App::draw`, `src/app.rs:5348-5371`)

| # | Panel | TUI source | Dashboard | Status | Gap |
| --- | --- | --- | --- | --- | --- |
| P1 | Chat transcript, agent colors, streaming | `draw_chat` `app.rs:5446` | transcript with agent color edge and streaming class (`session.js:483-551`) | full | — |
| P2 | Activity groups (tools/thinking) with collapse/expand and `[ui] show_tools`/`show_thinking` | `render_activity_group` `app.rs:5699-5778` | activity messages are plain text; no per-group expand; `[ui]` settings ignored | partial | FR-1011 |
| P3 | Graph status line above the input (`Graph i/n · role · runner model · state`) | `app.rs:5373-5396`, `:5679` | graph view / swimlane (`app.js`, SPEC-graph-swimlane) | full | — |
| P4 | Status bar: state badge with spinner and elapsed, model, mode, effort, **activity detail**, **other agents' activity**, **graph assignment line**, **notice slot** (`copy_notice`, also used for background-conversation notices such as a failed save, `app.rs:5340-5344`) | `draw_status` `app.rs:5847-5998` | agent chips with running/elapsed (`app.js:1739-1788`), pickers show model/mode/effort; no activity detail, no other-agents summary, no assignment line, background-conversation notices never reach the page | partial | FR-1013 |
| P5 | Attachments chip line in the prompt title | `draw_input` `app.rs:5785-5793` | none (no `pending_images` in the snapshot) | **missing** | FR-1010 |
| P6 | Model picker with catalog loading, policy filter, Codex effort | `app.rs:6000`, `:2455-2578` | Model picker + "Load models…" | full | — |
| P7 | Effort / agent / mode / conversation pickers | `app.rs:6095`, `:6209`, `:6390`, `:6278` | pickers + sidebar with running mark | full | — |
| P8 | Startup notices, welcome/help text, system messages | welcome text `app.rs:1000-1002`; `push_startup_notice` `:4430` | transcript `system` messages | full | help text names TUI keys, FR-1032 |
| P9 | Sensitive-step gate prompt (`Gate: confirm … with /graph continue`) | `app.rs:3989`, `:4136` | system message + palette `/graph continue` | partial | FR-1014 |
| P10 | Built-in grok runner (#66, merged): picker entry, preset color `#6366f1`, `grok models` probe and the `grok login` message | TUI `/agent` confirm probes (`app.rs:2345`, `:2265`), reply text in the preset color | dashboard agent control runs the same probe (`SessionCommand::SelectAgent`, `app.rs:1682-1690`); chip color via `AgentView.color` (SPEC-grok-runner FR-896, FR-898, tests `grok_chat_and_dashboard_use_preset_color`, `selecting_grok_requests_a_model_probe`) | full | under `--web` only; under `serve` grok is absent (R1) |
| P11 | Conversation picker width scales with the terminal (#70, merged (f1b36ae), SPEC-session-sidebar FR-865) | `draw_conversation_picker` `app.rs:6278` | the sessions sidebar already lays out by CSS, and #71 (open, SPEC-sidebar-resize FR-970..974) makes it resizable | n/a | terminal layout fix; disappears with the TUI |

### 3.6 Settings (`src/settings.rs`)

| # | Setting | Used by | Status | Gap |
| --- | --- | --- | --- | --- |
| C1 | `[theme]` TUI colors, `force_color`, `DMUXAI_FORCE_COLOR`, `NO_COLOR` (`settings.rs:52-53`, `src/theme.rs`) | TUI rendering only; `[web] theme/accent` is the dashboard's | n/a | deprecate with the TUI, FR-1032 |
| C2 | `[ui] show_tools`, `show_thinking` (`src/activity.rs:110-115`) | TUI activity rendering only (`app.rs:5714-5741`) | **missing** | FR-1011 |
| C3 | `reduced_motion` | TUI spinner and dashboard (`hub.set_ui`, `main.rs:516-520`) | full | — |
| C4 | `[cursor] prewarm*`, `[graph]`, `[routing]`, `[memory]`, `[notify]` delivery, `[[agents.custom]]`, `builtin_opencode`, `[mcp]`, `[todo]`, `[workspace]` | core (App) | full under `--web`; **not running** under `serve` (no App, no `EventSink`, so no webhook/command hooks and no custom agents; workspace browse/switch answer 403, `server.rs:1228-1229`, `:1341-1342`). The TUI never edited these either, so no settings editor is required | full | covered by R1 / FR-1001. Side finding: `[routing] cursor_models` is parsed but never read |

### 3.7 Core work that only runs inside the TUI loop (`App::run`, `src/app.rs:1187-1279`)

Session heartbeat (`engine.touch_session`, `:1203`), 1 s `tick`, dashboard
command drain (`:1224`), agent event drain, dashboard publish throttling
(FR-530), Cursor pre-warm, to-do runs, graph executor ticks, workspace switch
restart (`main.rs:678-720`), `shutdown` (`:1281`). None of it needs a terminal;
all of it moves to the session host (FR-1001).

### 3.8 In-flight features that must land on the dashboard too

| PR | Feature | Dashboard requirement already in that spec | Check here |
| --- | --- | --- | --- |
| #66 grok runner (merged a4c50ae) | `grok` preset in pickers, color `#6366f1`, logged-out state, graph routing | SPEC-grok-runner FR-896, FR-898 (TUI and dashboard chip); verified on the dashboard under `--web` (P10) | FR-1021: must also appear under interactive `serve` (TC-1000) |
| #70 TUI picker width (merged (f1b36ae), v0.2.2) | TUI-only layout fix | none needed | P11 n/a |
| #71 resizable sidebars (open, v0.2.2) | dashboard-only; no TUI counterpart | SPEC-sidebar-resize | dashboard ahead of the TUI; keep when the TUI goes |
| #67 plan gate | judge line, plan approval panel (approve / edit / reject), `/plan` and `/direct`, route selector | SPEC-plan-gate UX-930, UX-932, UX-933, FR-945 | FR-1021 |
| v0.2.3 #2, #3 | diff review sidebar, world model (INTENT/MISSION) | specs not written yet; must be dashboard-first | FR-1021 |

### 3.9 Summary

| Area | full | partial | missing | n/a |
| --- | --- | --- | --- | --- |
| Run modes (R1-R5) | 1 | 3 | 1 | 0 |
| Slash commands (S1-S17) | 14 | 3 | 0 | 0 |
| Keys (K1-K11) | 4 | 4 | 1 | 2 |
| Mouse/paste (M1-M6) | 0 | 0 | 2 | 4 |
| Panels (P1-P11) | 6 | 3 | 1 | 1 |
| Settings (C1-C4) | 2 | 0 | 1 | 1 |
| **Total (54 rows)** | **27** | **13** | **6** | **8** |

C4 is counted as full (it works under `--web`); its `serve` gap is R1.
Dashboard-only features with no TUI counterpart (not counted): workspace switch dialog, Docs tree/preview/edit, to-do board, Memory panel, MCP import, graph swimlane, resizable sidebars (#71).

Key gaps: R1 headless interactive serve; image attach from the browser
(K8, M5, P5, S12); clickable file links (M2); activity groups, `/details`
and `[ui]` settings (P2, S13, C2, K5); status-bar detail (P4); graph gate controls (S16, P9);
keyboard shortcuts (K4-K7); the `--web` command channel tied to the to-do DB
(R3).

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1000 | Must | Given `dmuxai serve` with no TTY (stdin/stdout redirected, `setsid`) | Then the server starts the session host (built like `App::new_interactive`, so Cursor pre-warm stays on; `App::new` turns it off, `app.rs:891`), prints the dashboard URL, writes `.dmuxai/web.json`, pushes config warnings into the transcript as startup notices as well as stderr, and `POST /api/prompt` (202 Accepted) and `POST /api/command` (200 with `session`) behave as under `dmuxai --web`, with the transcript streaming in snapshots; workspace switch re-execs `serve` with the same port | `serve --read-only` (OQ-1000) is exactly today's `serve` (prompt/command 409, read-only Docs, to-dos and memory writable). A second interactive `serve` in the same workspace exits 2 with the running URL when `.dmuxai/web.json` names a live pid whose `GET /api/state` answers within 1 s; a dead pid or no answer counts as stale and is overwritten | TC-1000, TC-1001 |
| FR-1001 | Must | Given the session host | Then one terminal-independent loop runs heartbeat, tick, command drain, agent event drain, publish throttling (FR-530), pre-warm, to-do runs, graph ticks and shutdown; `App::run` (TUI) calls the same step function and only adds input polling and drawing | The host never touches crossterm or ratatui; it builds and runs with no terminal attached | TC-1002, NFR-1000 |
| FR-1002 | Must | Given Ctrl+C or SIGTERM to `serve` | Then running turns are cancelled with the existing process-group kill, running to-dos are finished as the TUI shutdown does (`app.rs:1281-1310`), and `web.json` is removed | A second signal within 2 s exits immediately | TC-1003 |
| FR-1003 | Must | Given the to-do DB fails to open | Then the dashboard command channel still exists: prompts, pickers and conversations work; only `/api/todos*` answer 503 with the error | — | TC-1004 |
| FR-1004 | Must | Given `/quit` or the Quit button under interactive `serve` | Then after the confirm dialog the server shuts down as in FR-1002 and the page shows "DmuxAI stopped" | — | TC-1005 |
| FR-1010 | Must | Given the dashboard prompt box | Then an image can be attached by file picker, drag-and-drop, or pasting an image (clipboard) or a single image path. `POST /api/attachments` first validates like `/image` (extension and sniffed type png/jpg/jpeg/webp, at most `DEFAULT_MAX_IMAGE_BYTES` 10 MiB, `src/attachments.rs:9`, `:81-113`), then writes the bytes into `attachments_dir` through one shared helper that also replaces the PNG-only, unvalidated `save_attachment` (`src/clipboard_image.rs:78-84`). The snapshot lists `session.pending_images[]` (`id`, `name`, `bytes`) and the prompt box shows chips; `SessionCommand::RemoveAttachment { id }` removes one | Unsupported type or oversize → 400 with the TUI's message; unknown attachment id → 404; uploads need the dashboard token; non-loopback binds accept uploads only with `--allow-remote` (already required) | TC-1010, TC-1011 |
| FR-1011 | Must | Given activity in the transcript | Then the snapshot carries activity groups (`id`, `kind` tool/thinking, `summary`, `lines`, `expanded`) built by the same core function the TUI uses. State model (same as `render_activity_group`, `app.rs:5699-5778`): a group is shown expanded when its own flag is set, or `details_expanded` is on, or `show_tools = expanded`; `show_tools = hidden` omits tool groups; `show_thinking = live` includes thinking items while they stream and after, `collapsed` includes them only once finished (`finished_ms` set, `app.rs:5721-5725`), `hidden` omits them. Click toggles the group's own flag (`toggle_group {id}`); `toggle_details` flips `details_expanded`; `toggle_last_details` (Ctrl+O) flips the last group's own flag | Hidden groups are omitted from the snapshot | TC-1012 |
| FR-1012 | Should | Given a workspace file path or link in an agent message | Then the snapshot marks it as a link (same detector as `open_file_link_at`) and the dashboard opens it in the Docs preview (OQ-1004); http(s) URLs are links too (new-tab behavior checked in the manual browser run) | Paths outside the workspace are not linked (`docs::confine`) | TC-1013 |
| FR-1013 | Should | Given the selected agent | Then the dashboard header shows the same status segments as the TUI status bar: badge (state, spinner, elapsed), model, mode, effort, current activity detail, other agents' activity and the graph assignment line, by moving the existing `StatusSegments` / `fit_status_segments` (`app.rs:5938-5951`) into the core and reusing them, not a second API | Narrow widths truncate in the same order as `fit_status_segments` | TC-1014 |
| FR-1014 | Must | Given a graph waiting at a sensitive-step gate | Then the graph view shows the gate reason (the hint no longer mentions the TUI) and the allowed-runners line, with **Continue** and **Override…** (runner + model pickers limited by `allowed_runners` and model policy), calling the same core paths as `/graph continue` and `/graph override` | No gate → buttons hidden; disallowed runner or model → 403 with the policy message | TC-1015 |
| FR-1020 | Must | Given the parity table in `src/web/parity.rs` | Then it gains rows for keys, mouse, panels and settings with status `full`/`n-a`/`pending`; `tui_features_have_dashboard_controls` fails when a `TUI_FEATURES` entry has no row or a `full` row's control id is missing from the assets. Because `TUI_FEATURES` is hand-written, a second test scans `handle_key`, `handle_mouse` and `handle_paste` in `src/app.rs` for `KeyCode::`/`MouseEventKind::` arms and fails when one is not in `TUI_FEATURES` | `n-a` rows need a reason; `pending` rows are allowed until the removal PR, which requires zero `pending` | TC-1020 |
| FR-1021 | Must | Given features merged after this spec (grok runner #66, plan gate #67, diff review sidebar, world model) | Then each has a dashboard control and a parity row before the TUI removal PR may merge | The removal PR's checklist lists them | TC-1020, manual review |
| FR-1030 | Must | Given the deprecation release (OQ-1002) | Then plain `dmuxai` and `dmuxai --web` still start the TUI but print once: `The terminal UI is deprecated and will be removed in the next release; use dmuxai serve` | `DMUXAI_NO_DEPRECATION_NOTICE=1` hides it | TC-1030 |
| FR-1031 | Must | Given the removal release | Then `ratatui`/`crossterm` TUI code (`App::draw*`, pickers, `handle_key`, `handle_mouse`, `chat_selection.rs`, TUI `theme` rendering, `ui_png.rs` if only used for TUI shots) is deleted; `dmuxai` with no arguments behaves per OQ-1001; `--web` prints a one-line migration error and exits 2 | `cargo tree` no longer lists `ratatui`; `crossterm` stays only if another module needs it | TC-1031 |
| FR-1032 | Must | Given the removal release | Then TUI-only settings (`[theme]` TUI colors, `force_color`) load with a deprecation warning and are otherwise ignored; README, `docs/API.md`, help text and the welcome message stop mentioning TUI keys | Unknown-key warnings follow the existing config warning format | TC-1032 |
| UX-1000 | Should | Given the dashboard | Then keyboard shortcuts exist for cancel, model picker, conversations and last-details toggle (OQ-1003), are listed in the palette hints, and never fire while typing in a text field except cancel | — | TC-1016 |
| NFR-1000 | Must | Given the session host refactor | Then behavior under `dmuxai --web` is unchanged: all existing tests pass without edits to their oracles, and `src/app.rs` loses the loop body rather than duplicating it | — | TC-1002, CI |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1000 | FR-1000 | integration | `serve` with test adapters, no TTY | `POST /api/prompt "hi"`; `POST /api/command select_agent` | 202; 200 with `session`; snapshot transcript gets the agent reply; `web.json` written | `src/main.rs` or `tests/serve_interactive.rs` `serve_runs_prompts_without_tty` |
| TC-1001 | FR-1000 | integration | `serve --read-only`; a second `serve` | prompt; start second | 409; second exits 2 naming the first URL; with a stale `web.json` (dead pid) the second starts | `serve_read_only_and_single_instance` |
| TC-1002 | FR-1001, NFR-1000 | unit | App with test adapters, injected clock, recording hub | drive `step()` for 6 s simulated with a busy agent | `touch_session` called at 5 s, `tick` once per second, publishes coalesced to ≥ 200 ms apart and at least once per second while busy, a completed turn publishes immediately; no crossterm call | `src/app.rs` `session_host_step_matches_tui_loop` |
| TC-1003 | FR-1002 | integration | busy agent under `serve` | SIGTERM | child process group killed, to-do finished, `web.json` removed | `serve_shutdown_cancels_and_cleans_up` |
| TC-1004 | FR-1003 | unit | to-do DB path unwritable | `POST /api/command select_agent` | 200; `/api/todos` 503 | `command_channel_without_todo_store` |
| TC-1005 | FR-1004 | integration | `serve` | `quit` command | server exits cleanly | `serve_quit_stops_server` |
| TC-1010 | FR-1010 | integration | `serve` | upload png; upload `.exe`; oversize | 200 + `pending_images`; 400; 400 | `src/web/server.rs` `image_upload_contract` |
| TC-1011 | FR-1010 | unit (asset) | `session.js` | static checks + manual browser run (REVIEW) | file input, drop and paste handlers, chip remove control | `src/web/assets.rs` `session_module_has_image_attach` |
| TC-1012 | FR-1011 | unit | App with tool and thinking events; `[ui]` variants | build snapshot; toggle last; toggle all | literal expected group vectors per `[ui]` variant (pinned in the test, not computed by the builder); hidden omitted; toggles flip `expanded` | `dashboard_activity_groups_match_tui` |
| TC-1013 | FR-1012 | unit | messages with an in-workspace path, an outside path, a URL | render links | only the in-workspace path and URL are linked; rendering checked in the manual browser run | `dashboard_transcript_links_confined` |
| TC-1014 | FR-1013 | unit | busy Codex + idle Cursor + single graph plan | build status view | pinned literal segment strings and order at widths 120 and 60 | `dashboard_status_segments_match_tui` |
| TC-1015 | FR-1014 | integration | graph paused at a gate | Continue; Override allowed; Override disallowed | resumes; node reassigned; 403 | `graph_gate_controls_contract` |
| TC-1016 | UX-1000 | unit (asset) + browser run | `session.js` | static checks; REVIEW browser run | shortcuts present and listed | `session_module_has_shortcuts` |
| TC-1020 | FR-1020, FR-1021 | unit | `TUI_FEATURES`, `PARITY` | compare | every feature mapped; control ids exist; `n-a` rows have reasons | `src/web/parity.rs` `tui_features_have_dashboard_controls` |
| TC-1030 | FR-1030 | unit | deprecation build | start TUI path | notice printed once; env var hides it | `tui_deprecation_notice` |
| TC-1031 | FR-1031 | CI | removal build | `cargo tree -i ratatui`; `dmuxai --web` | not found; exit 2 with migration line | release audit in REVIEW |
| TC-1032 | FR-1032 | unit | config with `[theme]` | load settings | deprecation warning; no effect | `tui_theme_settings_deprecated` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| CLI | `serve` interactive by default (OQ-1000); `serve --read-only`; later `--web` removed | Until removal, `--web` keeps working; rollback = ship `serve --read-only` as default | TC-1000, TC-1001 |
| Security | All new routes (image upload, gate controls) use the #27 token, Host and Origin checks; uploads are size-capped and type-checked; non-loopback binds still need `--allow-remote` | Unchanged auth model | TC-1010, TC-1015 |
| Persistence | Uploaded images go to `attachments_dir` (same as clipboard paste); no schema change | — | TC-1010 |
| Processes | Interactive `serve` owns agent child processes; shutdown uses the existing process-group kill | Same as TUI shutdown | TC-1003 |
| Snapshot | New optional fields: `session.pending_images`, `session.activity_groups`, `session.status`, transcript message `links[]`, `graph.gate_reason` (next to the existing `graph.waiting_gate_node`, `src/web/snapshot.rs:206`) | Older pages ignore them | TC-1012, TC-1014 |

## 7. Implementation plan

See [PLAN-dashboard-full-parity](../plans/PLAN-dashboard-full-parity.md) and
[TASKS-dashboard-full-parity](../tasks/TASKS-dashboard-full-parity.md).

## 8. Review and test exit criteria

- [ ] Dongjin reviewed the §3 gap list and answered OQ-1000..1004.
- [ ] All Must requirements map to passing evidence.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` (Linux and Windows target), `cargo test --all-targets` pass.
- [ ] Light-theme screenshots of interactive `serve`, image attach, activity groups, status header and gate controls attached to each PR.
- [ ] `docs/reviews/REVIEW-dashboard-full-parity.md` records the free-model reviews, commands, residual risk and rollback notes.

## Appendix: audit evidence

An independent row-by-row inventory (87 finer-grained rows) was produced by Grok Build
(grok-4.7) and is kept in [research/dashboard-parity-audit](../research/dashboard-parity-audit.md).
Its findings that changed §3: `/details` does not expand in the browser (S13), the
gate hint text names the TUI (S16), Shift+Enter is not a TUI key (K2), and the
background-conversation notice slot (P4).
