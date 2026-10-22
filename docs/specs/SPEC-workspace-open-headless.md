# SPEC: "Open in new window" starts a real dashboard

Status: approved  
Owner: dmuxai  
Baseline: `main` (`b2d59fe`)

## Problem

`POST /api/workspace/open` (FR-876) spawns `dmuxai --web -C <path> --port 0`
with stdin/stdout set to null. `--web` runs the TUI, which exits at once with
"dmuxai needs an interactive terminal (TTY)". The handler never reaped the
child, so the exited process stayed a zombie, `process_alive` (`/proc/<pid>`)
kept returning true, and the request waited the full 20 s before
`504 the new dashboard did not publish a URL`. The dialog showed "Opening…"
with Switch and Open disabled for that whole time, and no window ever opened.
"Switch" (same window, re-exec) was not affected.

## 4. Requirements

| ID | Priority | Given | Then | Edge cases | Tests |
| --- | --- | --- | --- | --- | --- |
| FR-1470 | Must | A dashboard spawned by `POST /api/workspace/open` (env `DMUXAI_HEADLESS_DASHBOARD=1`, no TTY) | It runs the full App (agents, conversations, prompts) with the web dashboard and no terminal UI, writes `web.json`, and exits on the dashboard `quit` command | Without the env var, `--web` without a TTY still fails with the TTY error | TC-1470, TC-1471 |
| FR-1471 | Must | The spawned child exits before publishing `web.json` | The open request fails within about 1 s with `the new dashboard exited before publishing a URL (<status>)`; the child is reaped (no zombie) | A child that stays alive but never publishes still times out after 20 s | TC-1472 |
| FR-1472 | Must | A spawned child that published its URL | Its exit is reaped in the background so it never lingers as a zombie | — | TC-1472 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement | Command |
| --- | --- | --- |
| TC-1470 | FR-1470 | `cargo test tc_1470_spawn_dashboard_sets_headless_env` |
| TC-1471 | FR-1470 | `cargo test --test web_workspace_open tc_1471_open_in_new_window_starts_headless_dashboard` |
| TC-1472 | FR-1471, FR-1472 | `cargo test tc_1472_open_fails_fast_when_child_exits` |
