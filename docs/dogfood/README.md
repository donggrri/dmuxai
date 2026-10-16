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

## #6 Enter right after a long pasted/typed burst is swallowed — open
- Repro (tmux): `send-keys -l "<~600 chars>"` immediately followed by `send-keys Enter` → text stays in the prompt, nothing is sent; a second Enter sends it. With a 1 s pause before Enter it works.
- Likely cause: paste-burst detection treats an Enter inside the burst window as a literal newline.

## #7 AGY Implement turns silently empty — fixed (PR #8)
- Repro: AGY + Implement, ask it to create a file → turn ends after ~20 s with no text, no file, no error.
- Cause: headless AGY auto-denies `run_command` (`result.denied_actions`, `response: ""`; stderr says "no output produced ... auto-denied"). DmuxAI treated it as success.
- Fix: Implement mode passes `--dangerously-skip-permissions` (still `--sandbox`); `denied_actions` or an empty response is now an error turn.

## #8 AGY long Implement turn hangs on a background command — open
- Repro: AGY flash, Implement, "create src/agent/opencode.rs ... run cargo build" → "waiting for the background compilation" for 5+ min with no events; cancelled with Ctrl+C. No file was written.
- Workaround used: smaller prompts that say "do not run cargo"; the orchestrator compiles.

## #9 Built-in-only agent CHECK constraints in the conversation DB — fixed (PR #8)
- Repro: first opencode prompt → `CHECK constraint failed: requested_agent IN ('codex', 'cursor', 'agy')`.
- Fix: schema v2 rebuilds `turns`, `provider_bindings`, `context_checkpoints` without the built-in-only CHECK. Older binaries refuse a v2 DB (`UnsupportedSchema`).

## #10 Invalid dmuxai.toml was silently ignored — fixed (PR #8)
- Repro: any TOML syntax error → all settings silently dropped.
- Fix: startup notice `dmuxai.toml ignored: <parse error>`.

## #11 Raw-string test fixture broke on `"#` — fixed during review
- AGY flash wrote `r#"... color = "#ffa500" ..."#` in tests; `"#` ends the raw string. Changed to `r##"..."##`.
- AGY's first `substitute` re-scanned substituted values (a prompt containing `{model}` would be rewritten). Replaced by a single-pass substitution with a test.

## #12 Conversation DB is global, not per worktree — open
- Repro: run a v2-schema build (PR #8) once, then a build from an older `main` in another worktree → `Error: UnsupportedSchema(2)` and exit 1.
- Cause: the DB lives at `$XDG_STATE_HOME/dmuxai/conversations.sqlite3` (default `~/.local/state`), shared by every worktree and binary version; there is no per-workspace override.
- Workaround (parallel worktrees): `XDG_STATE_HOME=<worktree>/.dmuxai-state dmuxai`. Suggested fix: `DMUXAI_STATE_DIR` / `[state] dir`, and a clearer error naming the newer schema.

## #13 opencode (glm-5.3-flash-free) forgets fixed-size array length — fixed in review loop
- Adding `/compact` to `const SLASH_COMMANDS: [SlashCommand; 11]` without bumping the length; one follow-up prompt with the compiler error fixed it.

## Process note
#2, #3 (PR #6) and #7 (PR #8) were fixed directly (not through DmuxAI), because #3 blocked the AGY flash runner that dogfooding needs.
