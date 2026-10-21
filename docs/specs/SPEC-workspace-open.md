# SPEC: Open another workspace dashboard

Status: approved
Owner: dmuxai
Created: 2026-10-22
Related: FR-874

## 1. Decision summary

The workspace dialog can start a second DmuxAI dashboard for another directory while this window keeps running. A turn or graph run does not block that. Switching this window still waits until the current run finishes.

## 2. Scope

### In scope

- `POST /api/workspace/open {path}` on `dmuxai --web`.
- A dialog button that opens the returned URL in a new browser window.

### Out of scope / non-goals

- One process editing two workspaces.
- Changing the 409 rule on `POST /api/workspace/switch`.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-876 | Must | Given `dmuxai --web` and a path that passes workspace validation | The server returns 200 `{ok:true, url}` for a new dashboard of that directory. An existing live `web.json` is reused. The current turn is not cancelled and switch-pending is not set | View-only `serve` returns 403. An invalid path returns 400. If the spawned process does not publish discovery in time, the response is 504 | TC-879 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-879 | FR-876 | integration | Temp workspace with a live discovery pid, then a script executable | `POST /api/workspace/open` | First response reuses the URL; second response is the URL the script published | `src/web/server.rs` `workspace_open_reuses_live_dashboard_and_spawns` |
