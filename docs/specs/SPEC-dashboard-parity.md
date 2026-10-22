# SPEC: Dashboard parity with the TUI

> **Superseded (mode picker):** User-facing Ask/Plan/Implement selection was removed in
> SPEC-acp-runners FR-1306. `set_mode` and dashboard mode fields are ignored with a notice;
> other parity rows in this spec remain in force.

Status: Implemented in v0.2.0 (slice G2)
Owner: DmuxAI
Created: 2026-10-17
Baseline: `feat/agent-api`
Related: SPEC-agent-api (FR-765 `POST /api/prompt`), SPEC-web-dashboard, SPEC-theme-colors

## 1. Decision summary

Users want to do everything from the web dashboard that they can do in the
TUI. With `dmuxai --web` the dashboard gets a prompt box with a live
transcript, agent/model/mode/effort pickers, every slash command (buttons and
a command palette), cancel, new/load conversation, share and status. A parity
matrix in this spec maps each TUI command and key to a dashboard control, and
a test fails when a TUI slash command has no dashboard mapping. `dmuxai serve`
stays view-only.

## 2. Scope

### In scope

- Session state in the snapshot (TUI source only): active conversation,
  selected agent, per-agent mode/model/effort/busy and model catalogs,
  transcript tail with streaming text, recent conversations, slash command list.
- `POST /api/command` for picker and lifecycle actions, plus `POST /api/prompt`
  (SPEC-agent-api FR-765) for text and slash commands.
- Dashboard UI in its own JS module and CSS section: prompt box (Enter sends,
  Shift+Enter inserts a newline), transcript, pickers, command palette
  (Ctrl+K or `/` in an empty prompt box), cancel, conversations panel.
- Respect `[graph] allowed_runners`: disallowed runners are shown disabled and
  rejected by the server.

### Out of scope / non-goals

- Image paste/upload from the browser (`/image` takes a workspace path only).
- Mouse text selection and pane resizing (terminal-only affordances).
- Remote multi-user sessions; one TUI process owns the session.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-780 | `/quit` from the dashboard stops the whole `dmuxai --web` process after a confirmation dialog. Decided. | DmuxAI | Spec approval |
| OQ-781 | The snapshot transcript keeps the newest 100 messages, 32 KiB per message (cut with a marker) and 512 KiB in total; `transcript_omitted` counts the rest. Decided (smaller than first drafted because every SSE frame carries the whole snapshot). | DmuxAI | Spec approval |

## 3. Parity matrix

