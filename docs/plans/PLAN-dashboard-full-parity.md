# PLAN: Dashboard full parity and TUI retirement

Status: Draft (v0.2.4; starts after v0.2.3 is released and Dongjin approves the gap list)

Spec: [SPEC-dashboard-full-parity](../specs/SPEC-dashboard-full-parity.md).
Baseline: `origin/main` 787436b. Rebase on v0.2.3 (plan gate #67, grok runner
#66, diff review sidebar, world model) before slice 1 starts.

## Principles

- Logic in the shared Rust core, rendering in the client. A dashboard control
  calls a `SessionCommand` or `POST /api/prompt`; the App/session layer owns
  the behavior. The TUI keeps calling the same core functions until removal.
- Dashboard first for every new feature from now on (Dongjin, 2026-10-18).
- One worktree, target dir and draft PR per slice under `/workspace/dmux-wt/`.
- Grok Build (`grok -m grok-4.7`) implements; opencode free models review
  (deepseek-v4.1-flash-free, glm-5.3-flash-free second opinion).

## Module contracts

| Area | File | Contract |
| --- | --- | --- |
| Session host | `src/app.rs` → new `src/session_host.rs` | `App::step(&mut self, now) -> StepOutcome { changed, completed }` holds the body of `App::run` (`app.rs:1201-1242`, `1263-1275` minus input/draw). `App::run` (TUI) = `step` + `event::poll` + `terminal.draw`. `session_host::run(app, shutdown)` = `step` + `sleep(EVENT_LOOP_PAUSE)` + signal handling, then `App::shutdown` |
| CLI | `src/main.rs` | `run_serve` builds the same `App::new_interactive` + `attach_web_dashboard` wiring as `run_tui(Some(..))` without `Terminal`; `serve --read-only` keeps today's `run_serve` body; single-instance check through `.dmuxai/web.json` |
| Command channel | `src/main.rs:738-747` | create the `DashboardCommand` channel unconditionally; the to-do store becomes optional inside `DashboardServices` (FR-1103) |
| Images | `src/web/server.rs`, `src/attachments.rs`, `src/clipboard_image.rs` | `POST /api/attachments` (multipart or raw body + `Content-Type`), reuse `attachments_dir` and the `/image` validation; one shared validate-and-save helper also used by clipboard paste; `SessionCommand::RemoveAttachment { id }`; `SessionView.pending_images` |
| Activity | `src/activity.rs`, `src/app.rs:5699-5778` | split `render_activity_group` into a core `activity_groups(&[..], &UiSettings) -> Vec<ActivityGroupView>` used by both the TUI renderer and `session_view()`; `SessionCommand::ToggleGroup { id }`, `ToggleLastDetails` |
| Links | `src/app.rs:4487`, `src/docs/links.rs` | move the link detector into `docs::links`; snapshot messages carry `links[] {start, end, target}` confined by `docs::confine` |
| Status | `src/app.rs:5847-5998` | move the existing `StatusSegments` and `fit_status_segments` (`app.rs:5938-5951`) into the core with owned strings; `draw_status` and `SessionView.status` both use them |
| Graph gate | `src/app.rs:4105-4150`, `src/web/snapshot.rs` | reuse `GraphView.waiting_gate_node` (`snapshot.rs:206`) and add `gate_reason`; `SessionCommand::GraphContinue`, `GraphOverride { node, runner, model }` validated by `allowed_runners` and `model_policy` |
| Parity guard | `src/web/parity.rs` | `TUI_FEATURES` list (keys, mouse, panels, settings) with status and reason; `tui_features_have_dashboard_controls` |
| Page | `src/web/assets/session.js`, `session.css`, `app.js` (graph view) | attach UI (file input, drop, paste, chips), activity group expand, status header, link rendering, gate controls, shortcuts (OQ-1103) |
| Removal | `src/app.rs`, `src/chat_selection.rs`, `src/theme.rs`, `src/ui_png.rs`, `Cargo.toml` | delete TUI code paths after the deprecation release (OQ-1102) |

## Slices and order

| # | Slice | Requirements | Depends on | Notes |
| --- | --- | --- | --- | --- |
| 1 | Session host + interactive `serve` + command channel decoupling | FR-1100..1104, NFR-1100 | v0.2.3 merged | Highest risk; refactor only, behavior under `--web` unchanged. Gap list item #1 |
| 2 | Parity guard harness (`TUI_FEATURES`, source scan, TC-1120) | FR-1120, FR-1121 | 1 | Lands with today's rows; gaps are `pending`. Each later slice flips its rows to `full` in the same PR; the removal PR requires zero `pending` |
| 3 | Image attach from the browser | FR-1110 | 1 | Closes K8, M5, P5, S12 |
| 4 | Activity groups + `[ui]` settings + Ctrl+O | FR-1111 | 1 | Closes P2, C2, K5 |
| 5 | Graph gate controls | FR-1114 | 1; plan gate #67 | Shares the approval panel area from #67 |
| 6 | Status header | FR-1113 | 4 | Closes P4 |
| 7 | Transcript links | FR-1112 | 1 | Should; can slip |
| 8 | Keyboard shortcuts | UX-1100 | 3-6 | Closes K4, K6, K7 |
| 9 | Deprecation notice | FR-1130 | 2 green | Release v0.2.4 |
| 10 | TUI removal | FR-1131, FR-1132 | 9 + OQ-1102 | v0.2.4 or next release per OQ-1102 |

Slices 3, 4, 5 and 7 touch `session.js`, so they run in sequence or in
separate files (`attach.js`, `activity.js`) to avoid merge conflicts.

## Risks

- `App::run` refactor touches the hottest code path; mitigated by NFR-1100
  (existing tests untouched) and TC-1102 comparing the TUI loop and the host.
- Interactive `serve` owns child processes without a terminal: zombie/orphan
  risk on crash; covered by process-group kill and TC-1103.
- Removing the TUI removes the fallback when the browser cannot reach the
  server (SSH without port forwarding). Mitigation: document
  `ssh -L 7420:127.0.0.1:7420`; keep `dmuxai run`/`status`/`events` CLI.
- Windows: workspace switch already answers 501; interactive `serve` must
  build on the Windows target (clippy job).
