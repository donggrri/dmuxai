# SPEC: Board and dashboard feature acceptance (v0.4.2)

Status: approved · Target: v0.4.2
Owner: DmuxAI
Created: 2026-11-28
Baseline: `main` after PR #363, #364, #365, #366
Related: SPEC-decision-board, SPEC-board-ux-042, SPEC-board-approve-autorun, SPEC-runner-status, SPEC-headless-gate-env, SPEC-web-local-access, SPEC-spec-review-gate (board spec review and the "Plan review" label, FR-3711..FR-3715, TC-3714 extends `tests/web_board_e2e.rs`), SPEC-dashboard-feature-coverage

## 1. Purpose

The board and dashboard features of v0.4.2 were specified in four documents and
tested mostly with unit tests and asset-string checks. Nothing proves that the
pieces work **together in a browser**: a person creates an item, opens it,
chats, reads a linked Markdown file, approves it and watches it run, rejects
another, and checks which agent CLIs are signed in. This spec defines those
user-level acceptance criteria and maps each to an automated browser test
(headless Chrome via Playwright, `#[ignore]`d like the other `web_*` browser
tests) that runs against the real `dmuxai --web` / `dmuxai serve` binary with
fake agent children.

## 2. Gap analysis

| Feature | Covered by | Gap closed here |
| --- | --- | --- |
| Item detail as centered modal | TC-3412 (asset strings) | real geometry, Esc/backdrop/close, scroll lock in a browser |
| Chat Send answered by planner | TC-3418 (asset strings), TC-3401 (unit) | click Send in the browser and see a `planner` reply; argv is read-only and never `--allow-sensitive`; Note only |
| Markdown path viewer | TC-3414..3417 (router/unit/asset) | click a path in a real chat bubble; outside and hidden paths are refused in the UI |
| Approve → wake bot + auto-run | TC-3230..3237 (unit/router) | click Approve in the browser, item runs, UI shows the result; opt-out; stale plan hash |
| Run button, reject | TC-3133/3134 (unit/asset) | click Run on an approved item; Reject moves to Rejected and never runs |
| Human-only approval | TC-3138 (unit/router) | a scripted request without browser headers cannot decide |
| Runners panel | TC-3310 (asset), TC-3308 (router) | open the panel, see badges and masked account, toggle enable and see it persist |
| View-only `serve` | TC-3134 (asset) | no Ask/Run controls, Send store-only, approve does not launch |

## 3. Test environment (normative for the tests)

- Each test starts its own `dmuxai --web --port 0` (or `serve`) with a temp
  workspace, temp `DMUXAI_STATE_DIR`, temp `XDG_CONFIG_HOME`, and no token.
- `DMUXAI_BOARD_CHILD_BIN` points at a fake shell script standing in for
  `dmuxai` for board asks and runs. It records its argv to a file and prints
  the JSON a real child prints (`exit_code`, `text`, `run_id`).
- Runner status uses fake `DMUXAI_CURSOR_BIN` / `DMUXAI_AGY_BIN` scripts.
- Chrome is found like the other web tests (`DMUXAI_CHROME`, standard paths);
  without Chrome or Playwright a test prints `SKIP` and passes.
- No real agent, network or company data is used.

