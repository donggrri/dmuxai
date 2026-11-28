# SPEC: Board item work-session links

Status: implemented
Owner: Dongjin
Created: 2026-11-28
Baseline: `068b920` + SPEC-board-approval-hardening
Related: SPEC-decision-board FR-3100 FR-3111 FR-3113 FR-3120 FR-3123 FR-3133, SPEC-headless-runs-dashboard FR-2800 FR-2807, SPEC-task-sessions, SPEC-board-ux-042

## 1. Decision summary

A board item records only the run ids of its runs, so neither the owner nor a
manager bot can get from a card to the conversation (and the native agent
sessions) that did the work, to its graph, or to the files it changed. The
coordinator flow needs that link: the owner asks follow-up questions about the
result in the same work session (SPEC-board-ask-resume, SPEC-md-viewer-chat).
This spec makes every board run and every planner ask store a work-session link
on the item (`run_id`, `conversation_id`, `graph_id`, workspace, agent, status,
changed spec/result Markdown paths) and exposes it through CLI, MCP, HTTP and
the board modal.

## 2. Scope

### In scope

- `sessions` list on board items, written when a board run or ask finishes.
- `board session <id>` CLI, MCP `board_session`, HTTP `GET /api/board/{id}/session`.
- Board modal "Work session" section with links to the run (Runs view), the
  graph, the conversation in Chat, and result Markdown files (MD viewer).

### Out of scope / non-goals

- Continuing the session (SPEC-board-ask-resume) and chatting from the viewer (SPEC-md-viewer-chat).
- Links for runs not started by the board (they can be attached by id: FR-3826).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3820 | The "current work session" of an item is the most recent board run's conversation; planner asks are listed but are not the work session | Dongjin | Answered 2026-11-28 |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| session link | One entry in `item.sessions`: `{kind: "run"|"ask", run_id?, conversation_id?, graph_id?, workspace_path, agent?, model?, status, exit_code?, started_ms, ended_ms?, md_paths[]}` | A provider session id (never stored on the board) |
| work session | The latest `kind = "run"` link that has a `conversation_id` | The board thread |
| provider binding | Per-agent native session of a conversation, stored in the workspace conversation database (`provider_bindings`) | The board item |

Link status: `running` → `completed` \| `failed` \| `cancelled` \| `awaiting_approval`.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3820 | Must | Given `board run` starts the child run | Then a `kind = "run"` link with status `running`, `started_ms` and workspace is appended to `item.sessions` (max 50 entries, oldest dropped) and a `session` board event is logged | Old items without `sessions` load as an empty list | TC-3820 |
| FR-3821 | Must | Given the board child run returns its JSON (or exits) | Then the link gets `run_id`, `conversation_id`, `graph_id`, `agent`/`model` (resolved runner/model when known), `exit_code`, `status` and `ended_ms` from the child JSON and the run manifest, and `md_paths`: workspace-relative `.md` files changed by the run (the union of the spec gate paths recorded in the graph file and the `.md` paths that differ between a `git status --porcelain` snapshot taken by the board runner before the child starts and one taken after it ends, for git workspaces; at most 50, sorted; empty for non-git workspaces) | Missing fields stay absent; a child that crashed yields status `failed` with the exit code. No field ever contains a provider session id, a token, or a path outside the workspace | TC-3821 |
| FR-3822 | Must | Given `board ask` (with or without `--revise`) finishes | Then a `kind = "ask"` link is appended with its `run_id`, `conversation_id` and status | — | TC-3822 |
| FR-3823 | Must | Given `dmuxai board session <id> [--json]` (MCP `board_session`, HTTP `GET /api/board/{id}/session`) | Then it returns `{item_id, work_session, sessions, bindings}` where `work_session` is the latest run link with a conversation (or null) and `bindings` lists, for the work session's conversation, each agent with a native binding and its status (`active`/`stale`) read from the workspace conversation database, without the provider session id | Unknown item → exit 3 / 404; workspace or database missing → `bindings: []` and a `warning` field; read-only, works on `serve` | TC-3823 |
| FR-3824 | Must | Given `board show` / MCP `board_show` / `GET /api/board/{id}` | Then the item JSON includes `sessions` | — | TC-3824 |
| FR-3825 | Must | Given the board modal of an item with sessions | Then a "Work session" section lists each link newest first with kind, status, agent/model, age, and links: "Run" (opens the Runs view detail for `run_id`), "Graph" (opens the graph view for `graph_id`), "Open in Chat" (switches the dashboard to the workspace and conversation, `--web` only), and one link per `md_paths` entry that opens the Markdown viewer (SPEC-board-ux-042 FR-3417) | All text inserted with `textContent`; links hidden when the id is absent; the dashboard token never appears in links | TC-3825 |
| FR-3826 | Should | Given `dmuxai board link <id> --run-id R` (MCP `board_link`) | Then the run manifest R in the item's workspace is read and a `kind = "run"` link is appended from it | Unknown run → exit 3; invalid id → exit 2 | TC-3826 |
| NFR-3820 | Must | Board list/show | Listing 500 items stays within the FR-3111/NFR-3120 budgets; `board session` opens the conversation database read-only and finishes under 200 ms for a 10k-turn database | — | TC-3823 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3820 | FR-3820 | unit | Approved item, fake child runner | `board run` | Running link appended; `session` event logged; legacy item JSON without `sessions` loads | `src/board/runner.rs` `tc_3820_run_appends_running_session_link` |
| TC-3821 | FR-3821 | unit | Fake child printing JSON with run_id/conversation_id/graph_id and a temp git workspace where the fake child writes `docs/specs/SPEC-x.md` | Run to completion; crash variant | Link fields filled; md_paths workspace-relative and sorted; crash → failed | `src/board/runner.rs` `tc_3821_run_result_fills_session_link` |
| TC-3822 | FR-3822 | unit | Fake planner child | `board ask` and `--revise` | Ask links appended | `src/board/runner.rs` `tc_3822_ask_appends_session_link` |
| TC-3823 | FR-3823, NFR-3820 | unit + cli + http | Workspace conversation DB with an active cursor binding and a stale agy binding | `board session --json`, MCP, GET | work_session and bindings with status, no provider session id; missing DB → warning | `src/board/store.rs` / `src/web/board_api.rs` `tc_3823_board_session_reports_bindings` |
| TC-3824 | FR-3824 | unit | Item with sessions | show / MCP / GET | `sessions` present | `src/board/mcp.rs` `tc_3824_show_includes_sessions` |
| TC-3825 | FR-3825 | unit (asset) + e2e (browser, ignored) | Item with a completed run link with md_paths | Open modal | Work session section, Run/Graph/Open in Chat/MD links; MD link opens viewer | `src/web/assets.rs` `tc_3825_board_work_session_section`, `tests/web_board_e2e.rs` `tc_3825_*` |
| TC-3826 | FR-3826 | cli | Existing run manifest | `board link --run-id` | Link appended; unknown → exit 3 | `src/cli.rs` `tc_3826_board_link_run_id` |
