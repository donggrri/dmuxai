# REVIEW: dashboard full parity spec (v0.2.4)

Date: 2026-10-18/19 (KST). Scope: SPEC, PLAN and TASKS for dashboard full parity and TUI retirement. Docs only; no code changed.

## Audit

- Grok Build (grok-4.7, read-only) produced an independent 87-row inventory: [research/dashboard-parity-audit](../research/dashboard-parity-audit.md). It changed four §3 rows:
  - S13: `/details` does not expand anything in the browser.
  - S16: the gate hint tells the user to type `/graph continue` in the TUI.
  - K2: Shift+Enter is not a TUI key.
  - P4: the background-conversation notice slot is missing.

## Free-model reviews (read-only, opencode)

- **deepseek-v4.1-flash-free** (primary): REQUEST_CHANGES with 23 items (2 blockers, 7 major, 14 minor). All were applied:
  - `serve` is not view-only (to-dos and memory are writable there).
  - `/api/prompt` returns 202, not 200.
  - The upload contract now validates like `/image` and replaces the unvalidated PNG save.
  - The parity guard is now a harness with `pending` rows plus a source scan of `handle_key`, `handle_mouse` and `handle_paste`.
  - TC oracles that compared functions to themselves now use concrete values: clock-driven cadence, pinned literals.
  - Wrong citations fixed: `server.rs:2607`, `main.rs:99` and `:365`, `session.js` K9.
  - Added the workspace-switch row (R5).
  - Added the new snapshot fields.
  - Defined the activity state model, attachment ids and stale `web.json` handling.
  - Moved the manual browser checks into the TC oracles.
  - Added the `check-spec.ps1` gate to TASKS.
- **glm-5.3-flash-free** (second opinion): failed 3 times with `Upstream request failed: Endpoint is unavailable`. One run hung for over 20 minutes and was killed. A fourth run is noted below.
- **nemotron-3-ultra-free** (allowed fallback second opinion): REQUEST_CHANGES with 30 items. Applied:
  - OQ-1000 wording ("view-only").
  - Clearer S12 text.
  - Workspace-switch re-exec added to slice 1.
  - Separate JS modules per slice.

  Rejected, with reasons:
  - Clicks on status-bar segments: `handle_mouse` only handles the input border and chat, `app.rs:2143-2207`.
  - Requirements "assume code that doesn't exist": they are requirements for new code, and the plan names the refactor.
  - `[routing] cursor_models`: already marked as a side finding, not a gap.
  - Several duplicates of the deepseek items.

## Checks

- `cargo test --locked --test spec_index` (`every_spec_has_status_line`, `spec_ids_are_unique_across_specs`, `spec_index_script_lists_every_spec`) passed locally with `CARGO_TARGET_DIR=/workspace/dmux-wt/dashboard-parity-target`.
- CI on PR #68: Spec traceability passes. Rust quality gates are skipped because the PR is a docs-only draft (#63).

## Residual risk

- `App::run` refactor (slice 1) is the riskiest step; see PLAN risks.
- Gap counts are a judgment call at row granularity (52 rows here vs 87 in the Grok inventory); Dongjin's review of §3 decides the final list.

## Round 2 (2026-10-19 00:15-00:27 KST): stopped

