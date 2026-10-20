# Changelog

All notable changes to dmuxai are listed here. Versions follow
[Semantic Versioning](https://semver.org/); dates are Asia/Seoul.

## Unreleased

### Changed

- **Dashboard center column uses the full width (SPEC-dashboard-fullwidth FR-1200..FR-1204).**
  The chat, graph and task checklist now fill the whole area between the
  sidebars instead of an 880px column, and the Documents panel is collapsible
  and collapsed by default.
- **ChatGPT-style dashboard chat (FR-1205..FR-1208).** The chat fills the
  window height with its own scrolling transcript, a rounded composer pinned at
  the bottom (Send inside), right-aligned user bubbles, plain agent text with
  styled code blocks, and a centered ~860px reading column.

- **Diff and Graph open from header buttons (FR-1211..FR-1216).** The change
  list (Diff) and the graph run (swimlane, plan approval, task checklist) are no
  longer inline in the center. `Diff` and `Graph` buttons next to To-dos open
  them over the center area (Esc or a second click closes; kept per tab). The
  Graph button shows `!` while a plan waits for approval and a dot while a run
  is active; a pending plan opens the Graph view once, and the chat always
  shows an inline approval card.

- **Agent processing inline in the chat (FR-1217..FR-1221).** Compact, muted,
  collapsible one-line rows: a live "Running <step> on <runner> · 2/8 steps ·
  29s" row whose steps open the Graph view at that node; "N actions" rows that
  fold the turn's tool activity; per-file rows on the document card that open
  Diff at that file; and an inline plan approval card with Approve / Edit /
  Reject. Line counts (+N/−M) are not shown because change records do not
  carry them.

- **Graphs follow the conversation (FR-1222..FR-1224).** Opening or switching
  to a conversation shows its latest stored graph run again (read-only; never
  another conversation's; a live run still wins; a run left mid-run shows as
  cancelled and is not resumed). Every chat exchange that started a run gets
  its own collapsed run row, and "Open in Graph" on an earlier run loads that
  run into the Graph view (`show_graph_run` command).

### Fixed

- **Agent replies no longer show twice (FR-1209, FR-1210).** The Documents
  panel printed each turn's output tail, which is the same text as the chat
  reply. Change records now carry the chat `message_id`; the reply stays in the
  chat with a compact document card ("Open in Diff"), and the Documents row
  keeps the changed files plus a one-line "Show in chat" reference.

## [0.2.2] - 2026-10-19

Dashboard layout, workspace and graph view changes requested by Dongjin on
2026-10-18, PRs #57-#60, #62-#66 and #70-#74 (milestone [v0.2.2](https://github.com/donggrri/dmuxai/milestone/2),
issues #54-#56 and #61, plus the grok runner Dongjin added on 2026-10-19). Linux is the supported platform. The Windows target
still compiles and passes clippy, but the live workspace switch returns 501 on
Windows (see ROADMAP "Later").

### Added

- **Built-in grok runner (SPEC-grok-runner FR-890..FR-900, SPEC-model-policy FR-910..FR-912, #66).** When `grok` is on
  `PATH`, `DMUXAI_GROK_BIN` names it, or `~/.grok/bin/grok` is executable, DmuxAI
  adds a headless `grok` chat runner (`grok -p` with streaming-json). Models are
  `grok-*` without `fast` and default to `grok-4.7`. A logged-out CLI still lists
  the runner and asks for `grok login`. `[agents] builtin_grok = false` hides it,
  and an `[[agents.custom]]` entry named `grok` replaces it. With
  `allowed_runners` unset, this built-in preset takes no graph nodes until
  `grok` is listed. A machine that has grok installed gains the chat runner and
  does not gain grok graph nodes until that list includes it. Model policy:
  any id containing `fast` is rejected for every runner, `grok-*` ids only on
  the grok runner, and Cursor stays `composer-2.5` only.

- **Horizontal swimlane graph view (SPEC-graph-swimlane FR-880..FR-885, UX-880, UX-881, #61).**
  The Graph view's subway map is replaced by a timeline: one lane per runner
  (with its latest model), time left to right with nice ticks, and each step as
  a card placed at its start and as wide as its duration. Close cards stack
  into sub-rows. A "now" line shades the future; running cards pulse and have
  a striped tail. Pending cards show `est. start +…` and `~duration` (median of
  finished steps of the same role), and a step after a failed dependency is
  marked `BLOCKED`. Dependency arrows never loop back over their source card,
  and their vertical legs avoid other cards. The detail panel is now an overlay
  drawer, so the timeline keeps its width. Fit never shrinks text below 11px.
  The light theme gets a dedicated pending color (`--graph-pending`, at least
  3:1). Graph nodes now persist `started_at_ms` / `finished_at_ms`.

- **Workspace directory selection and live switch (SPEC-workspace-dir FR-870..FR-875, NFR-870, #56).**
  A global `-C DIR` / `--workspace-dir DIR` flag and user-config `[workspace] dir`
  set the active workspace for every mode (flag > `DMUXAI_WORKSPACE` > user
  config `[workspace] dir` > current directory). Paths are canonicalized and
  validated (must be a writable directory and not `/`, the DmuxAI state/config
  dirs, or credential directories such as `~/.ssh`, `~/.aws`, `~/.gnupg`, or
  runner credential folders); invalid paths exit with code 2. The web dashboard
  header shows the `~`-abbreviated, middle-ellipsized workspace path as a button
  that opens `#workspace-dialog` with live validation (`POST /api/workspace/validate`),
  an MRU recent-paths list (`recent-workspaces.json`, max 10), and a directory-only
  browser (`GET /api/workspace/dirs`) confined to `$HOME` and `[workspace] browse_roots`.
  Confirming a switch (`POST /api/workspace/switch`) guards against busy turns,
  performs a preflight check, cleanly shuts down warm processes and session
  state, and on Unix `exec`s `dmuxai` into the new workspace on the same port,
  bind address and token while the browser reconnects automatically.

- **Left sessions sidebar and running conversation marks (SPEC-session-sidebar FR-860..FR-864, UX-860, #55).**
  The conversation list moves from the in-panel box into a collapsible 320px
  left sidebar (`#sessions-sidebar`, header toggle `Sessions`) with a filter
  input and `New` button. Each row shows the conversation title, last agent,
  relative updated time (with ISO timestamp tooltip), and a reduced-motion-aware
  spinner while a turn is running; the active conversation is listed first with
  `aria-current="true"`. Collapse state persists per workspace in `localStorage`
  (default open above 1200px, overlay drawer at 900px and below). Snapshot
  `SessionConversation` entries gain `last_agent`, `turn_count` and `running`
  (capped at 200, 2s cache TTL while any conversation is running), and the TUI
  conversation picker (`F3` / `/conversations`) shows a `● running` mark on
  in-flight conversations.

- **Resizable dashboard sidebars (SPEC-sidebar-resize FR-970..FR-974, UX-970..UX-971, #71).**
  On viewports wider than 900px, the left sessions sidebar and the right
  Files/To-dos sidebar each get a drag handle on their inner edge (pointer,
  pen or touch). Widths are clamped to 180px..min(50% of the viewport, the
  space left after the other sidebar and a 360px content area), re-clamped on
  window resize, and stored in `localStorage` (`dmuxai.sidebar.width.left` /
  `.right`). The prompt box stays centered between the open sidebars at any
  width. Handles are keyboard accessible (`role="separator"`, arrows 16px,
  Shift 64px, Home/End, Enter or double-click resets to 320px). At 900px and
  below the handles are hidden and the overlay drawers are unchanged.

### Changed

- **Centered prompt box across sidebar states (SPEC-centered-prompt FR-850..FR-852, UX-850, #54).**
  On viewports above 900px, the dashboard reserves space for open docked
  sidebars via `--left-dock` and `--right-dock` on `body` and keeps `main`
  (`max-width: 880px; margin-inline: auto`) and `.session-prompt` centered in
  the remaining content area instead of shifting off-centre with a fixed right
  margin. At 900px and below, sidebars stay overlay drawers and the prompt box
  remains centered in the viewport.

### Fixed

- **TUI conversation picker width (SPEC-session-sidebar FR-865, #70).** The F3
  picker was a fixed 76 columns, so the agent and time were cut off in wide
  terminals. It now takes about 90% of the terminal width (60 to 160 columns,
  never wider than the terminal or the rows need) and shortens long titles
  with `…`, so the running mark, id, turn count, agent and time stay visible
  at 80, 120 and 200 columns.

- **TUI picker rows never overflow (SPEC-session-sidebar FR-865, #73).** With a
  long custom agent label or a terminal under 60 columns, the agent and time
  were still clipped. Fields now give way in a fixed order (title, then the
  agent label with `…`, then the id, the turn count, the title, the agent, and
  `● running` becomes `●`), so the time and the running mark always fit.
  Control, zero-width and bidi characters in titles become spaces, so each
  conversation stays on one row.

- **Process-group kill on Unix (#63 for the MCP server, #64 for the rest).** `kill` now receives `--` before
  the negative process-group id when DmuxAI stops a custom agent, a Cursor
  warm process, the MCP server's children, or an Office conversion. Without
  it, some `kill` builds read `-<pgid>` as an option and left the group running.

- **grok turns no longer hang after the final reply (SPEC-grok-runner FR-913..FR-917, #72).**
  In grok 1.0.62, a background command that finished during a turn queued an
  auto-wake prompt that headless `grok -p` never ran, so the process idled
  forever after its `end` line. The grok preset now sets `GROK_AUTO_WAKE=0` and
  passes `--no-wait-for-background`. Custom agents gain `done_when`: the grok
  `end` line now ends the turn, and DmuxAI then stops the process group, so a
  child left holding stdout cannot keep the turn open. The 900 s timeout
  remains only as a last resort. Text before and after a tool call is now
  separated by a blank line instead of running together (`text_break_when`).
  [docs/agents/grok-build-delegation.md](docs/agents/grok-build-delegation.md)
  explains how to delegate work to Grok Build with fewer tokens.

- **grok tool calls show as activity (SPEC-grok-runner FR-980, #74).** grok's
  streaming-json `tool_call` / `tool_call_update` lines now become activity
  rows (`▸ read src/app.rs`, `▸ run cargo test ✓ 4.2s`) and the TUI status
  detail, the same way agy and cursor tools do. A long grok turn no longer
  shows only "running".
- **Dashboard shows the live activity (SPEC-running-indicators FR-981, #74).**
  `agents[].activity` carries the TUI status detail; the agent chip and the
  `Running` pill show it while the turn runs.
- **Cancelled turns are marked (SPEC-running-indicators FR-982, #74).** After a
  cancel (Ctrl+C, dashboard Cancel, `/api/command cancel`) the transcript gets
  one `<agent> turn cancelled.` line after the partial reply, for every runner,
  and the stored turn is `cancelled`. Before, the adapter's late `Cancelled`
  event was dropped and no line appeared.
- **Welcome text lists the registered runners (SPEC-first-run FR-983, #74)**, for
  example `Runners: cursor, agy, codex, grok.`, instead of a hardcoded
  "Codex, Cursor, and AGY".
- **The dashboard token stays in the TUI (SPEC-web-dashboard FR-984, #74).** The
  `dmuxai dashboard: http://…/?token=…` notice is TUI-only; it is no longer in
  dashboard snapshots, `/api/state`, SSE, headless output or agent context.

### CI

- Verify runs on `ubuntu-latest`, cancels superseded runs, and skips the Rust
  job for docs-only changes and for draft PRs (it runs when a PR is marked
  ready) (#63).

## [0.2.1] - 2026-10-18

Dogfood fixes from the 2026-10-17 Linux trial of v0.2.0, PRs #39 and #47-#52
(milestone [v0.2.1](https://github.com/donggrri/dmuxai/milestone/1), issues
#40-#46). Linux is the supported platform for this release. Windows builds
still compile, but the new features are not re-checked on Windows, and the
Cursor pre-start has no Windows job object yet (see ROADMAP "Later").

### Added

- **Built-in opencode runner (SPEC-first-run FR-810..FR-812).** When `opencode`
  is on `PATH` (or `DMUXAI_OPENCODE_BIN` names it) and no `[[agents.custom]]`
  entry is named `opencode`, DmuxAI registers an `opencode` runner. It uses the
  documented opencode command and the free-model policy
  (`allowed_models = "^opencode/.*-free$"`); the first allowed listed model is
  the default. Model listing times out after 10 seconds. A failed or empty
  listing still shows the runner, and selecting it or sending a prompt names
  the policy and how to override it. A user `[[agents.custom]]` named
  `opencode` replaces the preset; `[agents] builtin_opencode = false` hides it.
- **`dmuxai init [--workspace] [--force]` (FR-814).** Writes a commented starter
  `dmuxai.toml` to the user config, or to the workspace with `--workspace`. It
  lists the runners found on `PATH` and refuses to overwrite without `--force`.
- **Running indicators (SPEC-running-indicators FR-820..FR-824).** Each agent
  has a run state (`idle`, `running`, `done`, `failed`, `cancelled`) with start
  and end times, exposed in the dashboard snapshot and `/api/status`. The TUI
  status bar shows a spinner and live elapsed seconds while a turn runs, then
  `✓`, `✗` or `■`; `reduced_motion` shows a static mark. The web dashboard adds
  a header status pill, spinners on agent chips and running graph nodes, and a
  tab-title prefix.

- **Documents folder tree in a right sidebar (SPEC-docs-tree, #43).**
  `GET /api/docs/tree?dir=` lists one directory at a time, with the same
  confinement, ignore and symlink rules as the flat list (FR-830, FR-831). The
  dashboard has a toggleable right sidebar (header `Files` button) with a
  collapsible, lazily loaded tree; the path filter, preview and edit work as
  before. Expanded folders persist per workspace; it is keyboard accessible and
  becomes an overlay drawer below 900px (FR-832, FR-833, UX-830).

- **To-dos in the sidebar (SPEC-todo-sidebar, #44).** The to-do UI (list,
  add form, Jira field, MCP import) moved from the main column into a `To-dos`
  tab of the right sidebar, opened from a header `To-dos (n)` button whose badge
  counts open to-dos. The list can be filtered by state (open, done, all) and
  searched. Files and To-dos share the sidebar as tabs; the active tab persists
  per workspace, and Esc closes the sidebar unless you are typing or the
  preview is open (FR-840..FR-842, UX-840).

- **Cursor pre-start (SPEC-cursor-latency FR-805..FR-807).** In the TUI and
  dashboard, DmuxAI starts the next `cursor-agent` process while you type and
  hands it the prompt on send, saving an indicative ~1.4–2.1 s of measured
  latency per turn (depending on typing and idle time between turns). The process
  is reused only when its program, arguments, working directory and environment
  match, and is reaped on a mode or model change, `/new`, idle expiry
  (`[cursor] prewarm_idle_secs`, default 300) or quit. Headless `dmuxai run` and
  graph nodes never pre-start; `[cursor] prewarm = false` turns it off.

### Changed

- **Graph routing exception for the built-in opencode runner (FR-813).** With
  `[graph] allowed_runners` unset, every runner may take graph nodes except the
  built-in opencode preset. List `opencode` in `allowed_runners` to allow it.
  Your own `[[agents.custom]]` agents keep the previous rule. Single-agent runs
  and TUI or dashboard selection are unchanged. Existing v0.2.0 users who have
  `opencode` on `PATH` and no `opencode` custom agent will see a new `opencode`
  runner in chat.
- `[agents]` without `[[agents.custom]]` (for example, only
  `builtin_opencode`) no longer clears the custom agents from the other config.

### Fixed

- **Cursor text shown twice (SPEC-cursor-stream-dedupe FR-800, FR-801, #40).**
  With `--stream-partial-output`, cursor-agent repeats each text segment as an
  aggregate event before a tool call. DmuxAI showed that segment a second time
  in the TUI and the dashboard. Aggregates now add only text that was not
  already streamed; streams without partial deltas show each segment once.

### Docs

- README: new "First run" section covering the built-in runners, opencode
  detection and its free-model policy, and `dmuxai init`.
- README: the Linux/WSL install steps match the release archive
  (`dmuxai-linux-x86_64/{dmuxai,README.md,CHANGELOG.md}`), with
  `mkdir -p ~/.local/bin`, the `install` command, a `tar --strip-components=1`
  one-liner, and a PATH note.

## [0.2.0] - 2026-10-17

Changes since v0.1.1 (2026-08-24), PRs #1-#38.

### Added

- **Local web dashboard (slices A-H).** `dmuxai --web` runs the TUI with a
  dashboard beside it. `dmuxai serve` runs the dashboard view-only. The
  dashboard binds to 127.0.0.1, needs a per-start access token, and rejects
  foreign Origin/Host. Binding elsewhere requires `--allow-remote`, which
  prints a warning.
  - **Slice A, live graph (#27):** running, done and pending nodes with
    runner and model, live elapsed time, and agent busy chips over SSE.
  - **Slice B, to-do board (#29):** persistent per-repo to-dos with a CRUD
    API. A to-do can run as a single agent turn or as a graph, and its status
    follows the run.
  - **Slice C, shared MCP servers (#31):** a merged `[[mcp.servers]]` config
    from user and repo files, passed to every runner (codex, cursor,
    opencode, custom agents). Jira/Confluence import into to-dos, Jira key and
    link strips on cards, secrets redacted in `/api/mcp/servers`.
  - **Slice D, themes (#30):** dashboard light/dark/system with an accent
    color. TUI background, foreground, border and per-agent colors, shared
    with the dashboard.
  - **Slice E, Obsidian memory (#33):** `[memory]` vault integration with
    `/remember` and `/memory`, an approval queue for agent-proposed notes,
    memory context injection, and a `dmuxai mcp-memory` stdio server.
  - **Memory MCP auto-registration (FR-733, #38):** when `[memory]` is
    configured, a built-in `dmuxai-memory` entry is added to the shared MCP
    server list, so every runner can use the vault. A user or repo entry with
    the same name overrides it, and `enabled = false` turns it off.
  - **Slice F, document preview and edit (#34):**
    - workspace file browser and per-turn changed-files feed;
    - clickable file links in agent output and in the TUI (OSC 8);
    - GitHub-style Markdown with a split edit view, saved with a sha256
      conflict check;
    - HTML preview in a sandboxed iframe;
    - Office files rendered through headless LibreOffice.
  - **Slice G1, agent API (#35):**
    - headless `dmuxai run|todo|status|events` with `--json` and stable exit
      codes;
    - `POST /api/prompt` and the token HTTP API, documented in
      `docs/API.md`;
    - a stdio MCP server (`dmuxai mcp`);
    - state-change events in `.dmuxai/events.jsonl`, available over SSE and
      `GET /api/events/log?since=`;
    - optional `[notify]` webhooks and command hooks, with stall detection.
  - **Slice G2, dashboard parity (#36):** a chat panel with a streaming
    transcript and a prompt box, plus agent/mode/model/effort pickers. Every
    slash command is reachable from a button or the Ctrl+K palette, along
    with cancel, new/load conversation and share (`POST /api/command`). A
    parity test fails when a TUI command has no dashboard control.
  - **Slice H, graph view (#32):** the graph run drawn as a subway map, with
    a layered layout, dependency edges, and a per-node log tail and changed
    files.
- **Custom agents (#8):** `[[agents.custom]]` attaches any headless agent CLI
  (command template, prompt via argument or stdin, JSONL or text output,
  resume, per-run temp config). The built-in opencode presets use it.
- **`/compact` (#9):** an alias for `/summarize`, with an optional focus
  argument.
- **Graph on by default, with a live status line (#11).** `[graph]
  allowed_runners` and a user-level config at `~/.config/dmuxai/dmuxai.toml`
  (#19). The plan header shows the effective model per runner (#23).
- **Tool and thinking activity display (#13):** collapsed by default,
  toggled with Ctrl+O and `/details`, and configured with `[ui] show_tools`
  and `show_thinking`.
- **Windows:**
  - WSL launcher `dmuxai-launcher`, with environment checks, path
    conversion, and argv-only launch (#14).
  - A zip package with per-user install/uninstall and a Start-menu shortcut
    (#15).
- **Per-repo conversation DB and `DMUXAI_STATE_DIR` (#20).**
- **Spec tooling:** a SPEC index generator, plus cross-spec ID uniqueness
  checks (#22).
- **Agent test helpers (#25).**
- **Earlier work since v0.1.1:**
  - agent-graph orchestration and multi-model policy (#4)
  - speaker colors and `[theme]` (#5)
  - image attachments (#3)
  - MISSION.md and permission hardening (#2)
  - the spec-driven test plan (#7)
  - docs (#1, #10)

### Changed

- **Cursor runner:** always pinned to `composer-2.5` (#16).
- **AGY:**
  - idle timeout
  - a foreground-only Implement prompt (#21)
  - a terminal lifecycle fix (#26)
- **Release workflow:** now also builds a static (musl)
  `dmuxai-linux-x86_64.tar.gz` with a SHA-256 checksum for WSL and Linux users. It is published to the same
  GitHub release as the Windows zip. The tarball holds only `dmuxai` (run it
  inside WSL or on Linux). The Windows zip adds `dmuxai-launcher.exe`, which
  opens DmuxAI inside WSL.
- **Docs:**
  - README section "Calling dmuxai from an agent", with Claude Desktop MCP
    examples for Linux and for Windows via WSL (not yet tested with Claude
    Desktop itself);
  - refreshed dashboard screenshot and an updated code map;
  - personal paths removed from the README and review notes;
  - SPEC, PLAN and TASKS statuses for the v0.2.0 slices, recorded in
    `docs/reviews/REVIEW-v0.2.0-release.md`.

### Fixed

- **TUI stabilization (#12):**
  - `--help` works without a TTY
  - the status line fits the terminal
  - the AGY model cache
  - Enter after a paste
- **Slash commands (#18):** command parsing, an unknown-command guard, and
  the redraw tick.
- **Implement mode (#26):** plan-only detection, and cancelling while busy.
- **F2 model selection (#6):** the model is honored per runner, and colors
  can be forced under `NO_COLOR`.
- **Event log on Windows (#37):** an append no longer fails when it finds the
  `.dmuxai/events.lock` file pending deletion by another handle (Windows
  reports "Access is denied"). It now retries.
- **Builds:**
  - the non-Windows `dmuxai-launcher` stub (#17)
  - bash-only spec-index tests are gated on Windows (#24)

[0.2.2]: https://github.com/donggrri/dmuxai/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/donggrri/dmuxai/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/donggrri/dmuxai/compare/v0.1.1...v0.2.0
