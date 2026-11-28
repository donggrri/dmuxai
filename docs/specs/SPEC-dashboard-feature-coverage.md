# SPEC: Dashboard feature inventory, spec coverage and test coverage

Status: approved (Dongjin 2026-11-28, t793u)
Owner: Dongjin
Created: 2026-11-28
Baseline: `main` after PR #370 and #371
Related: SPEC-board-dashboard-acceptance, SPEC-spec-review-gate, SPEC-web-dashboard, SPEC-web-local-access, SPEC-chat-followup-queue, SPEC-stage-skills, SPEC-obsidian-memory, SPEC-image-attachment, SPEC-workspace-dir, SPEC-workspace-open-headless, SPEC-concurrent-conversations

## 1. Decision summary

Dongjin (t793u): every dashboard feature gets a spec and tests. This document is the inventory of the features the web dashboard (`src/web/**`, assets `src/web/assets/*`) offers, derived from the code (routes in `src/web/*.rs`, toggles and panels in `index.html` and the asset scripts). Each feature is mapped to the requirement IDs and test IDs that already specify and test it (§7). Features with no spec get requirements here (FR-3750..FR-3763); features whose existing TCs are only manual or unlinked get automated tests here. Existing tests that have no TC id are linked by renaming the test function to the `tc_<id>_` form.

## 2. Scope

### In scope

- Every user-visible dashboard feature and every `/api/*` route used by the dashboard assets.
- Linking existing untagged browser tests to TC ids; new unit/HTTP tests and headless-Chrome e2e tests (`#[ignore]`d like the other `web_*` browser tests) for the gaps.
- Human-only rules for the remaining dashboard approvals (skill pin approval, memory proposals).

### Out of scope / non-goals

