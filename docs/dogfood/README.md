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

## #8 AGY long Implement turn hangs on a background command — fixed (PR #21, SPEC-agy-idle-timeout)
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

## #12 Conversation DB is global, not per worktree — fixed (PR #20, SPEC-state-per-repo)
- Repro: run a v2-schema build (PR #8) once, then a build from an older `main` in another worktree → `Error: UnsupportedSchema(2)` and exit 1.
- Cause: the DB lives at `$XDG_STATE_HOME/dmuxai/conversations.sqlite3` (default `~/.local/state`), shared by every worktree and binary version; there is no per-workspace override.
- Workaround (parallel worktrees): `XDG_STATE_HOME=<worktree>/.dmuxai-state dmuxai`. Suggested fix: `DMUXAI_STATE_DIR` / `[state] dir`, and a clearer error naming the newer schema.

## #13 opencode (glm-5.3-flash-free) forgets fixed-size array length — fixed in review loop
- Adding `/compact` to `const SLASH_COMMANDS: [SlashCommand; 11]` without bumping the length; one follow-up prompt with the compiler error fixed it.

## #14 Parallel worktrees all append to the same TEST-PLAN index rows — resolved (SPEC-spec-index)
- Every spec PR adds a row at the end of the spec index table in `docs/TEST-PLAN.md`, so the second PR to merge always conflicts on rebase. Resolution is trivial (keep both rows). Suggested fix: one index file per spec or a generated index.
- Downgraded friction → resolved: the §0.1 table is gone; each SPEC carries its own `Status:` metadata plus ID ranges, `scripts/spec-index.sh` prints the index on demand, and `tests/spec_index.rs` enforces the rule, so a new SPEC touches only its own file.

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

## #22 Graph plan header shows composer-2.5 for nodes routed to AGY — fixed
- Repro: With `allowed_runners = ["agy"]` the nodes correctly run on AGY, but the plan line still printed `step-1:plan→composer-2.5`.
- Cause: `assignment_line()` printed raw `node.model_id` without runner resolution.
- Fix: Added `GraphSession::assignment_line_with(resolve)` rendering `<id>:<role>→<runner>:<model>`, and updated `App` to resolve effective models per runner (`resolved_assignment_line`), showing `provider default` for non-Cursor runners with no F2 pick while Cursor stays pinned to `composer-2.5`.

## #24 Implement mode accepted plan-only AGY replies and left the adapter busy — fixed (SPEC-implement-plan-only)
- Repro: AGY + Implement; AGY answers with a plan for review (no `write_to_file`). Turn ended early while the AGY child still ran; the next prompt showed “already running” as a Notice, cancel did nothing when the UI was idle, and write detection false-positived on the init tool list.
- Cause: Terminal events fired on the `result` NDJSON line before process exit; concurrent sends used `Notice`; cancel required `busy_agents`; Implement prompt did not require edits; write detection scanned raw lines for `write_file`/`edit_file`.
- Fix: Defer terminal events until child exit (grace kill after `result`); track writes from AGY tool `step_update` events; stricter Implement prompt with one plan-only retry then fail; concurrent sends emit `Error`; cancel clears `active_turns` and busy state.

## #23 AGY repeatedly invents `App::handle_enter` in tests — fixed
- Two separate AGY sessions wrote tests calling a nonexistent `handle_enter`; the real entry point is `submit_prompt`.
- Fix: `AGENTS.md` § Agent test helpers documents `submit_prompt`, `test_app()`, and related helpers; the `app` test module has a module doc listing them; `#[cfg(test)] App::handle_enter` forwards to `submit_prompt` so invented calls still compile.

## #21 AGY wrote PowerShell with parameters that do not exist — fixed in review
- `Join-Path -LiteralPath` / `New-Item -LiteralPath` parse fine but fail at run time. Lesson: run scripts under pwsh on the box (parse + dry run) before merging; added to the windows-pkg graph run.

## #25 tmux paste needs extra Enter presses — open
- Repro (web-dashboard run, 2026-10-17): `tmux paste-buffer -p` of a long prompt, then Enter: nothing is sent; it took two to three more Enter presses before the turn started (also no redraw after Enter during graph planning).

## #26 Ctrl+C cancels the step but the graph continues — open
- Cancelling the running node's turn leaves the graph run advancing to the next node and the status line stuck on `running`.

## #27 Killing the tmux session orphans `opencode run` children — open
- `tmux kill-session` ends dmuxai without reaping the custom-agent child; the orphan keeps editing files. Workaround: kill leftover `opencode run` processes by hand.

## #28 `routing.default_agent` ignored at startup — open
- With `default_agent = "opencode-deepseek"` the TUI still starts on Cursor (reproduced again in the web-dashboard run); `/agent` is needed every time.

## #29 Graph header shows orchestrator composer-2.5 when every node runs opencode — open
- The plan header's orchestrator label is not resolved per runner (the node labels were fixed in #22).

## #30 Pane PATH misses `~/.local/bin` / `~/.opencode/bin` — open
- dmuxai launched from a non-login shell cannot find `agy` / `opencode`; workaround: start it with `env PATH=$HOME/.local/bin:$HOME/.opencode/bin:$PATH`.

## #31 Custom agents show `Model: provider default` while running their `default_model` — open
- `opencode-glm` ran `-m opencode/glm-5.3-flash-free`, but the status bar said `Model: provider default`.

## #32 Graph routing sends every node to `routing.default_agent` — open
- `assign_runner` ignores the node role, so a per-role split (AGY for the hardest node, opencode deepseek/glm for the rest) cannot be expressed through the graph. The web-dashboard run used one dmuxai instance per runner with `[graph] enabled = false` in an untracked worktree `dmuxai.toml` instead.

## #33 opencode turns show no progress for minutes — open
- opencode `--format json` turns showed `responding · last event 4m ago` with no tool activity until the final text, so a long glm run looks hung.

## #34 An AGY network error ends a long turn with no retry — open
- After 134 tool calls, an AGY turn ended with `stream reading error ... connection timed out` (`retryable: true`). dmuxai shows the error and goes idle. Its edits stay on disk, but the turn is not retried or resumed automatically, so the user has to re-prompt with "continue". dmuxai could offer a resume for errors the provider marks retryable.

## #35 One of several dmuxai instances started together exits silently — open
- Three `dmuxai` instances were started in tmux windows within the same second in one worktree. One exited at once with no message (`EXIT=` stayed unset until the pane was restarted). Starting them a few seconds apart works. This suggests a race on the per-repo DB or state files at startup.

## #36 opencode deepseek ended a turn without writing anything — open
- The turn ended right after "Let me implement src/todo.rs", with no tool calls, no file writes and no error. dmuxai showed it as a normal completion. A "Continue" prompt in the same session resumed it. dmuxai could flag Implement turns that end without any edit or tool call.

## #37 AGY in Ask mode is denied RunCommand — open (by design?)
- A read-only Ask prompt led AGY to run `ls`, and the call was denied ("denied permission for RunCommand"), so the turn failed instead of answering. Ask mode could tell AGY up front that it has no shell, or allow read-only commands.

## #38 opencode context compaction ends the turn with a summary — open
- A long glm turn auto-compacted its context, then stopped and printed a "Next Move / Relevant Files" summary ending with "Continue if you have next steps". dmuxai showed this as a normal completion with no edits, and the work resumed only after a manual "continue". dmuxai could detect the compaction marker and continue automatically, or at least label the turn as compacted.

## #25 (recurrence) Pasted prompts still need 2-3 Enter presses
- This was seen again in every dispatch while building slices B and C. The scripts work around it by sending C-m in a loop until the status shows RUNNING.

## #39 AGY ends an Implement turn while "waiting for cargo check" — open
- AGY started `cargo check` in the background, replied "I have started `cargo check` and am waiting for it to finish." and the turn ended. DmuxAI then reported "implement mode completed without any observed file changes". A follow-up prompt was needed.

## #40 Provider network errors cut runner turns — open
- AGY turns ended with `There was a network issue connecting to the server` (retryable gRPC error) after 81 tool calls with only partial edits, and an opencode glm turn exited non-zero after repeated `AI_APICallError: Upstream ...` stream errors. DmuxAI does not retry retryable provider errors; the work was resumed or reassigned by hand.

## #41 Multi-line paste submits line by line while the runner is busy — open
- A multi-line prompt pasted with tmux bracketed paste (`paste-buffer -p`) into an opencode pane was split: the first line was submitted as a prompt, the next lines were rejected with "is busy; cancel the active turn", and the rest stayed in the input box. A `.png` path inside the text was also attached as an image. Workaround: collapse the prompt to one line.

## #42 "already has an active turn" after cancel while the status shows IDLE — open
- After Ctrl+C on an opencode turn, the status bar showed IDLE but every submit failed with "Could not start opencode-deepseek turn: opencode-deepseek already has an active turn" and no opencode process was running. Restarting DmuxAI was the only fix.

## Process note
#2, #3 (PR #6) and #7 (PR #8) were fixed directly (not through DmuxAI), because #3 blocked the AGY flash runner that dogfooding needs.
