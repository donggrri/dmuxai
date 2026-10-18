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
