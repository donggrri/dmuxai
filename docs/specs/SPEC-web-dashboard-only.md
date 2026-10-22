# SPEC: `dmuxai --web` runs the dashboard only (no TUI)

Status: approved  
Owner: dmuxai  
Baseline: `main` (`a89eddd`)

## Problem

`dmuxai --web` started the terminal UI and the web dashboard together. The
TUI in the terminal (often WSL) is no longer used; the dashboard is the main
surface. Running `--web` without a TTY failed unless the internal
`DMUXAI_HEADLESS_DASHBOARD` variable was set (FR-1470).

## 4. Requirements

| ID | Priority | Given | Then | Edge cases | Tests |
| --- | --- | --- | --- | --- | --- |
| FR-1600 | Must | `dmuxai --web [--port N] [-C DIR]` from a terminal | The full App (agents, conversations, prompts, graphs) runs with the web dashboard and **no terminal UI**: no alternate screen, no raw mode. stdout prints `dmuxai dashboard: <url>` | Supersedes the TTY-only part of FR-1470: `--web` no longer needs a TTY, with or without `DMUXAI_HEADLESS_DASHBOARD` | TC-1600, TC-1603 |
| FR-1601 | Must | A running `--web` process | It stays alive until Ctrl-C (SIGINT) or SIGTERM, or the dashboard `quit` command; then it shuts runners down, stops the server, removes `.dmuxai/web.json` and exits 0 | A second Ctrl-C while shutting down is not required to work | TC-1601, TC-1603 |
| FR-1602 | Must | `dmuxai` with no `--web` | Still starts the TUI (needs a TTY) as before | — | TC-1602 |
| FR-1603 | Should | `dmuxai --help` | Describes `--web` as "Run the local dashboard with agents (no TUI)" | — | TC-1602 |
| FR-1604 | Must | Workspace Switch from the dashboard of a `--web` process | Re-exec keeps working with no TUI and the same port/token (FR-1227, FR-1474) | — | TC-1604 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement | Command |
| --- | --- | --- |
| TC-1600 | FR-1600 | `cargo test tc_1600_web_runs_without_tty`: start `dmuxai --web --port 0` with stdin/stdout piped (no TTY); the first stdout line is `dmuxai dashboard: http://…`, `web.json` appears |
| TC-1601 | FR-1601 | `cargo test tc_1601_web_exits_on_sigint` (Unix): send SIGINT; the process exits 0 within 10 s and `web.json` is removed |
| TC-1602 | FR-1602, FR-1603 | `cargo test tc_1602` : plain `dmuxai` without a TTY still prints the TTY error; help text names `--web` as no-TUI |
| TC-1603 | FR-1600, FR-1601 | Manual: in a real terminal run `dmuxai --web`; Pass: only the URL line (no TUI screen), the dashboard (light theme) loads, a grok prompt answers, Ctrl-C returns to the shell |
| TC-1604 | FR-1604 | Manual: in that dashboard Switch to another folder and back. Pass: the dashboard reconnects, no TUI appears in the terminal |
