# Changelog

All notable changes to dmuxai are listed here. Versions follow
[Semantic Versioning](https://semver.org/); dates are Asia/Seoul.

## Unreleased

### Added

- **Persistent terminal sessions: `dmuxai term` (PR1a of SPEC-terminal-sessions,
  FR-2600..2609, FR-2620, FR-2621, FR-2632).** Named Linux PTY sessions (`shell` or `cmd`)
  owned by a per-workspace session host on a private Unix socket: `open`, `send`
  (text or named keys), `read`, `wait` (regex, idle, timeout), `exec` (exit code via a nonce
  marker), `list`, `show`, `resize`, `close`, `reopen`, all with `--json`. Every input and
  output event goes to a masked local journal under `terms/`. `dmuxai run` never starts the
  host. Agent integration and the dashboard view follow in later PRs.
- **Terminal sessions PR1b (FR-2610..2616, FR-2630).** `term open --serial DEV --baud N`
  opens the device with serial2 (parity/data/stop/flow; no extra tool). Missing, permission,
  and busy failures use distinct `error_kind` values and the FR-2611 hint text (never
  `chmod 666`). WSL1 maps `COMn` to `/dev/ttySn`. On WSL2 a missing path runs
  `usbipd.exe attach --wsl --busid <id>` once (id from `--usbipd-busid` or trusted
  `[terminal.serial] usbipd_busid`; never auto-selected from `usbipd.exe list`).
  Without an explicit busid, attach is skipped. `--no-usbipd-attach` (or trusted
  `[terminal.serial] usbipd_attach = false`) disables attach even when a busid is set.
  Unplug (EIO/ENXIO) moves the
  session to `lost`; `term reopen` drops the old file descriptor before retrying.
  `open --wait-for REGEX --timeout` waits for a banner. `term wait` without `--since`
  starts at the current seq so older buffer text cannot match. `term exec` `text` is only
  the command output (no echo, nonce marker, or trailing prompt). A seatbelt (not a
  sandbox) blocks default-denied `send`/`exec` text until `term approve <id>` (refused
  when `DMUXAI_RUN_ID` is set); trusted `[terminal.policy]` can add allow/deny patterns
  and `approve_timeout`. `scripts/bench-term.sh` prints p50/p95 for status, send+read,
  and exec versus `bash -c`.
- **`dmuxai term --help` and `dmuxai term open --help` print term-specific usage (FR-2617, #309),**
  not the root help. `dmuxai --help` and `dmuxai run --help` are unchanged.

### Fixed

- **`term open --serial` no longer auto-attaches the first `usbipd list` device (#308).**
  Without an explicit busid, dmuxai does not invoke `usbipd.exe` (no `list`, no `attach`)
  and exits with `error_kind` `usbipd_busid_missing` plus a hint to pass `--usbipd-busid`.

## [0.3.1] - 2026-11-09

### Added

- **Busy headless runs are distinguishable from stalled ones (#166, FR-2430..2433).**
  `dmuxai run --progress` now emits `tool` lines for opencode/JSONL tool events,
  `file_edit` lines with workspace-relative paths (never file content), and a
  `heartbeat` line about every 30 s with turn age, last-output age and the last
  JSONL `type`/`event`. Custom `[[agents.custom]]` runners can map their own tool
  lines with `tool_when`, `tool_name_path` and `tool_title_path`.
- **Task header and sidebar agent badge (FR-2331, FR-2341).** The dashboard and
  TUI list the agents that have spoken in the active task and mark those with an
  `active` native session live; the existing agent picker still switches agents.
  Sidebar task rows show the task's stored agent and model as a short badge
  (falling back to the last agent) and list spoken agents in the tooltip.
- **Graph node detail shows tool activity and stall markers (#228, FR-2420, FR-2422).**
  Dashboard graph node details reuse `ActivityItem`/`ToolStatus` to show
  the current tool or command summary (first line, max 120 chars), elapsed time,
  and the last 3 output lines; a muted "no new output for N min" marker appears for
  running nodes whose log tail and activity are unchanged for 5 minutes without
  stopping or failing the node. The TUI keeps its one-line graph status.
- **Test and verify nodes cannot wait forever (#228, FR-2421).** Graph `test` and
  `verify` node prompts now require confirming that external work was accepted (via
  a job id or queue response) before waiting, bound every wait/poll loop to about
  10 minutes, and fail with a clear message instead of waiting indefinitely.
- **Jira tickets in the To-dos sidebar, read-only (SPEC-jira-todos, FR-2400..2408,
  UX-2409..2411).** A `[jira]` table in `dmuxai.toml` (`base_url`, `email`, `jql`,
  `max_results`, `token_env`) lists matching Jira Cloud issues in a collapsible
  **Jira** group with Open in Jira, Start task (prefilled, unsent draft), and Copy to
  to-do. The API token comes only from the `DMUXAI_JIRA_TOKEN` environment variable
  (or the one named by `token_env`) and never reaches config, logs, or the browser.
- **Agents keep their own native session inside a task (FR-2323..FR-2326).**
  When another agent adds completed turns, the original agent resumes its own
  provider session and receives only the unseen turns, labelled with source
  agent and model, inside the existing untrusted shared-context envelope.
  Deltas over 200 turns or 65,536 UTF-8 bytes, stale or retired bindings, and
  provider-rejected resume ids fall back to a fresh session with the bounded
  projection; rejected resumes are retried once. `known_through_seq` advances
  only after a completed turn is saved, so failed, cancelled and interrupted
  attempts re-deliver the same delta.
- **Tasks remember their picker (FR-2322).** Each conversation stores its last
  selected agent, model and reasoning effort; selecting it in the TUI (F3,
  `/conversations`, `/resume`) or dashboard restores those values after runner
  availability and model-policy validation, falling back to the global default
  with a notice when they are no longer available. The dashboard snapshot also
  exposes each conversation's `settings`.
- **Gate approval banner in the session chat (FR-1985).** While a graph run waits at
  a sensitive-step gate, the dashboard session view shows a banner above the composer
  naming the node and role, with **Continue** (`graph_continue`) and **Cancel graph**
  (`graph_cancel`) buttons, so queued prompts no longer look stuck.
- **One dashboard process for several folders (SPEC-multi-workspace-host,
  in progress).** Dashboard API requests take an optional `ws=<id>` query
  parameter to pick an open folder, and `GET /api/workspaces` lists the open
  folders with their running state (FR-2300..2302).
- **Open another folder without stopping running turns (FR-2303..2306).**
  In headless `dmuxai --web`, switching to another folder opens it inside the
  same process (up to 4 folders) instead of restarting dmuxai, so it is no longer
  refused while a turn or graph runs elsewhere. Switching to an open folder only
  focuses it; the tab follows with `?ws=<id>`.
- **Open and running folders in the sidebar (FR-2307, FR-2308).** Folder
  groups that are open in this dashboard are marked, a spinner shows folders
  with a running turn, and a `+` button starts a new conversation in that
  folder.
- **Close a folder and confirm Quit (FR-2309..2311).** `POST
  /api/workspaces/{id}/close` stops one open folder (running folders need
  `force`), the sidebar has a close button per open folder, and Quit lists the
  folders whose turns it would stop.
- **Untrusted folders open restricted (FR-2312, FR-2313).** A folder opened in
  the dashboard whose `dmuxai.toml` needs trust and is not in the workspace
  trust store (`dmuxai trust`) has its command, secret and URL settings
  ignored, and shows a warning and a `restricted` badge.
- **A tab for a closed folder falls back to the first folder (FR-2314).** If a
  dashboard tab's `ws` folder is no longer open, the page reloads once on the
  first folder instead of polling a missing folder.
- **Git diff for any text file in the doc viewer (FR-2260..FR-2262).** Files with an
  unrecognized extension (for example Yocto `.bbappend`) are now previewed as plain text
  when their content is text, decided like git (no NUL byte in the first 8000 bytes).
  A new read-only `GET /api/docs/diff?path=` returns the exact `git diff HEAD` output for
  one file (untracked and deleted files included, 2 MiB cap), and the viewer's **Diff**
  toggle shows it line for line with **Copy raw**. Files opened from the Diff view open
  straight in their diff, and real binaries show git's `Binary files ... differ` line.

- **Dashboard settings UX (FR-2360..FR-2369).** Run controls grouped next to the
  prompt box, consistent settings rows, a small button system with busy state,
  inline confirm for destructive actions, and accessibility improvements.

### Changed

- Workspace `dmuxai.toml` settings that run commands, read secret environment variables, or set outbound URLs now require `dmuxai trust` (pinned to the file hash); headless runs opt in with `DMUXAI_TRUST_WORKSPACE_CONFIG=1`.
- Hardened workspace config trust checks (path confinement, trust store permissions, revoke, and clearer approval prompts).
- Saving workspace settings from a write-capable dashboard keeps an existing trust record current when the file on disk still matches it; untrusted folders and one-process opt-in stay untrusted (FR-2226..FR-2231).
- Dashboard saves of the workspace `dmuxai.toml` (stage skills, agent prompts) now hold a per-folder lock and write the file atomically, so saves made at the same time no longer drop each other's changes (FR-2242, FR-2243).
- Skill and memory paths inside the workspace are confined to it in every trust state; trust output escapes control characters; the trust state directory permissions are checked (FR-2234..FR-2241).

### Fixed

- **opencode compaction no longer leaks into the reply or fakes success (#185, FR-2440..2443).**
  The opencode preset skips synthetic parts (`text_skip_when`), drops the
  auto-compaction summary of the open message when opencode's compaction
  continuation arrives (`compaction_when`, grouped by `message_id_path`) and
  shows a `context compacted` notice instead. A turn whose stream ends right
  after a compaction without a final answer now fails with `error_kind`
  `compacted_unfinished` (no runner fallback) instead of reporting `ok: true`
  with the summary as the answer. Custom runners without the new options are
  unchanged.
- **Dashboard Stop no longer leaves a stale chat (#155, FR-1237).** The reply of
  a session command (Stop, queue remove/edit, picker) could arrive after a newer
  live snapshot and overwrite it, so the next follow-up kept showing as queued
  with no run active until something else changed. The reply is now ignored when
  a live snapshot was applied while it was in flight. This was the root cause of
  the flaky `web_followup_queue` browser test, which now also holds the next
  follow-up open until its `after_stop` check (no wall-clock race).
- **`web_chat_single_send` no longer fails when the browser driver stalls (#251).**
  The sampling window used `time.time()` and ended without a sample after a
  pause; it now uses a monotonic clock, keeps sampling until the expected reply
  shows (30 s cap), and always takes a final sample.

- **`dmuxai run --read-only` (and the MCP legacy `ask`/`plan` mode) now actually
  runs read-only (#212, FR-1824).** Creating the headless conversation cleared
  the pending turn policy, so the turn reached the runner as write-capable.
  Cursor now gets `--mode ask` without `--force`, custom/opencode temp configs
  get `edit`/`bash` `deny`, and grok gets `--permission-mode plan`, including
  with `--agent`/`--model` and on the triage/graph path. Runs without
  `--read-only` stay write-capable.

- **Headless sensitive-gate failures now name the headless way out and report
  `error_kind: "sensitive_gate"` (#213).** A `dmuxai run` stopped at a
  sensitive-step gate no longer suggests the TUI-only `/graph continue`; it says
  to rerun with `--allow-sensitive` (or `--agent <name> --approve-plan`), and
  automation can detect the stop from JSON. Read-only turns skip the gate
  because they cannot perform the sensitive write it protects.

- An idle-timeout (or cancel) failure no longer inherits the
  "model ... is not offered by ..." hint. That hint is now attached only when
  the launched model is genuinely absent from a successfully loaded runner model
  list, judged against the model the runner actually launched (#214).

- `[runners.cursor] model` is now used by the Cursor runner: `dmuxai run --agent cursor` launches `cursor-agent` with the configured model and reports it in `triage.model`; an explicit `--model` still wins (#211).

- `dmuxai run` / `dmuxai todo run` now start the runner with the model given by `--model`, and `--json` reports the model actually used.

- ACP runners no longer end a turn early when the agent sends a request whose id matches dmuxai's pending prompt (#242).

- **Stop now also stops an agent's shell commands (#230, FR-2270, FR-2271).**
  Stopping a running turn or graph step (dashboard **Stop**, TUI Ctrl+C) on the
  Cursor (ACP and legacy) and custom runners now also ends processes the agent
  started in their own session, such as a shell loop waiting on an external
  result. Before, only the runner's process group was signalled, so those
  commands kept running after Stop. Normal turn endings are unchanged.

- **Headless `--agent agy` plan-only replies are no longer a silent success
  (#164, FR-2121).** When AGY stops at its own plan review (it writes
  `implementation_plan.md` to its private artifact directory and asks for
  approval) without editing the workspace, a headless run now exits `7`
  (approval required) with the plan text kept in `text`. With `--approve-plan`
  the run requires file edits: AGY retries once with a stricter edit
  instruction and the run fails with exit `1` if it still only plans. A turn
  that changed workspace files (through a write tool or a shell command) is
  never treated as plan-only, and ordinary answers to questions keep exit `0`.
  Interactive TUI and dashboard runs are unchanged.

## [0.3.0] - 2026-10-27

### Changed

- **Cursor runner accepts any non-fast model (FR-2180..2182).** Lifted the built-in
  Cursor runner model block so it accepts non-fast model IDs including `grok-*`
  and `claude-sonnet-5-5-*` IDs; user configuration, custom agent `allowed_models`,
  or model catalogs dictate what is offered. Default model when unset remains `composer-2.5`,
  and models containing `fast` remain blocked across all runners.

### Added

- **Per-model system prompts and built-in prompts (SPEC-agent-system-prompt
  FR-2150..FR-2156, FR-2160..FR-2162).** Configure prompts per model with
  `[runners.<id>.models."<model>"]` (`system_prompt`, `system_prompt_file`,
  `prompt_inject`), set `builtin_prompt = false` to disable built-ins, and
  resolve prompts in order: model entry > runner table > built-in by model >
  built-in by runner > `[runners.default]` > none. Built-in prompts cover
  AGY/Gemini Flash, Cursor `composer-2.5`, runner-scoped Cursor `grok-4.7`, and
  Grok Build (`grok` runner), apply only on write turns, and
  re-inject when a model switch changes the prompt hash. The AGY efficiency
  rules now come from the built-in prompt instead of `agy_prompt`.

- Dashboard: run turns in several conversations at once (different agents; the
  same agent in a second conversation is refused with 409). Background turns keep
  running when you switch conversations, the Sessions list marks every running
  conversation, and a background turn that ends no longer wedges the follow-up
  queue (#124, FR-1620..1625).

- **Runner fallback (SPEC-runner-fallback).** A runner can hand a failed turn to
  the next runner/model in an ordered `[runners.<id>]`/`[runners.default]`
  `fallback` chain on provider errors or stalls, with `stall_timeout_secs` and
  `stall_api_retries`. Stalled runners (no model progress or repeated API retries) are stopped and handed over too.

### Fixed

- **Grok CLI v1.0.62 model listing recognized as authenticated (#207, FR-2170).**
  `grok_models_authenticated` now accepts `grok models` output from newer grok CLI
  versions that print only the model list without `"logged in"`, recognizing
  exit code 0 with at least one parseable `grok-` model id while retaining
  compatibility with the older `"logged in"` banner.
- **A reply that cannot be saved no longer disappears silently (FR-2140,
  #203).** When the workspace conversation database is locked by another
  process *during* a turn, saving the agent's reply now waits for the lock with
  the same bounded backoff the database open uses (about 20 s of waiting per
  turn; after that the database is only probed, without waiting, so a released
  lock still receives the reply). If the reply still cannot be saved, the
  headless run fails with exit code 4 (conflict; exit 6 for other database
  errors) and an error that says the reply could not be saved, instead of
  reporting `ok: true` with an empty `text`. The answer itself is never lost:
  it stays in the JSON `text`, in the transcript, and next to the "Could not
  save <agent> output" system note. The JSON now reports `error_kind`
  `conflict` for exit 4 and `internal` for exit 6 (also for the locked open of
  #196), and once the retry budget is spent the process closes its session with
  a short busy timeout, so a headless run exits without waiting the default
  connection timeout a second time.

- **A second `dmuxai run` in a busy workspace no longer fails with "database is
  locked" (FR-2120, #167).** Opening the conversation database waits for the
  other process (bounded backoff of about 20 s), takes no write transaction
  while the schema is current, and skips the WAL switch on a database that is
  already in WAL mode. A database that stays locked ends the headless run with
  exit code 4 (conflict) and a message that asks the caller to retry, instead
  of exit code 6.

### Removed

- **Removed: MCP propagation to runners (SPEC-mcp-propagation-removal).** DmuxAI
  no longer passes its `[[mcp.servers]]` list to any runner (Codex
  `-c mcp_servers.*` overrides, OpenCode `mcp` key in temp configs, custom agent
  per-run JSON and `{mcp_config}`, Cursor legacy notice and `.cursor/mcp.json`
  guard, AGY notice, and Cursor/Grok ACP `mcpServers` parameter). Every agent CLI
  configures MCP tools directly in its own native configuration. `[[mcp.servers]]`
  in `dmuxai.toml` is retained exclusively for the web dashboard (`GET /api/mcp/servers`
  and Jira/Confluence to-do import via `POST /api/import`). The automatic built-in
  `dmuxai-memory` MCP entry when `[memory]` is enabled is removed; users add
  `dmuxai mcp-memory --workspace <dir>` to their agent's own MCP config.
- **Deprecated:** `[[mcp.servers]] runners` allow-list is deprecated and ignored
  with a single warning per settings load (never a parse error). Custom agent
  `{mcp_config}` placeholder is deprecated and ignored with one warning per agent
  (the placeholder and preceding flag are dropped; `DMUXAI_MCP_CONFIG` remains
  scrubbed from child environments).
- Removed: native Windows support (packaging, launcher, docs). WSL keeps
  working. Revertable in one commit.

### Added

- **Follow-ups while a reply streams (SPEC-chat-followup-queue).** During a
  run the dashboard composer says "Send follow-up" and shows a Stop button. A
  prompt sent then is queued (marked `queued` in the chat) and starts on its
  own, in order, when the run ends. Queued follow-ups can be edited or removed
  (x) before they start, and Stop keeps the queue. View-only dashboards cannot
  queue.
- **Image attachments in the dashboard chat.** Paste an image (Ctrl/Cmd+V),
  drop it on the composer, or pick files with the paperclip button. Thumbnails
  (with a remove button) show before sending, and the sent message keeps them.
  Images are uploaded to `POST /api/attachments` (png, jpeg, gif or webp checked
  by magic bytes, up to 10 MiB each and 10 per prompt), saved under
  `.dmuxai/attachments/`, and reach the runner the same way as TUI clipboard
  images.
- **Chat button in the dashboard top bar.** One click (or Enter) goes back to
  the chat from the Diff or Graph view and closes the Memory and Skills
  popovers. The chat keeps its scroll position and the composer keeps its
  draft. Chat, Diff, Graph, Memory and Skills show which one is active
  (`aria-pressed`).

### Changed

- **ACP runners for Cursor and grok (SPEC-acp-runners).** grok defaults to
  ACP (`grok agent -m <model> stdio`); read-only internal turns start it as
  `grok --permission-mode plan agent ... stdio` and dmuxai also denies write and
  exec permissions itself. `[agents] grok_protocol = "legacy"` keeps
  streaming-json. Cursor stays on **legacy** stream-json with non-fast
  `composer-2.5` by default, because Cursor ACP only offers
  `composer-2.5[fast=true]`; with `[cursor] protocol = "acp"` a fast session is
  refused before any prompt is sent. Read-only turns use `session/set_mode` or a
  mode `configOptions` entry when the agent offers one. Configured MCP servers
  are passed as ACP `mcpServers`. A disallowed model is an error, not silently
  dropped. A denied permission or cancel ends the turn as cancelled (graph shows
  "Graph cancelled"). Execute activity rows show the command. agy and opencode
  keep their headless CLI modes (`[agents] opencode_protocol` is ignored with a
  warning).
- **CLI:** `dmuxai run` / `todo run` reject `--mode` with an error instead of
  running write-capable turns silently.
- **AGY:** normal agent turns no longer require observed file writes to succeed.
- **Single agent turn mode.** Removed `/mode`, the mode picker, and per-agent
  mode state. Every user turn runs in agent mode; judge/plan/replan use
  read-only adapter flags internally. CLI `--mode` and dashboard `set_mode` are
  ignored with a notice.

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

- **Sessions grouped by workspace folder (FR-1225..FR-1227).** The Sessions
  sidebar lists every workspace's sessions from the state dir (read-only),
  one group per folder: folder icon, bold name, muted parent path (or WSL
  distro), full path on hover. Rows are newest first with a compact time
  ("5h"); 3 per group with More/Less; the current workspace and the active
  session are highlighted; the filter spans all groups; collapsed groups are
  remembered per tab. Opening another workspace's session switches workspace
  and opens it there.

### Fixed

- **Custom agents that edit files but send no final message no longer fail
  (#146, FR-2125).** An agent turn that exits 0 with no reply, no JSONL error
  and no `failure_stderr` match used to be reported as "empty reply text". In
  an Implement (edit-allowed) turn, DmuxAI now compares `git status --porcelain`
  before and after the run: when the workspace changed, the turn completes with
  the notice "<label> finished without a final message; workspace files
  changed" instead of an error. An empty reply without a workspace change, and
  every read-only turn, still fail.

- **A model-catalog warning no longer turns unrelated failures into a runner
  handover (FR-2126).** The "model <m> is not offered by ..." hint is still
  attached to a failed turn, but the error kind becomes `model_unavailable` only
  when the error message itself reports an unknown or unavailable model. Other
  provider or process failures keep their kind and are no longer handed to the
  next runner in the fallback chain.

- **AGY keeps results after a recovered provider error (#151).** A transient
  provider error reported after AGY's final answer step no longer fails the
  turn; the result is kept with a Notice. Same behaviour for custom agents
  when the error arrives right after the final text. Mid-turn errors still fail.
- **AGY agent turns no longer pass `--sandbox`** (it made `cargo fmt`/`cargo test`
  fail with "Read-only file system"); read-only turns keep `--mode plan --sandbox`.
  AGY agent turns also get a short "Work efficiently" rule block.
- **Approving a fallback plan works again (FR-941, FR-950, TC-1011).** The
  1-node fallback revision (heuristic complex route, invalid plan twice, plan
  turn error/timeout, busy agent) stored its step without the implicit
  `task-1` reference, so `/graph approve` and the dashboard Approve were
  refused with "Approved plan steps are invalid: step must reference a task".

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

[0.3.1]: https://github.com/donggrri/dmuxai/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/donggrri/dmuxai/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/donggrri/dmuxai/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/donggrri/dmuxai/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/donggrri/dmuxai/compare/v0.1.1...v0.2.0
