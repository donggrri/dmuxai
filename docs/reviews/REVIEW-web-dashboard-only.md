# REVIEW: `--web` runs the dashboard only (no TUI)

Spec: [SPEC-web-dashboard-only.md](../specs/SPEC-web-dashboard-only.md)
Plan: [PLAN-web-dashboard-only.md](../plans/PLAN-web-dashboard-only.md)
Date: 2026-10-22

## Spec compliance

| Requirement | Status | Notes |
| --- | --- | --- |
| FR-1600 | Met | `--web` never enters `TerminalSession` (`headless = web.is_some()`), crossterm polling is off, stdout prints `dmuxai dashboard: <url>` first (TC-1600, TC-1603) |
| FR-1601 | Met | SIGINT exits 0, stops the server and removes `.dmuxai/web.json` (TC-1601); SIGTERM verified manually (see manual evidence below); the dashboard `quit` command also shuts down (TC-1471 path) |
| FR-1602 | Met | plain `dmuxai` without a TTY still fails with the TTY error (TC-1602) |
| FR-1603 | Met | `--help` says "Run the local dashboard with agents (no TUI)" (TC-1602) |
| FR-1604 | Met | dashboard-initiated switch re-execs with the same port/token and no TUI (TC-1604) |

## Test evidence

Automated, from the rev105 review round (recorded in `target/review/rev105.txt`):

```text
cargo test --test web_dashboard_only                          # TC-1600/1601/1602: 3 passed
cargo test --test web_workspace_open                          # TC-1471: 1 passed
cargo fmt --check                                             # clean
cargo clippy --all-targets --all-features -- -D warnings      # clean
bash scripts/check-spec.sh docs/specs/SPEC-web-dashboard-only.md   # PASS
```

Follow-up fix round (docs + minor code edits recorded here): added the missing
PLAN/TASKS/REVIEW artifacts, updated the stale README `--web` wording, deleted
the unused `HEADLESS_DASHBOARD_ENV` copy in `src/main.rs`, updated the
`workspace.rs` doc comment, set `DMUXAI_STATE_DIR` in
`tests/web_dashboard_only.rs`, and made a non-`InvalidInput` dashboard-start
failure print to stderr and exit 2 in headless `--web` instead of running with
no UI. Not re-run with cargo in this round per instruction.

Manual (real terminal, verified by the operator):

- **SIGTERM**: run `dmuxai --web`, `kill -TERM` the process — exit 0,
  `.dmuxai/web.json` removed.
- **TC-1603 PASS**: the dashboard came up live with no TUI screen in the
  terminal, a grok prompt answered and a cursor prompt answered.
- **TC-1604 PASS**: Switch to another folder and back reconnected the dashboard
  with no TUI appearing in the terminal.

## Residual risks

- The SIGTERM half of FR-1601 is manual-only (TC-1601 automates SIGINT);
  SIGTERM handling is one `tokio::select!` arm away from SIGINT
  (`headless_shutdown_signal`), and a failed SIGTERM registration cannot block
  SIGINT.
- A second Ctrl-C while shutting down is not required to work (FR-1601 edge).
- The headless run future is dropped (not awaited) when a signal wins; shutdown
  then runs from the surrounding function.
- The new headless dashboard-start failure path (stderr + exit 2) has no
  automated test: injecting a server-start failure into a child process is not
  scripted; the branch mirrors the existing `InvalidInput` handling.
- Windows is covered by compile gates only; the manual runs above were Linux.
