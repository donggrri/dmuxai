# DmuxAI dogfood log

Bugs and friction found while using DmuxAI to develop DmuxAI.
Format: id, status, repro, cause, fix.

## #1 No headless mode; `dmuxai --help` without a TTY fails — fixed (PR #12)
- Repro: `dmuxai --help` (or any run with stdout not a terminal) → `Error: Os { code: 6, kind: Uncategorized, message: "No such device or address" }`.
- Cause: `main` always enters raw mode / alternate screen; no argument parsing, no `--help`, no headless prompt mode.
- Impact: DmuxAI can only be driven via a real terminal (tmux `send-keys`/`capture-pane` works).

## #2 Speaker colors never shown — fixed (PR #6)
- Repro: environment has `NO_COLOR=1` (the box/desktop session exports it, plus `FORCE_COLOR=0`). Start DmuxAI: every chat line (You, Cursor, AGY, DmuxAI) is plain white even though PR #5 added `[theme]`.
- Cause: not the theme code. The ratatui buffer holds the right colors (new TestBackend test proves it), but crossterm honors `NO_COLOR` and strips every color escape on output.
- Fix: `DMUXAI_FORCE_COLOR=1` or `[theme] force_color = true` in `dmuxai.toml` force colors on (`DMUXAI_FORCE_COLOR=0` forces off). When `NO_COLOR` turns colors off, a startup notice explains how to enable them.

## #3 F2 model on AGY overridden with composer-2.5 — fixed (PR #6)
- Repro: `/agent` → AGY, F2 → "Gemini 3.8 Flash (Low)", send any prompt → `AGY model selected: composer-2.5`, agy fails `invalid model selection (--model "composer-2.5")`.
- Cause: the auto planner's single/graph plan always carries composer-2.5, and `apply_runner_and_model` pushed it onto whatever runner was selected.
- Fix: a model picked with F2 for that agent always wins; the composer-2.5 graph default only applies to the Cursor runner; other runners use their provider default. `/graph override` uses the runner-aware policy (AGY: `gemini-*-flash-*`; opencode: `opencode/*-free`; Cursor: composer-2.5 only).
- Verified live in tmux: AGY + Gemini 3.8 Flash (Medium) answered `PONG`.

## #4 F2 model list for AGY is slow — fixed (PR #12: model cache)
- Repro: F2 on AGY shows "Loading models from the provider CLI..." for ~20 s.

## #5 Status bar truncated on narrow terminals — fixed (PR #12)
- Repro: 200-column terminal; status line's right side (`Esc close`) is cut.

## #6 Enter right after a long pasted/typed burst is swallowed — fixed (PR #12)
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

## #14 Parallel worktrees all append to the same TEST-PLAN index rows — friction
- Every spec PR adds a row at the end of the spec index table in `docs/TEST-PLAN.md`, so the second PR to merge always conflicts on rebase. Resolution is trivial (keep both rows). Suggested fix: one index file per spec or a generated index.

## #15 "last event Ns ago" keeps growing while AGY runs tools — fixed (PR #18)
- Repro: AGY Implement turn that reads/edits files: status shows `working · 9m · last event 9m ago` although AGY is active (tool steps are not counted as events). Looks hung; it is not.
- Cause: `apply_agent_output` returned early on `stored_event` returning `None` for ephemeral `AgentEvent::Activity` items before reaching `observe_activity`.
- Fix: `observe_activity` is called before `stored_event`, and `AgentActivity::observe` resets `last_event_at` on every agent event including `Tool` and `Thinking` items.

## #16 Interrupted worker left activity WIP uncommitted; rebase onto #8–#11 conflicted — fixed during resume
- Stash/rebase hit keep-both conflicts in app.rs/settings.rs/TEST-PLAN; the expand test also outgrew the 40-row test viewport after the graph status line landed (raised to 60). Lesson: commit WIP per node so a resume can rebase cleanly.