## 4. Requirements

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-3600 | Must | An item created with `dmuxai board add` appears as a card in the dashboard Board view (My decisions for `needs_decision`); clicking the card opens the detail as a centered modal: the dialog's center is within 5% of the viewport center horizontally and vertically, it never extends past the viewport, `role="dialog"` and `aria-modal="true"` are set, the page behind does not scroll, and Esc, a backdrop click and the close button each close it while a click inside does not. |
| FR-3601 | Must | In `--web`, typing a question that contains sensitive words (for example "merge the db migration") in the item chat and pressing Send posts the owner message, shows a busy indicator, and then shows a `planner` reply without a page reload; the child was started with `--read-only` and `--json`, never with `--allow-sensitive`, and no exit-7 error text appears. |
| FR-3602 | Must | With "Note only" checked, Send stores the owner message and starts no planner child (no planner message appears, the fake child is not invoked). |
| FR-3603 | Must | A Markdown path in a chat bubble (workspace-relative and `~/`-style) is a link; clicking it opens a read-only viewer modal showing the rendered file with no raw `<script>` element; a path outside the workspace and home, a hidden path under home, a non-`.md` file and a symlink escape each show an error message in the viewer and none of the file content. Esc closes the viewer first and leaves the item modal open. |
| FR-3604 | Must | Clicking Approve on an item that has a plan (as a human click in the browser) records an owner decision, wakes any blocked `board wait` and starts the run: the response reports `auto_run.started = true`, the UI says the run started, the item moves to In progress and then Done when the (fake) run exits 0, and a `run` thread message with the run id is present. |
| FR-3605 | Must | Auto-run is refused, with the decision still recorded, when `[board] auto_run_on_approve = false` (UI: an "auto-run skipped" notice that names the reason), and Approve with a plan hash that no longer matches the item's plan (changed after the card was rendered) is rejected with a "plan changed" notice and records no decision and starts no run. |
| FR-3606 | Must | An approved item whose run was not auto-started shows a Run button; clicking it starts the run, a second Run while it is in progress is refused (conflict), and the Run button is not shown for items that are not runnable. |
| FR-3607 | Must | Clicking Reject records an owner `reject` decision, moves the item to the Rejected lane, starts no run and offers no Run button. |
| FR-3608 | Must | A scripted `POST /api/board/{id}/decision` without the human browser headers (`Sec-Fetch-Site`, `Sec-Fetch-Mode`, `X-Dmuxai-Human: 1`) is refused with 403, records nothing and starts no run. |
| FR-3609 | Must | The More menu opens a Runners panel that lists cursor-agent and agy with a status badge, version and masked account derived from the fake CLIs (the plain email never appears in the page), and toggling the enable switch of a runner writes `[runners.<id>] enabled = false` to the user config and shows the Disabled badge; toggling it back removes the key. Opening the dashboard alone spawns no probe. |
| FR-3610 | Must | In view-only `dmuxai serve` the board shows no Ask, Ask & revise, Run or Note-only controls; Send is store-only; Approve records the decision, starts no run and the thread shows `auto-run skipped: view-only dashboard`. |
| NFR-3600 | Must | Each browser test finishes in under 60 s, leaves no child process behind and never touches the real user config, state or ports 7420 and 7472. |

## 5. Acceptance scenarios and test design

| ID | Covers | Test |
| --- | --- | --- |
| TC-3600 | FR-3600, NFR-3600 | `tests/web_board_e2e.rs` `tc_3600_board_card_opens_centered_modal` |
| TC-3601 | FR-3601 | `tests/web_board_e2e.rs` `tc_3601_chat_send_gets_read_only_planner_reply` |
| TC-3602 | FR-3602 | `tests/web_board_e2e.rs` `tc_3602_chat_note_only_stores_without_planner` |
| TC-3603 | FR-3603 | `tests/web_board_e2e.rs` `tc_3603_md_path_link_opens_viewer_and_refuses_bad_paths` |
| TC-3604 | FR-3604 | `tests/web_board_e2e.rs` `tc_3604_approve_with_plan_auto_runs_to_done` |
| TC-3605 | FR-3605 | `tests/web_board_e2e.rs` `tc_3605_auto_run_opt_out_and_stale_plan_hash` (known deviation: the visible "Plan changed" notice is not shown, issue #369; the test reports it as `KNOWN-BUG #369` until fixed, then it becomes a hard check) |
| TC-3606 | FR-3606 | `tests/web_board_e2e.rs` `tc_3606_run_button_runs_approved_item_once` |
| TC-3607 | FR-3607 | `tests/web_board_e2e.rs` `tc_3607_reject_moves_to_rejected_and_never_runs` |
| TC-3608 | FR-3608 | `tests/web_board_e2e.rs` `tc_3608_scripted_decision_without_human_headers_refused` |
| TC-3609 | FR-3609 | `tests/web_board_e2e.rs` `tc_3609_runners_panel_status_masking_and_enable_toggle` |
| TC-3610 | FR-3610 | `tests/web_board_e2e.rs` `tc_3610_view_only_serve_hides_launch_controls` |

## 6. Manual verification checklist (informative)

Run once per release candidate with the real binary and real agent CLIs; not
part of the gate: sign-in state of cursor-agent and agy in the Runners panel
matches `cursor-agent status` / `agy models`; a real planner answers a board
question that contains "merge" or "db"; a real approved plan runs to Done.
