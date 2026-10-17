# Changelog

All notable changes to dmuxai are listed here. Versions follow
[Semantic Versioning](https://semver.org/); dates are Asia/Seoul.

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

[0.2.0]: https://github.com/donggrri/dmuxai/compare/v0.1.1...v0.2.0
