# PLAN: `--web` runs the dashboard only (no TUI)

Status: implemented
Spec: [SPEC-web-dashboard-only.md](../specs/SPEC-web-dashboard-only.md)

Make `dmuxai --web` headless: it runs the full App with the dashboard attached but
never enters a `TerminalSession` (no raw mode, no alternate screen, crossterm
event polling off), prints `dmuxai dashboard: <url>` as the first stdout line,
and ends on SIGINT/SIGTERM or the dashboard `quit` command with the normal
shutdown (`app.shutdown()`, `server.stop()`, discovery removed, exit 0).
`dmuxai` without `--web` keeps the TUI and its TTY gate; `--help` describes
`--web` as no-TUI (FR-1600..FR-1604).

Re-exec switch reuses the existing multipart path: `exec_workspace_switch`
keeps the same port/token, so a dashboard-initiated workspace switch stays
browser-only (FR-1604).

## Modules touched

- `src/main.rs`: `run_tui` headless branch (run loop over a sink-backed ratatui
  terminal, `headless_shutdown_signal`, URL print), `--help` text, the headless
  dashboard-start failure path (stderr + exit 2).
- `tests/web_dashboard_only.rs`: TC-1600..TC-1602 child-process tests.
- `README.md`: `--web` described as dashboard only (no TUI); the URL is printed.

## Risks

- FR-1601's SIGTERM half has no cross-child-process automated test (SIGINT is
  automated); verified manually — see REVIEW.
- The headless run future is dropped on signal (not awaited); shutdown runs
  after the drop, which is acceptable but unobserved by tests.
- A second Ctrl-C while shutting down is not required to work (FR-1601 edge).

## Rollback

Revert the branch; no persistence, protocol, or migration change. The library
constant `workspace::HEADLESS_DASHBOARD_ENV` is kept unchanged so
`spawn_dashboard` and its TC-1470 test keep agreeing on the reserved name for
older child binaries that still read it.