- Visual design changes. Manual screenshot TCs stay manual unless a requirement below automates them.
- Terminal sessions (`dmuxai term`): a CLI/TUI feature with no dashboard surface (listed in §7 for completeness).
- Fixing open issues found by the tests (they are linked, e.g. #369, and the test reports `KNOWN-BUG #N` until fixed, like TC-3605).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3750 | Skill-pin approval and memory proposal approve/reject are owner approvals, so they get the human browser header rule of board decisions (FR-3762). Scripts that relied on calling them without the headers must use the CLI | Dongjin | Answered by the standing rule "approval is always human-only" |

## 3. Test environment (normative for the tests)

Same as SPEC-board-dashboard-acceptance §3: each test starts its own `dmuxai --web --port 0` / `serve` (or the in-process router for HTTP tests) with a temp workspace, temp `DMUXAI_STATE_DIR`, temp `XDG_CONFIG_HOME`, no token, fake agent CLIs, no network. Browser tests find Chrome like the other web tests and print `SKIP` and pass without Chrome or Playwright. Screenshots, when taken, never show a token or the dashboard URL with a token.

## 4. Requirements

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-3750 | Must | Copy reply: every agent message in the chat has exactly one copy button (system and user lines have none); it copies the raw Markdown source of that message, shows a `Copied` state, keeps working across snapshot re-renders, and falls back to `document.execCommand('copy')` when `navigator.clipboard` is missing. |
| FR-3751 | Must | Single send: Enter or Send posts a prompt exactly once and shows the user's message immediately (optimistic echo); a second Enter or click while the first request is in flight does not post again; the echo is replaced by the server's copy of the prompt (never duplicated, never dropped while the server shows it) and removed when the server refuses the prompt. |
| FR-3752 | Must | Model menu: the composer agent/model chip opens a menu with a `Search models` box that filters agents and models; arrow keys and Enter pick a model or an effort; Esc closes the menu and returns focus to the chip; focus and the typed query survive snapshot re-renders; the menu never offers a fast model or a Fast toggle. |
| FR-3753 | Must | Follow-up queue (SPEC-chat-followup-queue FR-1230..FR-1236): the existing browser test is the evidence for TC-1230..TC-1236 and is linked to those ids. |
| FR-3754 | Must | Top bar Chat button: from Graph, Diff, Memory, Runs and Board views, the Chat button returns to the chat view with the current conversation and composer focus; the active view toggle shows `aria-pressed="true"` and only one view is visible at a time. |
| FR-3755 | Must | More menu: the `More` toggle opens a menu with the panels available in the current mode (Runners, Skills, Agent prompts, Memory, Jira, MCP import, Board rules, all projects where enabled); Esc, a click outside and a second click on the toggle close it; each entry opens its panel; in view-only `serve` entries that change files or start agents are hidden or disabled. |
| FR-3756 | Must | Board rules panel: it shows the default lane and the kind→lane rules from `GET /api/board/rules`; saving sends `PUT /api/board/rules` with the human headers and shows the saved rules; an invalid rule shows the server's error and changes nothing; a scripted PUT without the human headers is refused with 403. |
| FR-3757 | Must | Board live updates: with the Board view open, an item added with `dmuxai board add`, a lane move or a new thread message made outside the page appears within 3 s without a reload (board events stream); closing the page ends the stream. |
| FR-3758 | Must | Dashboard image attachments: the composer accepts a PNG/JPEG/GIF/WebP via the file picker and paste, uploads it to `POST /api/attachments`, shows a chip that can be removed (`DELETE /api/attachments/{name}`), and sends the attachment with the next prompt; an unsupported type or a file over the limit is refused with a visible notice and no chip; attachment names with path separators or `..` are refused by the API. |
| FR-3759 | Must | Conversation switching (automates SPEC-dashboard-fullwidth TC-1220, SPEC-workspace-open-headless TC-1489 and SPEC-concurrent-conversations TC-1622): switching A → B → A in the session sidebar shows each conversation's own messages and run rows; switching to A while it streams shows the live text growing and the final reply when done. |
| FR-3760 | Must | Workspace dialog validation (automates SPEC-workspace-open-headless TC-1490): for a git repo and a plain folder Switch/Open are enabled; for a missing path the dialog says `does not exist` and they are disabled; for an unreadable folder it shows the error and they are disabled; Up and a recent pick fill the path. |
| FR-3761 | Must | Skills map (SPEC-stage-skills UX-1500, FR-1513): `GET /api/skills` returns the stage→skill map with status; the Settings Skills section shows it; saving writes `[stages.<stage>] skills` to the workspace `dmuxai.toml` after the confirm dialog, keeping other keys; view-only `serve` refuses writes. |
| FR-3762 | Must | Human-only approvals: `POST /api/skills/approve`, `POST /api/memory/proposals/{id}/approve` and `POST /api/memory/proposals/{id}/reject` require the human browser headers (`Sec-Fetch-Site: same-origin`, non-empty `Sec-Fetch-Mode`, `X-Dmuxai-Human: 1`) like board decisions and are refused with 403 otherwise, changing nothing; the dashboard assets send the headers. |
| FR-3763 | Must | Memory panel: it lists notes and pending proposals from `GET /api/memory`; Save stores a note; Approve / Reject on a proposal call the endpoints of FR-3762 and update the list; with memory not configured the panel shows the not-configured message; view-only `serve` hides Save. |
| NFR-3750 | Must | Each new browser test finishes in under 60 s, leaves no child process behind, never touches the real user config, state or ports 7420 and 7472, and prints `SKIP` (passing) without Chrome or Playwright. New HTTP/unit tests run in the normal gate. |

## 5. Acceptance scenarios and test design

| ID | Covers | Type | Test |
| --- | --- | --- | --- |
| TC-3750 | FR-3750 | e2e (ignored) | `tests/web_chat_copy.rs` `tc_3750_copy_button_copies_raw_markdown_in_headless_chrome` (existing test renamed) |
| TC-3751 | FR-3751 | e2e (ignored) | `tests/web_chat_single_send.rs` `tc_3751_*` (existing tests renamed) |
| TC-3752 | FR-3752 | e2e (ignored) | `tests/web_model_menu.rs` `tc_3752_model_menu_search_keyboard_and_no_fast_in_headless_chrome` (existing test renamed) |
| TC-3753 | FR-3753 | e2e (ignored) | `tests/web_followup_queue.rs` `tc_1230_1231_1232_1233_1234_1235_1236_3753_followups_queue_in_order_and_survive_stop` (existing test renamed) |
| TC-3754 | FR-3754 | e2e (ignored) | `tests/web_dashboard_views.rs` `tc_3754_chat_button_returns_from_every_view` |
| TC-3755 | FR-3755 | e2e (ignored) | `tests/web_dashboard_views.rs` `tc_3755_more_menu_opens_closes_and_respects_view_only` |
| TC-3756 | FR-3756 | http + e2e (ignored) | `src/web/board_api.rs` `tc_3756_rules_put_requires_human_headers`; `tests/web_board_e2e.rs` `tc_3756_board_rules_panel_edit_and_error` |
| TC-3757 | FR-3757 | e2e (ignored) | `tests/web_board_e2e.rs` `tc_3757_board_updates_live_without_reload` |
| TC-3758 | FR-3758 | http + e2e (ignored) | `src/web/server.rs` `tc_3758_attachments_api_types_limits_and_names`; `tests/web_attachments.rs` `tc_3758_composer_attach_chip_remove_and_refuse` |
| TC-3759 | FR-3759 | e2e (ignored) | `tests/web_session_sidebar.rs` `tc_3759_switch_conversations_keeps_history_and_live_stream` |
| TC-3760 | FR-3760 | e2e (ignored) | `tests/web_workspace_dir.rs` `tc_3760_workspace_dialog_validation_states` |
| TC-3761 | FR-3761 | http | `src/web/server.rs` `tc_3761_skills_map_get_post_and_view_only` |
| TC-3762 | FR-3762 | http + unit (asset) | `src/web/server.rs` `tc_3762_skill_and_memory_approvals_need_human_headers`; `src/web/assets.rs` `tc_3762_assets_send_human_headers_for_approvals` |
| TC-3763 | FR-3763 | e2e (ignored) | `tests/web_memory_panel.rs` `tc_3763_memory_panel_notes_proposals_and_view_only` |
| TC-3764 | NFR-3750 | meta | Every new browser test above uses the shared temp-env helper and a 60 s deadline; `cargo test --test 'web_*' -- --ignored` passes |

## 6. Spec text amendments

- SPEC-chat-followup-queue: TC-1230..TC-1236 evidence points to the renamed test (FR-3753).
- SPEC-dashboard-fullwidth TC-1220, SPEC-workspace-open-headless TC-1489/TC-1490, SPEC-concurrent-conversations TC-1622: add "automated by TC-3759 / TC-3760" (manual screenshots stay manual).

## 7. Coverage table (feature → requirements → tests)

Legend: **auto** = automated test exists; **new** = test added by this spec or SPEC-spec-review-gate; **manual** = only manual TCs; **n/a** = not a dashboard feature.

| Feature | Code (main entry) | Spec / requirement IDs | Test IDs | Status |
| --- | --- | --- | --- | --- |
| Dashboard server, snapshot, SSE events, prompt, command | `server.rs`, `snapshot.rs`, `app.js` | SPEC-web-dashboard FR-520..FR-984 | TC-520..TC-987 (34) | auto |
| Dashboard-only `--web` mode, default mode | `mod.rs`, `server.rs` | SPEC-web-dashboard-only FR-1600..1604; SPEC-web-default-mode FR-2970..2978 | TC-1600..1602, TC-2970..2979 (TC-1603/1604 manual) | auto + manual |
| Local access, token, tokenless loopback | `access.rs`, `auth.rs`, `local_peer.rs` | SPEC-web-local-access FR-2950..2964; SPEC-dashboard-token-docs FR-2513..2515 | TC-2950..2965, TC-2964 (`web_tokenless_dashboard`), TC-2513..2515 | auto |
| View-only `serve` | `server.rs`, `board_api.rs` | SPEC-web-dashboard FR-531; SPEC-board-dashboard-acceptance FR-3610 | TC-531, TC-3610 | auto |
| All projects view | `global_api.rs`, `all-projects.js` | SPEC-web-all-projects FR-2980..2986 | TC-2980..2987 | auto |
| Multi-workspace host, workspace tabs | `hub.rs`, `workspace.js` | SPEC-multi-workspace-host FR-2300..2314 | TC-2300..2315 | auto |
| Workspace dialog (switch/open/validate/dirs) | `directory.rs`, `workspace.js` | SPEC-workspace-dir FR-870..1836; SPEC-workspace-open FR-876; SPEC-workspace-open-headless FR-1470..1475; FR-3760 | TC-870..1836, TC-879, TC-1471, TC-1985, TC-3760 | auto + **new** |
| Session sidebar, conversation switching | `session.js` | SPEC-session-sidebar FR-860..865; SPEC-concurrent-conversations FR-1620..1669; FR-3759 | TC-860..869, TC-863, TC-3759 | auto + **new** |
| Sidebar resize | `sidebar-resize.js` | SPEC-sidebar-resize FR-970..974 | TC-970..972 | auto |
| Full-width layout, run rows | `app.css`, `session.js` | SPEC-dashboard-fullwidth FR-1200..1985 | TC-1204..1224 auto; TC-1203/1210/1213/1220/1226 manual (TC-1220 automated by TC-3759) | auto + manual |
| Centered prompt | `session.js` | SPEC-centered-prompt FR-850..852 | TC-850, TC-851 | auto |
| Chat composer: single send | `session.js` | FR-3751 | TC-3751 | **new** (linked) |
| Chat: copy reply | `session.js` | FR-3750 | TC-3750 | **new** (linked) |
| Chat: model menu | `session.js` | SPEC-open-model-lists, SPEC-model-policy (no fast); FR-3752 | TC-3752 | **new** (linked) |
| Chat: follow-up queue | `session.js` | SPEC-chat-followup-queue FR-1230..1237; FR-3753 | TC-1230..1237, TC-3753 | **new** (linked) |
| Chat Markdown rendering | `chat_markdown.rs` | SPEC-remove-modes-chat-markdown FR-1308..1402 | TC-1309..1401 | auto |
| Image attachments | `attachments_api.rs`, `session.js` | SPEC-image-attachment FR-1890..1895 (TUI); FR-3758 | TC-3758 | **new** |
| Top bar view toggles, Chat button | `app.js`, `index.html` | FR-3754 | TC-3754 | **new** |
| More menu | `app.js`, `index.html` | FR-3755 | TC-3755 | **new** |
| Running indicators, favicon, title | `app.js`, `favicon-*.svg` | SPEC-running-indicators FR-820..982 | TC-820..985, TC-824 | auto |
| Task progress, task click | `session.js` | SPEC-task-progress-visibility FR-20..62; SPEC-task-click-progress FR-1310 | TC-60..63, TC-1310 (TC-64 manual) | auto + manual |
| Graph view, swimlane, plan header | `app.js` | SPEC-graph-view FR-700..2422; SPEC-graph-swimlane FR-880..885; SPEC-graph-plan-header FR-460..462 | TC-700..2422, TC-705, TC-881, TC-882, TC-460..462 | auto |
| Runs view (headless runs) | `runs_api.rs`, `runs.js` | SPEC-headless-runs-dashboard FR-2800..2817; SPEC-dashboard-ui-polish FR-2840..2846 | TC-2800..2817, TC-2811..2846 | auto |
| Tab trim | `index.html` | SPEC-dashboard-tab-trim FR-2820..2828 | TC-2820..2828 | auto |
| Docs tree, preview, diff view | `docs_api.rs`, `docs.js` | SPEC-docs-tree FR-830..833; SPEC-docs-preview FR-740..2262 | TC-830..834, TC-740..2267 | auto |
| Markdown path viewer | `md-viewer.js`, `docs_api.rs` | SPEC-board-ux-042 FR-3414..3417; FR-3603 | TC-3414..3417, TC-3603 | auto |
| Todo sidebar, todo links | `app.js` | SPEC-todo-sidebar FR-840..842; SPEC-todo-links FR-2020..2027 | TC-840..842, TC-2020..2023 | auto |
| Jira todos | `jira-todos.js` | SPEC-jira-todos FR-2400..2411 | TC-2400..2411 | auto |
| Agent prompts panel | `agent-prompts.js` | SPEC-agent-system-prompt (TC-1720) | TC-1720 | auto |
| MCP import panel | `server.rs` `/api/import` | SPEC-web-dashboard FR-569..579 | TC-569, TC-579 | auto |
| Memory panel | `memory.js` | SPEC-obsidian-memory FR-720..733; FR-3762, FR-3763 | TC-720..731, TC-3762, TC-3763 | auto + **new** |
| Settings, skills map | `skills.js`, `server.rs` | SPEC-dashboard-settings-ux FR-2360..2369; SPEC-stage-skills UX-1500, FR-1513; FR-3761, FR-3762 | TC-2360..2369, TC-3761, TC-3762 (TC-1525 manual) | auto + **new** |
| Theme colors | `app.css` | SPEC-theme-colors FR-600..604 | TC-600..609 | auto |
| Dashboard parity with TUI | `parity.rs` | SPEC-dashboard-parity FR-780..784 | TC-780..787 | auto |
| Board: cards, lanes, item modal | `board.js`, `board_api.rs` | SPEC-decision-board FR-3100..3138; SPEC-board-ux-042 FR-3412..3422; FR-3600 | TC-3100..3138, TC-3412..3422, TC-3600 | auto |
| Board: ask / Send / Note only | `board.js`, `board/runner.rs` | FR-3132, FR-3418; FR-3601, FR-3602 | TC-3132, TC-3418, TC-3601, TC-3602 | auto |
| Board: plan review (approve / reject / run / auto-run) | `board.js`, `board_api.rs` | SPEC-board-approve-autorun FR-3230..3237; FR-3604..3608; SPEC-spec-review-gate FR-3715 | TC-3230..3237, TC-3604..3608, TC-3715 | auto + **new** |
| Board: spec review | `board.js`, `board_api.rs` | SPEC-spec-review-gate FR-3711..3714 | TC-3711..3714 | **new** |
| Board: rules panel | `board.js` | SPEC-decision-board (rules); FR-3756 | TC-3756 | **new** |
| Board: live updates | `board_api.rs` `/api/board/events` | SPEC-decision-board; FR-3757 | TC-3757 | **new** |
| Runners panel | `runners.js`, `runner_status.rs` | SPEC-runner-status FR-3300..3311; FR-3609 | TC-3300..3312, TC-3609 | auto |
| Terminal sessions | `src/term/**` (CLI/TUI) | SPEC-terminal-sessions FR-2600..2693 | TC-2600..2695 | n/a (no dashboard surface) |

Summary: 42 features inventoried (41 with a dashboard surface, plus terminal sessions as n/a). 26 were already covered by automated tests (3 of them also keep manual screenshot TCs). 15 get new or newly linked tests here or in SPEC-spec-review-gate: 4 existing untagged browser tests are linked (single send, copy reply, model menu, follow-up queue), 6 features had no spec or test at all (attachments in the dashboard, Chat button, More menu, board rules panel, board live updates, board spec review) and 5 partly covered features get new tests (workspace dialog states, conversation switching, memory panel, skills map with human-only approvals, board plan review label). Remaining manual-only TCs are screenshot checks (TC-1203, TC-1210, TC-1213, TC-1226, TC-1525, TC-1603, TC-1604, TC-64).
