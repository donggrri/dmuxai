# SPEC-multi-workspace-host

Status: draft

Owner: dmuxai
Created: 2026-10-27
Baseline: main
Related: SPEC-workspace-open-headless, SPEC-session-sidebar, SPEC-dashboard-fullwidth FR-1227, SPEC-process-supervisor

## 1. Decision summary

One `dmuxai --web` process can host several workspace folders at once. Each
open folder has its own runtime (conversation engine, App loop, dashboard hub,
to-do store and agent processes). The dashboard can open a conversation in
another folder, or start a new one there, while turns in other folders keep
running, and it can switch back and forth between open folders.

Defaults chosen with the owner:

- At most 4 folders are open in one process.
- The terminal UI (plain `dmuxai`) is unchanged and shows only the first folder.
- Quitting while turns are running in any folder asks for confirmation and then
  stops them.
- A folder that was never opened with dmuxai before opens in a restricted way,
  with a warning.

This spec is delivered in stacked pull requests; each one adds its rows here.

## 2. Scope

### In scope

- Folder selection for every dashboard API request (`ws` query parameter).
- Listing the open folders and whether each one is running.
- Opening and focusing folders inside the running process instead of
  re-executing it.
- Dashboard sidebar state per folder, closing folders, quit confirmation, and
  restricted opening of unknown folders.

### Out of scope / non-goals

- Several folders in the terminal UI.
- Sharing one agent process between folders.
- Changing how a single-folder dashboard behaves when no `ws` parameter is sent.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2300 | Folder ids are opaque, process-local strings (`default` for the first folder). A reloaded page whose id no longer exists falls back to the first folder. | dmuxai | B2 |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Open folder | A workspace folder with a live runtime in this process, listed by `GET /api/workspaces`. | A folder that only appears in the recent-workspaces list. |
| Focused folder | The folder used when a request has no `ws` parameter. | The folder a given browser tab shows (its `ws` parameter). |
| Running | The folder's latest snapshot has a busy agent, an unfinished turn, or an unfinished graph run. | A folder that is merely open. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2300 | Must | Any `/api/*` request, including docs routes | A `ws=<id>` query parameter selects that open folder; without it the focused folder is used | Unknown id: `404` with `{"error":"no such open folder"}`; docs ticket URLs are found in any open folder | TC-2300 |
| FR-2301 | Must | `GET /api/workspaces` with a valid token | `{"max":4,"workspaces":[{"id","path","name","focused","running"}]}` in open order; `running` follows the folder's snapshot | Same auth and view-only rules as other `/api` routes | TC-2301 |
| FR-2302 | Must | The dashboard page URL has `ws=<id>` | Every dashboard API request and the event stream carry the same `ws`; `window.dmuxaiApiUrlFor(id, path)` targets another folder | Without `ws` (or with an invalid id) request URLs are unchanged | TC-2302 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2300 | FR-2300 | unit + integration | Directory with two entries | `resolve` with and without `ws`; `GET /api/state?ws=<id>` and `?ws=missing` | Second folder's snapshot is returned; missing id gives 404 `no such open folder` | `src/web/directory.rs` `tc_2300_resolve_ws_query`; `src/web/server.rs` `tc_2300_ws_query_selects_workspace` |
| TC-2301 | FR-2301 | unit + integration | Two open folders; snapshots with busy agent, running turn, active graph | `snapshot_is_running`; `GET /api/workspaces` | Running flags match; list has ids, names, focused flag, `max` 4 | `src/web/directory.rs` `tc_2301_snapshot_is_running`; `src/web/server.rs` `tc_2301_get_workspaces_lists_open_folders` |
| TC-2302 | FR-2302 | unit (static asset) | Embedded `app.js` | Inspect `apiUrl` and the event stream URL | `apiUrl` reads `ws` from the page URL; `dmuxaiApiUrlFor` exists; no query is appended after `apiUrl(...)` | `src/web/assets.rs` `tc_2302_api_url_carries_ws` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| HTTP API | Optional `ws` query parameter on `/api/*`; new `GET /api/workspaces` | Requests without `ws` behave as before | TC-2300, TC-2301 |
| Dashboard JS | `apiUrl` appends `ws` only when the page has one | Single-folder dashboards send byte-identical URLs | TC-2302 |