- Rebased on origin/main a4c50ae (#65, #66). The script `remap.py` (difflib) remapped the `app.rs`, `app.js`, `snapshot.rs`, `settings.rs` and `cli.rs` line refs. Spot-checks match.
- Added §3.0, which records what each mode does without the TUI.
- Added rows P10 (grok runner, full under `--web`) and P11 (#70, n/a), and recorded #71 as a dashboard-only feature.
- The new counts were checked with a script: 54 rows, 27 full, 13 partial, 6 missing, 8 n/a.
- The delta review on deepseek-v4.1-flash-free and glm-5.3-flash-free gave no output in about 12 minutes. Both processes were killed when work stopped for the day. **This review is still to do.**

## Round 3 (grok via dmuxai)

Checked §3 against the workspace source, which is `origin/main` f1b36ae (#70 is in that tree). Docs only.

Changed:

- Baseline is `origin/main` f1b36ae. #70 is merged (f1b36ae) in the baseline, P11, §3.8, and `docs/ROADMAP.md`. #71 stays open. `draw_conversation_picker` is still `app.rs:6278`.
- §3.0: the no-args hub is `main.rs:655-661` (654 is the `--web` call). `SnapshotSource::Files` is `:515`. `attach_web_dashboard` is `:730-829` (also R2). Browse/switch 403 is `server.rs:1228-1229` and `:1341-1342` (also C4).
- R5: `server.rs:1199` is `can_switch` on a 200, not a 403. `GET /api/workspace` stays 200 under `serve`.
- #70 added lines after the conversation picker, so three `app.rs` cites still had a4c50ae numbers: `summary_prompt` 7228→7252 (S9), mode picker 6366→6390 (P7), slash-preview draw 6439→6463 (K9).
- P8: `push_startup_notice` is `app.rs:4430`; the welcome text is `:1000-1002`. Palette map is `session.js:647-697`. Filter and arrows are `:788-823`. Heartbeat `touch_session` is `:1203`.

Checked and left:

- §3.0 TTY exit `main.rs:607-612` (message and exit 1; stdin or stdout). `TerminalSession::start` and `App::run` `:663-664`. `run_serve` `:437-566` (URL at 543, `web.json` via `publish_discovery` at 545, read-only Docs `:467`, 500 ms poll `:552-559`, no App). Prompt 409 `:929-931`, command 409 `:970-972`, to-do Run 409 `:875-876`. `App::run_headless` `app.rs:4749` (called from `cli.rs` for `run` and `todo run`). Serve snapshot agents stay empty, so grok is absent there. Memory and MCP import do not need the command channel.
- P10: TUI confirm probes at `app.rs:2345` through `request_builtin_grok_models` `:2265`. Dashboard `SessionCommand::SelectAgent` `:1682-1690` calls the same probe. Chip color is `AgentView.color` `#6366f1` (SPEC-grok-runner FR-896, FR-898; tests `grok_chat_and_dashboard_use_preset_color`, `selecting_grok_requests_a_model_probe`). Still full under `--web` only.
- Fifteen other §3 cites that still match: `handle_key` 1996-2058, Enter 2043, Ctrl+C 1997 and `cancel_selected` 3168, Ctrl+O 2001 and `toggle_last_activity_details` 3746, F2 2041 and `open_model_picker` 2455, F3 2042, clipboard image 2030-2036 and 2704, paste 1977-1994, mouse 2144-2208, `open_file_link_at` 4487, M2 `:6607` (`file_link_click_target`), `render_activity_group` 5699-5778, `draw_status` 5847-5998, `App::run` 1187-1279 with command drain 1224, gate string 3989 and 4136, `app.js:1690-1700` (the TUI gate hint), chips `app.js:1739-1788`, session Esc 358/396/760/815, memory approve/reject `server.rs:1495-1502`, Shift+Enter row `parity.rs:26-30`, activity collapsed line `app.rs:1590-1592`.
- §3.9 recounted from the row statuses in §3.1-3.6: 54 rows, 27 full, 13 partial, 6 missing, 8 n/a. The table already matched. No status changed (P10 stays full, P11 stays n/a).

## Round 4 (free-model review of round 3)

Reviewers of the round-3 doc diff. Docs only.

- opencode nemotron-3-ultra-free: APPROVE, no items. Noted a ±1 drift on the §3.0 no-args hub cite.
- opencode beam-free: APPROVE, one minor on the `cli.rs` range in R4.
- deepseek-v4.1-flash-free and glm-5.3-flash-free: no answer (timeouts).

Changed in `docs/specs/SPEC-dashboard-full-parity.md`:

- §3.0 no-args hub: `:655-661` included the `} else {` line. The hub body (no server, event-log sink, `None`) is `main.rs:656-661`.
- R4: `src/cli.rs:524-612` covered `run_and_report` (524-550) and stopped one line short of `run_headless`. The row means the headless runner, now `src/cli.rs:552-613` `run_headless`. That was the only `cli.rs` cite in the spec.
