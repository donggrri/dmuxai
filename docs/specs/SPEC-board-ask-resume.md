# SPEC: Ask in the work session (hybrid resume)

Status: approved (Dongjin 2026-11-28)
Owner: Dongjin
Created: 2026-11-28
Baseline: `068b920` + SPEC-board-work-session
Related: SPEC-decision-board FR-3132, SPEC-board-ux-042 FR-3418 FR-3420, SPEC-task-sessions (per-agent native resume, unseen delta, fresh fallback), SPEC-board-work-session, docs/conversation-continuity-research.md

## 1. Decision summary

Today `board ask` starts a brand-new read-only turn that only sees the item
title, summary, plan and thread, so the answer has none of the context of the
session that did the work. Dongjin decided (2026-11-28) that a follow-up should
continue the work session with a **hybrid** model: if the work conversation has
a live native session (provider binding) for the agent, resume it; otherwise
start a new session of that agent seeded with a bounded summary of the work
session's history. This spec adds headless continuation of an existing
conversation (`run --conversation`) and makes `board ask` use it by default
when the item has a work session.

## 2. Scope

### In scope

- `dmuxai run --conversation <id>` (continue a stored conversation headlessly), MCP `run` `conversation_id`.
- `board ask --resume auto|native|fresh|off` and the HTTP/MCP equivalents; default `auto` when a work session exists.
- Reporting which mode was used (`native`, `summary`, `off`) in the answer, thread and JSON.

### Out of scope / non-goals

- Edit turns from the chat (SPEC-md-viewer-chat). In this spec asks stay read-only.
- Moving a session to a different agent family's native format.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3830 | Hybrid: native resume when the binding is active, else a fresh session seeded with a summary | Dongjin | Answered 2026-11-28 |
| OQ-3831 | Default agent for the follow-up is the agent of the work session's last write step (implement), else the run's resolved runner; `--agent` overrides | Dongjin | Answered 2026-11-28 |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| resume mode | `native` (provider session resumed with the unseen delta), `summary` (fresh provider session whose first prompt carries the work-session summary), `off` (old behavior: item context only) | Board lane |
| work-session summary | Bounded text (≤ 12 000 chars, secrets masked) built from the conversation's latest stored summary document if any, plus the plan steps with their status, the last assistant text of each step (each ≤ 1 500 chars), the changed Markdown paths and the item thread's last 10 messages | The board item summary field |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3830 | Must | Given `dmuxai -C <ws> run --conversation <id> --agent A [--read-only] [--json] -` | Then the prompt is sent as a new turn of conversation `<id>` in that workspace's conversation store (no judge, no planner, no spec-first graph: one agent turn), using the store's resume decision: active binding for A → native resume with the bounded unseen delta; otherwise a fresh session with the projection; the JSON output gains `conversation_id` (same id) and `resume: {mode: "native"|"summary", agent}` | Unknown conversation → exit 3 (`not_found`); invalid id → exit 2; `--conversation` with `--graph` or `--agents` → exit 2. A provider that rejects the resume retires the binding and retries once in `summary` mode (existing fallback), reported as `summary` | TC-3830 |
| FR-3831 | Must | Given FR-3830 with no active binding for A | Then the fresh session's prompt starts with a `Work session summary` section (the work-session summary, §3) before the user text | The summary never contains a token, a provider session id or text over the cap | TC-3831 |
| FR-3832 | Must | Given `board ask <id> --text T [--resume auto|native|fresh|off]` (MCP `board_ask` `resume`, HTTP `POST /api/board/{id}/ask` `{text, resume?}`) on an item with a work session (SPEC-board-work-session) | Then `auto` (default) runs `run --conversation <work conversation> --agent <OQ-3831 agent> --read-only --json -` in the item workspace; `native` requires an active binding (else conflict exit 4 / 409 with reason `no_active_binding`); `fresh` forces `summary` mode; `off` keeps the FR-3132 behavior | An item without a work session behaves as `off` for `auto` and is a usage error for `native`/`fresh`; `--revise` ignores `--resume` (plan revision keeps FR-3132) | TC-3832 |
| FR-3833 | Must | Given an ask answered via FR-3832 | Then the `planner` thread message is prefixed with `[native resume · <agent>]` or `[summary resume · <agent>]`, the JSON/HTTP result has `resume.mode`, and the ask session link (SPEC-board-work-session FR-3822) records the same conversation id | — | TC-3833 |
| FR-3834 | Must | Given an ask while the item has a live board run (lane `in_progress` with a run lock) | Then `auto` runs in a new conversation seeded with the work-session summary (mode `summary`; it never appends to the running conversation or resumes a provider session a running step may be using) and the answer says so; `native` is a conflict (exit 4 / 409) | — | TC-3834 |
| FR-3835 | Must | Given any ask in this spec | Then it runs with the read-only turn policy, never writes a decision or check (FR-3138, FR-3420), and never passes `--allow-sensitive` or `--trust-workspace-config` | — | TC-3835 |
| FR-3836 | Must | Given the board modal chat in `--web` | Then a small selector next to Send offers `Work session (auto)`, `Fresh with summary`, `Item only` (maps to `auto`, `fresh`, `off`); the mode used is shown on each answer; the selector is hidden when the item has no work session | View-only `serve` unchanged (store-only Send) | TC-3836 |
| NFR-3830 | Must | `run` without `--conversation`, board list/show | No added work; a native resume adds no extra provider turn | — | TC-3830 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3830 | FR-3830, NFR-3830 | integration | Fake cursor-agent binary recording argv; a conversation with an active cursor binding `S` | `run --conversation <id> --agent cursor --read-only --json -` | argv has `--resume S`; JSON `resume.mode = native`; unknown id → exit 3; with `--graph` → exit 2 | `tests/run_conversation.rs` `tc_3830_run_conversation_native_resume` |
| TC-3831 | FR-3831 | integration | Conversation with turns but no active binding for the agent | Same command | No `--resume`; prompt starts with `Work session summary`; mode `summary`; summary ≤ cap and masked | `tests/run_conversation.rs` `tc_3831_run_conversation_summary_seed` |
| TC-3832 | FR-3832 | unit | Item with work session; fake runner | ask auto/native/fresh/off; native without binding | Child argv uses `--conversation` per mode; native without binding → 4 | `src/board/runner.rs` `tc_3832_ask_resume_modes` |
| TC-3833 | FR-3833 | unit | As above | ask auto | Thread prefix and JSON resume.mode | `src/board/runner.rs` `tc_3833_ask_reports_resume_mode` |
| TC-3834 | FR-3834 | unit | Item in progress with live lock | ask auto | summary mode, never native | `src/board/runner.rs` `tc_3834_ask_during_run_uses_summary` |
| TC-3835 | FR-3835 | unit | Any ask | Inspect child argv/env | `--read-only` present; no `--allow-sensitive`/`--trust-workspace-config`; no decision written | `src/board/runner.rs` `tc_3835_ask_resume_stays_read_only` |
| TC-3836 | FR-3836 | unit (asset) + e2e (browser, ignored) | Item with/without work session | Open modal | Selector shown only with a work session; request carries `resume` | `src/web/assets.rs` `tc_3836_board_chat_resume_selector`, `tests/web_board_e2e.rs` `tc_3836_*` |
