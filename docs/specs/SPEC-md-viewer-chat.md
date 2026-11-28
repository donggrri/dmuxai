# SPEC: Chat beside the Markdown viewer, with edits in the work session

Status: approved (Dongjin 2026-11-28)
Owner: Dongjin
Created: 2026-11-28
Baseline: `068b920` + SPEC-board-work-session + SPEC-board-ask-resume
Related: SPEC-board-ux-042 FR-3414..FR-3420, SPEC-board-ask-resume FR-3830..FR-3836, SPEC-board-work-session, SPEC-decision-board FR-3138

## 1. Decision summary

The owner reads a result Markdown file (spec, report, review) in the dashboard
viewer and wants to ask about it, or have it changed, right there, in the same
session that wrote it. Dongjin decided (2026-11-28) that this chat **may make
edits**, continuing the work session (hybrid resume, SPEC-board-ask-resume).
This amends SPEC-board-ux-042 FR-3420 (board chat was read-only): edit turns
are allowed from the viewer chat, started only by the owner, and they still go
through the normal write policy and sandbox; `--allow-sensitive` is never used.
Approval stays human-only and an edit is never a decision.

## 2. Scope

### In scope

- A chat panel docked beside the Markdown viewer when the file belongs to a board item (opened from the item modal, its thread, or its work-session links).
- Ask (read-only) and Edit (write) turns in the work session, file-scoped context, viewer refresh and changed-file list after an edit.
- `board ask --edit` (owner only) and HTTP `mode: "edit"`.

### Out of scope / non-goals

- Chat for Markdown files that do not belong to a board item (viewer stays read-only there).
- Editing the file text by hand in the viewer.
- Spec review decisions (SPEC-spec-review-gate).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3840 | Viewer chat may edit, in the same work session | Dongjin | Answered 2026-11-28 |
| OQ-3841 | Edit turns are owner-only (dashboard click with human headers, or TTY CLI without `DMUXAI_RUN_ID`); bots may ask but not edit through this path | Dongjin | Default chosen; revisit later |
| OQ-3842 | An edit turn is one agent turn in the work conversation (no planner, no spec-first graph), because it continues work whose spec already exists | Dongjin | Default chosen |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| viewer chat | The panel beside the Markdown viewer bound to `(item_id, path)` | The board modal chat (same thread, different context) |
| edit turn | A write-policy turn run with `run --conversation <work conversation> --agent A --no-spec-first --json -` (no `--read-only`) | A board run of the approved plan |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3840 | Must | Given the Markdown viewer opened from a board item context (modal summary/thread link or work-session `md_paths` link) in `--web` | Then the viewer shows a docked chat panel (right side on wide screens, below on narrow) with the item's thread messages that reference this path, a textarea, a mode toggle `Ask` / `Edit` (default `Ask`), the resume selector of FR-3836 and Send (Ctrl/Cmd+Enter) | In view-only `serve` or without an item context, no panel is shown (viewer as FR-3417) | TC-3840 |
| FR-3841 | Must | Given Send in `Ask` mode | Then it calls `POST /api/board/{id}/ask` `{text, resume, file: <path>}`; the agent prompt adds a `File in question` section naming the workspace-relative path and including its current content (≤ 64 KiB, masked); the answer appears in the panel and the item thread as a `planner` message tagged with the path | Path outside the item workspace or home rules of FR-3415 → 403; over-size file → only the path is sent | TC-3841 |
| FR-3842 | Must | Given Send in `Edit` mode | Then the request carries `mode: "edit"` and the human headers (`X-Dmuxai-Human: 1` from the click/key handler, FR-3138 browser checks); the server runs an edit turn (§3) in the work conversation with hybrid resume (FR-3830/FR-3831); the prompt states the file to change and that changes should stay focused on the request | Without human headers → 403; item without a work session → summary mode in a new conversation seeded like FR-3831; an item `in_progress` with a live run → 409 | TC-3842 |
| FR-3843 | Must | Given an edit turn | Then it uses the normal write turn policy and sandbox of the agent, never passes `--allow-sensitive`, `--trust-workspace-config` or `--approve-plan`; a sensitive gate or plan-review stop (exit 7) is reported in the panel as "needs approval — not applied" and nothing is approved | Exit codes other than 0 are shown with the masked error | TC-3843 |
| FR-3844 | Must | Given an edit turn finishes | Then the panel and thread show the changed workspace-relative paths (git status diff before/after, at most 50), the viewer reloads the file, a `kind = "ask"` session link with `mode: "edit"` is appended (SPEC-board-work-session), and the item's lane, plan, decision and checked record are unchanged | — | TC-3844 |
| FR-3845 | Must | Given `dmuxai board ask <id> --edit --text T [--file P]` | Then it is allowed only for the owner (TTY, no `DMUXAI_RUN_ID`, typed item-id confirmation as FR-3138); MCP `board_ask` has no edit mode (an `edit`/`mode` argument is rejected) | Non-owner → exit 5 | TC-3845 |
| FR-3846 | Must | Given SPEC-board-ux-042 FR-3420 | Then that row is amended: asks stay read-only, edit turns are allowed only via FR-3842/FR-3845 and never write a decision or check | — | TC-3846 |
| NFR-3840 | Must | Viewer without item context, other routes | No added requests or timers; the panel opens no stream when hidden | — | TC-3840 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3840 | FR-3840, NFR-3840 | unit (asset) + e2e (browser, ignored) | Item with a work session and md_paths | Open MD link from the modal; open a plain MD link from main chat | Panel present only for the item context; absent in serve | `src/web/assets.rs` `tc_3840_md_viewer_chat_panel`, `tests/web_board_e2e.rs` `tc_3840_*` |
| TC-3841 | FR-3841 | unit (http) | Fake runner recording the prompt | POST ask with file | Prompt has `File in question` with content; out-of-workspace path → 403 | `src/web/board_api.rs` `tc_3841_ask_with_file_context` |
| TC-3842 | FR-3842 | unit (http) | Item with work session; fake runner | POST edit with/without human headers; during live run | 202 + argv has `--conversation`, no `--read-only`, `--no-spec-first`; 403 without headers; 409 during run | `src/web/board_api.rs` `tc_3842_edit_turn_requires_human_and_resumes` |
| TC-3843 | FR-3843 | unit | Fake runner exiting 7 | Edit turn | Argv lacks forbidden flags; panel/thread "needs approval — not applied" | `src/board/runner.rs` `tc_3843_edit_turn_policy_and_exit7` |
| TC-3844 | FR-3844 | unit + e2e (browser, ignored) | Temp git workspace; fake runner edits the file | Edit turn | Changed paths listed; viewer reloads; lane/plan/decision unchanged; session link mode edit | `src/board/runner.rs` `tc_3844_edit_turn_records_changes`, `tests/web_board_e2e.rs` `tc_3844_*` |
| TC-3845 | FR-3845 | cli + mcp | Non-TTY CLI; MCP | `board ask --edit`; MCP with mode edit | exit 5; MCP rejects | `src/cli.rs` `tc_3845_cli_edit_owner_only`, `src/board/mcp.rs` `tc_3845_mcp_has_no_edit` |
| TC-3846 | FR-3846 | doc check | SPEC-board-ux-042 | Read FR-3420 row | Contains the amendment note | `tests/spec_index.rs` `tc_3846_fr3420_amended` |
