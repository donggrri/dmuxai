# DmuxAI dogfood log

Bugs and friction found while using DmuxAI to develop DmuxAI.
Format: id, status, repro, cause, fix.

## #1 No headless mode; `dmuxai --help` without a TTY fails — open
- Repro: `dmuxai --help` (or any run with stdout not a terminal) → `Error: Os { code: 6, kind: Uncategorized, message: "No such device or address" }`.
- Cause: `main` always enters raw mode / alternate screen; no argument parsing, no `--help`, no headless prompt mode.
- Impact: DmuxAI can only be driven via a real terminal (tmux `send-keys`/`capture-pane` works).

## #2 Speaker colors never shown — fixed (this PR)
- Repro: environment has `NO_COLOR=1` (the box/desktop session exports it, plus `FORCE_COLOR=0`). Start DmuxAI: every chat line (You, Cursor, AGY, DmuxAI) is plain white even though PR #5 added `[theme]`.
- Cause: not the theme code. The ratatui buffer holds the right colors (new TestBackend test proves it), but crossterm honors `NO_COLOR` and strips every color escape on output.
- Fix: `DMUXAI_FORCE_COLOR=1` or `[theme] force_color = true` in `dmuxai.toml` force colors on (`DMUXAI_FORCE_COLOR=0` forces off). When `NO_COLOR` turns colors off, a startup notice explains how to enable them.

## #3 F2 model on AGY overridden with composer-2.5 — fixed (this PR)
- Repro: `/agent` → AGY, F2 → "Gemini 3.8 Flash (Low)", send any prompt → `AGY model selected: composer-2.5`, agy fails `invalid model selection (--model "composer-2.5")`.
- Cause: the auto planner's single/graph plan always carries composer-2.5, and `apply_runner_and_model` pushed it onto whatever runner was selected.
- Fix: a model picked with F2 for that agent always wins; the composer-2.5 graph default only applies to the Cursor runner; other runners use their provider default. `/graph override` uses the runner-aware policy (AGY: `gemini-*-flash-*`; opencode: `opencode/*-free`; Cursor: composer-2.5 only).
- Verified live in tmux: AGY + Gemini 3.8 Flash (Medium) answered `PONG`.

## #4 F2 model list for AGY is slow — open (friction)
- Repro: F2 on AGY shows "Loading models from the provider CLI..." for ~20 s.

## #5 Status bar truncated on narrow terminals — open (friction)
- Repro: 200-column terminal; status line's right side (`Esc close`) is cut.

## Process note
#2 and #3 were fixed directly (not through DmuxAI), because #3 blocked the AGY flash runner that dogfooding needs.
