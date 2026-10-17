# Changelog

All notable changes to dmuxai are listed here. Versions follow
[Semantic Versioning](https://semver.org/); dates are Asia/Seoul.

## [0.2.0] - unreleased

Changes since v0.1.1 (2026-08-24), PRs #1-#27.

### Added

- **Web dashboard (slice A, #27):** `dmuxai serve` (read-only, follows
  `.dmuxai/graphs/*.json`) and `dmuxai --web` (TUI plus dashboard). The page
  shows the live graph run: running/done/pending nodes, runner and model, live
  elapsed time, and agent busy chips over SSE. It binds to 127.0.0.1 by
  default, prints a per-start access token URL, rejects foreign
  Origin/Host, and needs `--allow-remote` (with a warning) to bind elsewhere.
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
- **Release workflow:** now also builds `dmuxai-linux-x86_64.tar.gz` with a
  SHA-256 checksum for WSL and Linux users. It is published to the same
  GitHub release as the Windows zip.

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
- **Builds:**
  - the non-Windows `dmuxai-launcher` stub (#17)
  - bash-only spec-index tests are gated on Windows (#24)

[0.2.0]: https://github.com/donggrri/dmuxai/compare/v0.1.1...v0.2.0