## #17 Fast-typed `/agent agy` + Enter is sent to the selected agent as a prompt — fixed (PR #18)
- Repro (tmux send-keys, no delay): the slash popup had not opened yet, so Enter submitted `/agent agy` to Cursor as a normal turn (cancelled at once).
- Cause: `submit_prompt` only matched exact strings (`"/agent"`) and fell through any command with arguments or unknown `/...` to `submit_to_agent`.
- Fix: any prompt starting with `/` that is not a known slash command is rejected with `Unknown command: /x (type / for the list)` and kept in input; `/agent <name>` and `/mode <ask|plan|implement>` execute directly upon submission; `//` escapes literal `/` prompts.

## #18 Cursor turn shows "Model: provider default" — fixed (PR #16)
- Repro: A Cursor turn started outside the graph path (prompt began with `/`) used the provider default model instead of composer-2.5.
- Cause: `submit_to_agent` submitted with whatever model was in `catalog.selected` (None outside graph), and `cursor.rs` omitted `--model` when unset.
- Fix: Cursor adapter argv now always pins `--model composer-2.5` by default; any disallowed or fast model is replaced with `composer-2.5`; `submit_to_agent`, `effective_model_for`, `apply_runner_and_model`, and F2 selection enforce `composer-2.5` and emit a system notice on replacement; the status bar shows `composer-2.5` instead of "provider default".

## #19 Screen not repainted after a turn completes — fixed (PR #18)
- Under tmux the status bar kept showing `AGY RUNNING · 4m 09s` long after AGY exited; the next key press repainted it as IDLE with the full answer.
- Cause: event draining completion was throttled by `RENDER_INTERVAL` without an immediate draw trigger, and the event loop lacked a timer tick while agents were busy without keyboard events.
- Fix: `drain_agent_events` returns completion to force an immediate terminal redraw; `dirty` persists across throttled iterations; 1s busy tick (`app.tick()`) marks dirty while any agent is busy so `working · Ns · last event Ns ago` advances.

## #20 Graph-by-default can route repo work to the box cursor-agent — fixed (PR #19)
- Repro: Because the box cursor-agent is a company account, graph-by-default routed work to `cursor-agent`.
- Cause: Multi-step prompt planning unconditionally assigned nodes to `cursor` (or `routing_default_agent`) without a mechanism to restrict graph execution to specific runners, and configuration was only workspace-local.
- Fix: Added `[graph] allowed_runners = ["agy", "opencode", ...]` in `src/settings.rs` supporting built-ins (`agy`, `codex`, `cursor`) and custom agent names. Unknown runners emit a config warning and are ignored. `assign_runner` selects `routing.default_agent` if allowed, otherwise `cursor` if allowed, otherwise the first allowed runner; if empty, it falls back to single-agent mode with the selected agent and a system notice. Graph nodes (including orchestrate) and `/graph override` strictly enforce the allowed list. Supported machine-global user configuration loading from `$XDG_CONFIG_HOME/dmuxai/dmuxai.toml` (fallback `~/.config/dmuxai/dmuxai.toml`) with per-key workspace overrides. Displayed allowed runners in `/graph` and `/graph status`.

## #22 Graph plan header shows composer-2.5 for nodes routed to AGY — open (cosmetic)
- With `allowed_runners = ["agy"]` the nodes correctly run on AGY, but the plan line still prints `step-1:plan→composer-2.5`. It should print the effective per-runner model.

## #23 AGY repeatedly invents `App::handle_enter` in tests — friction
- Two separate AGY sessions wrote tests calling a nonexistent `handle_enter`; the real entry point is `submit_prompt`. A test-helper note in AGENTS/README would avoid the loop.

## #21 AGY wrote PowerShell with parameters that do not exist — fixed in review
- `Join-Path -LiteralPath` / `New-Item -LiteralPath` parse fine but fail at run time. Lesson: run scripts under pwsh on the box (parse + dry run) before merging; added to the windows-pkg graph run.

## Process note
#2, #3 (PR #6) and #7 (PR #8) were fixed directly (not through DmuxAI), because #3 blocked the AGY flash runner that dogfooding needs.
