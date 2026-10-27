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
| OQ-2312 | Resolved: "untrusted" uses the workspace trust store from SPEC-workspace-config-trust (`trusted-workspaces.json` hash check), not the recent-folders list. | owner | merge |

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
| FR-2303 | Must | Headless `dmuxai --web` (the host registers an opener) and `POST /api/workspace/switch` to a folder that is not open | The folder opens in the same process with its own App, hub and to-do store; response `200 {"ok":true,"id","path","in_process":true,"opened":true}`; the new folder becomes focused. The request is never refused because another folder is running | No opener (e.g. `serve`, tests): the previous re-exec flow is unchanged | TC-2303 |
| FR-2304 | Must | Switch to a folder that is already open | Only focus changes; `opened:false`; no new runtime | — | TC-2304 |
| FR-2305 | Must | Opening fails | The host's status and message are returned, e.g. `409 too many open folders (max 4); close one first`, `400` for an unwritable folder | Timeout: `504 the folder did not open in time` | TC-2305 |
| FR-2306 | Must | The dashboard receives an in-process switch result | It optionally loads the requested conversation (`load_conversation`) or starts one (`new_conversation`) in the target folder, then navigates the tab to `?ws=<id>` (no `ws` for `default`) | A `202` result keeps the poll-and-reload flow | TC-2306 |
| FR-2307 | Must | The Sessions sidebar shows folder groups and `GET /api/workspaces` lists open folders (refreshed every 5 s) | Groups of open folders are marked `open`; a folder whose `running` is true shows a spinner labelled "running in this folder" | Older servers without `/api/workspaces`: groups render as before | TC-2307 |
| FR-2308 | Must | A folder group in the sidebar | A `+` button starts a new conversation in that folder through the in-process switch (`newConversation: true`) | Re-exec fallback (202) keeps the old reload flow | TC-2308 |
| FR-2309 | Must | Headless host and `POST /api/workspaces/{id}/close` | The folder's App stops, its agents are shut down and the entry is removed; `200 {"ok":true}` | Unknown id `404`; the first folder (`default`) `409 the first folder closes with Quit`; no closer registered `409 requires dmuxai --web`; timeout `504` | TC-2309 |
| FR-2310 | Must | Closing a running folder, or Quit while any folder runs | Close without `force` returns `409` with `running:true`; the dashboard asks "Quit dmuxai? Turns are still running in: …" before Quit | `force:true` closes anyway; nothing running: no prompt | TC-2310 |
| FR-2311 | Must | An open, non-first folder group in the sidebar | A `×` button closes it, confirming and retrying with `force:true` on `409` running; a tab showing that folder navigates to the first folder | — | TC-2311 |
| FR-2312 | Must | The host opens a folder whose `dmuxai.toml` has keys that need trust and has no matching entry in the workspace trust store (SPEC-workspace-config-trust, `WorkspaceTrustStatus::Untrusted`) | It opens restricted: settings load through the trust-aware loader, so keys that run commands, read secrets or set URLs are not applied, and the loader's warning explains how to trust the folder (`dmuxai trust`) | Folders without `dmuxai.toml`, with only keys that need no trust, or trusted with the current file hash open unrestricted; every opened folder is recorded in the recent list | TC-2312 |
| FR-2313 | Must | `GET /api/workspaces` and the sidebar | Each entry has `restricted`; restricted open folders show a `restricted` badge whose tooltip says the folder's `dmuxai.toml` is not trusted and to run `dmuxai trust` | — | TC-2313 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2300 | FR-2300 | unit + integration | Directory with two entries | `resolve` with and without `ws`; `GET /api/state?ws=<id>` and `?ws=missing` | Second folder's snapshot is returned; missing id gives 404 `no such open folder` | `src/web/directory.rs` `tc_2300_resolve_ws_query`; `src/web/server.rs` `tc_2300_ws_query_selects_workspace` |
| TC-2301 | FR-2301 | unit + integration | Two open folders; snapshots with busy agent, running turn, active graph | `snapshot_is_running`; `GET /api/workspaces` | Running flags match; list has ids, names, focused flag, `max` 4 | `src/web/directory.rs` `tc_2301_snapshot_is_running`; `src/web/server.rs` `tc_2301_get_workspaces_lists_open_folders` |
| TC-2302 | FR-2302 | unit (static asset) | Embedded `app.js` | Inspect `apiUrl` and the event stream URL | `apiUrl` reads `ws` from the page URL; `dmuxaiApiUrlFor` exists; no query is appended after `apiUrl(...)` | `src/web/assets.rs` `tc_2302_api_url_carries_ws` |
| TC-2303 | FR-2303 | unit + integration | Directory with an opener task that inserts `w2` | Send through the opener; POST switch to a temp dir | Request arrives; 200 with `in_process`, `opened` true, id `w2`; `/api/workspaces` shows `w2` focused | `src/web/directory.rs` `tc_2303_opener_roundtrip`; `src/web/server.rs` `tc_2303_switch_opens_folder_in_process` |
| TC-2304 | FR-2304 | integration | Folder already in the directory | POST switch to it | 200 `opened:false`; opener receives nothing | `src/web/server.rs` `tc_2304_switch_to_open_folder_only_focuses` |
| TC-2305 | FR-2305 | integration | Opener replies `409 too many open folders (max 4)` | POST switch | 409 with that text | `src/web/server.rs` `tc_2305_switch_reports_open_error` |
| TC-2306 | FR-2306 | unit (static asset) | Embedded `workspace.js` / `session.js` | Inspect the switch handler | Handles `in_process`, sets `ws` via `searchParams`, posts through `dmuxaiApiUrlFor`; sidebar passes `conversation: c.id` | `src/web/assets.rs` `tc_2306_switch_in_process_navigates` |
| TC-2307 | FR-2307 | unit (static asset) | Embedded `session.js` | Inspect the sidebar renderer | Reads `/api/workspaces`; adds `sessions-group-open` / `sessions-group-running` and the spinner label | `src/web/assets.rs` `tc_2307_sidebar_marks_open_and_running_folders` |
| TC-2308 | FR-2308 | unit (static asset) | Embedded `session.js` / `session.css` | Inspect the folder group header | `sessions-group-new` button calls the switch with `newConversation: true`; CSS rule exists | `src/web/assets.rs` `tc_2308_sidebar_new_in_folder` |
| TC-2309 | FR-2309 | unit + integration | Directory with a closer task; two entries | Send a close request; POST close for `w2` and `default` | Request arrives; `w2` removed with 200; `default` refused with 409 | `src/web/directory.rs` `tc_2309_closer_roundtrip`; `src/web/server.rs` `tc_2309_close_folder`, `tc_2309_close_default_refused` |
| TC-2310 | FR-2310 | integration + unit (static asset) | Second folder whose snapshot is running | POST close without and with `force`; inspect `session.js` | 409 `running:true`, then 200; quit text present | `src/web/server.rs` `tc_2310_close_running_folder_needs_force`; `src/web/assets.rs` `tc_2310_quit_confirms_running_folders` |
| TC-2311 | FR-2311 | unit (static asset) | Embedded `session.js` / `session.css` | Inspect the folder group header | `sessions-group-close` posts `/close` and retries with `force: true` | `src/web/assets.rs` `tc_2311_sidebar_close_folder` |
| TC-2312 | FR-2312 | unit | Temp state dir (`RuntimeEnv.dmuxai_state_dir`) and temp folders | `DmuxSettings::load_with_env` then `folder_is_restricted` for: `[notify] command` without trust, the same after `TrustStore::trust` + `save`, only `[ui] theme`, no `dmuxai.toml` | Restricted only in the first case | `src/workspace_runtime.rs` `tc_2312_unknown_folder_with_config_is_restricted` |
| TC-2313 | FR-2313 | unit (static asset) | Embedded `session.js` | Inspect the folder group header | `sessions-group-restricted` badge is rendered for restricted folders | `src/web/assets.rs` `tc_2313_sidebar_marks_restricted_folder` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| HTTP API | Optional `ws` query parameter on `/api/*`; new `GET /api/workspaces` | Requests without `ws` behave as before | TC-2300, TC-2301 |
| Dashboard JS | `apiUrl` appends `ws` only when the page has one | Single-folder dashboards send byte-identical URLs | TC-2302 |