| TUI command / key | Dashboard control (`data-parity` id) | Server action | Test |
| --- | --- | --- | --- |
| Enter (send prompt) | prompt box Enter / Send button (`prompt-send`) | `POST /api/prompt` | TC-782 |
| Shift+Enter / multiline | prompt box Shift+Enter (`prompt-newline`) | — (client) | TC-786 |
| `/agent` | agent picker (`agent`) | `select_agent` | TC-783 |
| `/mode` | mode picker (`mode`) | `set_mode` | TC-783 |
| F2 (model picker) | model picker (`model`); "Load models…" fetches the catalog | `load_models`, `select_model` | TC-783 |
| `/effort` | effort picker (`effort`) | `set_effort` | TC-783 |
| `/status` | Status button (`status`) | `status` | TC-783 |
| `/quit` | Quit button with confirm (`quit`) | `quit` | TC-783 |
| `/new` | New conversation button (`new`) | `new_conversation` | TC-784 |
| `/conversations`, F3 | conversations panel (`conversations`) | snapshot `session.conversations` | TC-784 |
| `/resume <id>` | conversations panel Load (`resume`) | `load_conversation` | TC-784 |
| `/summarize` | palette entry (`summarize`) | `POST /api/prompt` `/summarize` | TC-785 |
| `/compact` | palette entry (`compact`) | `POST /api/prompt` `/compact` | TC-785 |
| `/share` | Share menu (`share`) | `share` | TC-783 |
| `/image <path>` | palette entry with path input (`image`) | `POST /api/prompt` `/image <path>` | TC-785 |
| `/details`, Ctrl+O | Details toggle (`details`) | `toggle_details` | TC-783 |
| `/graph [continue\|override]` | palette entry with status/continue/override (`graph`) | `POST /api/prompt` `/graph ...` | TC-785 |
| `/remember <text>` | palette entry with note input (`remember`) | `POST /api/prompt` `/remember ...` | TC-787 |
| `/memory [approve\|reject <id>]` | palette entry (`memory`); the Memory panel also approves/rejects | `POST /api/prompt` `/memory ...` | TC-787 |
| Ctrl+C (cancel turn) | Cancel button per busy agent (`cancel`) | `cancel` | TC-783 |
| Esc (close overlay) | Esc closes palette/pickers (`escape`) | — (client) | TC-786 |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-780 | Must | Given `dmuxai --web` | Then `/api/state` and SSE snapshots include `session` with `conversation_id`, `conversation_title`, `selected_agent`, `agents[]` (`id`, `label`, `mode`, `modes`, `model`, `effort`, `busy`, `allowed`, `models_loading`, `models[]` with `efforts`), `transcript[]` (`id`, `role` user/agent/system/activity, `agent`, `content`, `streaming`), `transcript_omitted`, `details_expanded`, `conversations[]` (`id`, `title`, `updated_at_ms`) and `commands[]` (`name`, `description`) | `serve`, headless and plain TUI snapshots have no `session`; transcript capped per OQ-781 | TC-780, TC-781 |
| FR-781 | Must | Given a valid token and `POST /api/command {command, ...}` | Then `select_agent`, `set_mode`, `load_models`, `select_model`, `set_effort`, `cancel`, `new_conversation`, `load_conversation`, `share`, `status`, `toggle_details`, `quit` perform the same App action as the TUI command and return 200 with the updated `session` | Unknown command or bad value → 400; unknown agent/conversation/model → 404; disallowed runner → 403; busy agent for mode/model change → 409; `serve` → 409 | TC-783, TC-784 |
| FR-782 | Must | Given the prompt box | Then Enter submits via `POST /api/prompt`, Shift+Enter inserts a newline, and the agent reply streams into the transcript as snapshots arrive | Submit errors are shown inline and the text is kept | TC-782, TC-786 |
| FR-783 | Must | Given the command palette (Ctrl+K, or `/` in an empty prompt box) | Then every entry of `session.commands` is listed, filterable, and runs its mapped control | Commands needing an argument ask for it first | TC-785, TC-786 |
| FR-784 | Must | Given the parity matrix | Then every TUI slash command has a dashboard mapping in `src/web/parity.rs`, and every mapped control id exists in the dashboard assets | A TUI command without a mapping fails `tui_commands_have_dashboard_controls` | TC-787 |
| UX-780 | Should | Given a busy agent | Then its Cancel button is visible and mode/model pickers for it are disabled | — | TC-786 |
| NFR-780 | Must | Given parallel dashboard work (MCP, memory, docs, graph) | Then the parity UI lives in its own `assets/session.js` and `assets/session.css`; `app.css` is untouched and `app.js` only re-dispatches each snapshot as a `dmuxai:snapshot` DOM event so the page keeps one SSE connection (`index.html` gains the container and two tags) | — | TC-786 (`session_module_has_parity_controls` asserts `session.js` opens no `EventSource`), manual review of the diff |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-780 | FR-780 | unit | App with test adapters and a hub | publish | snapshot `session` has all keys, selected agent and per-agent mode | `src/app.rs` `dashboard_session_snapshot_has_contract_keys` |
| TC-781 | FR-780 | unit | 250 messages, one 100 KiB | build session | 100 messages, 32 KiB cap with marker, ≤ 512 KiB, `transcript_omitted` 150; `serve` snapshot has no session | `src/app.rs` `dashboard_session_transcript_is_capped` |
| TC-782 | FR-782 | unit | App + `SubmitPrompt`, TUI draft in the input | submit multi-line text, stream deltas, complete | user message keeps newlines, agent message streams then settles; TUI draft kept | `src/app.rs` `dashboard_prompt_streams_into_transcript` |
| TC-783 | FR-781 | integration | router with command double; App handlers | each command, unknown, disallowed, busy, serve | statuses 200/400/403/404/409; App state changed as in the TUI | `src/web/server.rs` `command_route_contract`; `src/app.rs` `dashboard_commands_match_tui_actions`, `dashboard_cancel_targets_the_named_agent`, `dashboard_commands_cover_models_share_and_policy` |
| TC-784 | FR-781 | unit | App with two conversations | `new_conversation`, `load_conversation` | active conversation switches and transcript restores | `src/app.rs` `dashboard_new_and_load_conversation` |
| TC-785 | FR-783 | unit | App with a busy agent | `SubmitPrompt` with `/details`, `/graph status`, `/image p`, `/summarize`, then plain text | slash commands run like typed (allowed while busy, no agent request); plain text → busy | `src/app.rs` `dashboard_slash_commands_via_prompt` |
| TC-786 | FR-782, FR-783, UX-780 | unit (asset) | embedded `session.js` | static checks | Enter/Shift+Enter handling, Ctrl+K palette, Esc close, cancel button, disabled pickers present | `src/web/assets.rs` `session_module_has_parity_controls`, `session_assets_are_served_hardened`; browser run in REVIEW |
| TC-787 | FR-784 | unit | `SLASH_COMMANDS` and `parity.rs` | compare | every TUI command and key mapped; every control id present in `session.js`; rows unique | `src/app.rs` `tui_commands_have_dashboard_controls`; `src/web/parity.rs` `parity_matrix_controls_exist_in_session_js` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change | — | — |
| Security / permissions | `/api/command` and `/api/prompt` use the #27 token, Host and Origin checks; `serve` answers 409; allowed_runners enforced server-side | Unchanged #27 behavior | TC-783 |
| Compatibility | `session` is an additive optional snapshot field | Older clients ignore it | TC-780 |

## 7. Implementation plan

See [PLAN-dashboard-parity](../plans/PLAN-dashboard-parity.md).

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets` pass.
- [x] Screenshot of the dashboard prompt box, transcript and palette attached to the PR.
- [x] `../reviews/REVIEW-dashboard-parity.md` records commands, result, residual risk, and rollback notes.
