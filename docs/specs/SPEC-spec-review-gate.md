# SPEC: Human spec review and plan review gates on the board

Status: approved (Dongjin 2026-11-28, t793u)
Owner: Dongjin
Created: 2026-11-28
Baseline: `main` after PR #370 and #371
Related: SPEC-spec-first-workflow FR-3503 FR-3505 FR-3508, SPEC-decision-board FR-3138, SPEC-board-approve-autorun, SPEC-board-dashboard-acceptance, SPEC-plan-gate, SPEC-run-journal

## 1. Decision summary

Spec-first (SPEC-spec-first-workflow) makes every write run `spec → implement → test`, but nobody looks at the spec before code is written: the plan is approved before the spec exists, and the spec step flows straight into implementation. Dongjin decided (t793u) that **both the plan and the spec need a human check, and both must be visible on the board**:

1. **Plan review** is the existing human plan approval (board Approve with a plan hash, `--approve-plan-id`, TUI approval panel). It is unchanged in behavior; the board labels it clearly as "Plan review".
2. **Spec review** is new. When the spec step(s) of a run finish and pass the spec gate (FR-3508), the run pauses in the state `awaiting_spec_review`. A human reads the written spec in the board (existing safe Markdown viewer) or the CLI and chooses **Approve** (the run continues with implementation), **Request changes** with feedback (the spec step runs again with the feedback, then the run pauses for review again) or **Reject** (the run stops; no implementation runs). Implementation and test steps never start while a spec review is pending.
3. Approval stays human-only, with the same rules as plan approval: the dashboard needs the human browser headers (403 otherwise); the CLI needs an interactive terminal and no `DMUXAI_RUN_ID`; MCP, the agent API and agents have no way to decide. `--approve-plan` / `--approve-plan-id` never approve a spec.
4. The gate is on by default whenever `spec_first = "full"`; `[workflow] spec_review = "off"` or `--no-spec-review` turns it off.

Minimal-design rationale: the paused run keeps its own process and graph in memory and waits on a small review file under the state dir; the dashboard, the CLI and the TUI all write the same file. No daemon, no graph resume machinery and no database change are needed.

## 2. Scope

### In scope

- `[workflow] spec_review`, `spec_review_timeout_secs`, `spec_review_max_rounds`; `--no-spec-review` on `run` and `todo run`; MCP `run` `no_spec_review`.
- The `awaiting_spec_review` pause in the graph executor (headless, board child, TUI).
- The spec review record, its CLI (`dmuxai spec-review`), the board API and UI, the TUI panel.
- Run journal and board thread entries for spec review requests and decisions.
- The "Plan review" label for the existing plan approval on the board.

### Out of scope / non-goals

- Judging spec quality automatically.
- Editing the spec text inside the dashboard (the reviewer gives feedback; the agent edits).
- Write fan-out legs (unchanged, like SPEC-spec-first-workflow).
- `spec_first = "lite"` (one node, no separate spec step): no spec review in this version.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3700 | Default on with `spec_first = "full"`; headless runs without a board also wait for a human (they print the CLI command to approve). Automation that must not wait passes `--no-spec-review` or sets `spec_review = "off"` in its own user config | Dongjin | Answered 2026-11-28 (t793u: "사람의 확인 필요") |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| spec review | A pending human check of the spec files written by the spec step(s) of one graph run | Plan review, which approves the plan before any step runs |
| plan review | The existing human plan approval (board Approve of an item plan, TUI approval panel, `--approve-plan-id`) | `--approve-plan`, which a headless caller uses to auto-confirm a fresh plan |
| review id | The graph id of the paused run (same charset and length rules as graph ids) | Board item id |
| round | 1 for the first review of a run, +1 after each Request changes | Graph revision |
| spec digest | First 16 hex chars of sha256 over the sorted `path\0content\0` of every reviewed spec file | Plan hash |

Review record status: `pending` → `approved` \| `changes_requested` \| `rejected` \| `expired` \| `cancelled`. `changes_requested` goes back to `pending` (next round) when the re-run spec step finishes.

Graph run states added: `awaiting_spec_review` (between the last spec step's success and the first dependent step's start).

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3700 | Must | Given a user config or a trusted workspace `dmuxai.toml` with `[workflow]` | Then `spec_review` accepts `"on"` and `"off"` and defaults to `"on"` when `spec_first = "full"` and `"off"` otherwise; `spec_review_timeout_secs` (default 86400, min 60); `spec_review_max_rounds` (default 5, 1..=20). A workspace value overrides the user value key by key | Unknown values or wrong types are a config warning and the default is used. `spec_review = "off"` from an untrusted workspace is ignored with a warning (it removes a human gate) | TC-3700 |
| FR-3701 | Must | Given `dmuxai run`, `dmuxai todo run` or MCP `run` | Then `--no-spec-review` (MCP `no_spec_review: true`) turns the gate off for that run only; `--no-spec-first` implies it; `run --help` lists the flag; `--approve-plan`, `--approve-plan-id` and MCP `approve_plan_id` do not approve or skip a spec review | The flag never changes config | TC-3701 |
| FR-3702 | Must | Given the gate is on and a graph run has spec steps (role `spec`) | Then when every spec step that is an ancestor of a pending write step has succeeded and passed the spec gate (FR-3508), the executor creates a review record (FR-3703), sets the run state `awaiting_spec_review`, and starts no step that has a spec step as an ancestor until the record is `approved`. Steps that do not depend on a spec step and are read-only may keep running. The status line and progress stream show `spec review: waiting for a human · dmuxai spec-review approve <review-id>` with heartbeats every 30 s | A spec step that fails goes the normal retry/replan path and creates no review. With the gate off or no spec step, nothing changes | TC-3702 |
| FR-3703 | Must | Given a review is requested | Then dmuxai writes `<state>/spec-reviews/<review-id>.json` atomically (0600) with: `review_id`, `graph_id`, `run_id`, `workspace`, `prompt_summary` (first 200 chars, secrets masked), `spec_paths` (workspace-relative changed paths under `spec_dir` recorded by FR-3508, sorted), `spec_digest`, `round`, `status`, `created_ms`, `updated_ms`, optional `board_item_id`, and for a decision `verdict`, `by`, `feedback` (≤ 4 KB, secrets masked) and `decided_ms` | `spec_paths` never contains a path outside the workspace or outside `spec_dir`. A missing state dir is created | TC-3703 |
| FR-3704 | Must | Given a pending review | Then the waiting run polls its record at most twice per second; `approved` → the run continues with the next steps; `rejected` → no further step starts, running read-only steps are cancelled, pending steps are marked `skipped (spec rejected)` and the run ends with `ok: false` and error `spec rejected by reviewer` (exit 1); `changes_requested` → FR-3705 | A record that disappears or becomes unparsable is treated as `cancelled`: the run ends with `spec review cancelled` (exit 1). Ctrl-C / cancel of the run sets the record to `cancelled` | TC-3704 |
| FR-3705 | Must | Given a review decided `changes_requested` with feedback | Then the spec step(s) under review run again with the original job plus a DATA block `Reviewer feedback (round N): <feedback>` and the instruction to update the spec under `<spec_dir>` accordingly; the FR-3508 spec gate applies again; on success the record goes back to `pending` with `round + 1`, a new `spec_digest` and the new `spec_paths` | Empty feedback is refused (usage error) by every entry point. A Request changes after `spec_review_max_rounds` rounds is refused with `max spec review rounds reached; approve or reject` | TC-3705 |
| FR-3706 | Must | Given a pending review older than `spec_review_timeout_secs` | Then the record becomes `expired` and the run ends with `spec review timed out after Ns` (exit 1) and no implementation step runs | The board child timeout (FR-3712) does not end the run earlier while the review is pending | TC-3706 |
| FR-3707 | Must | Given a decision from any entry point | Then it is accepted only when the record is `pending`, the given `spec_digest` (when given; the dashboard always sends it) equals the record's digest, and the actor is a human (FR-3708). Accepting writes the record atomically under a lock; the first decision wins and a second decision gets a conflict | Digest mismatch → conflict `spec changed since it was shown; reload`. Not pending → conflict `spec review is not pending` | TC-3707 |
| FR-3708 | Must | Given a decision request | Then human-only rules are those of plan approval: the dashboard requires `Sec-Fetch-Site: same-origin`, a non-empty `Sec-Fetch-Mode` and `X-Dmuxai-Human: 1` and refuses otherwise with 403; the CLI requires stdin and stdout on a TTY and refuses when `DMUXAI_RUN_ID` is set (exit 5, `spec review decisions need an interactive terminal or the dashboard`); MCP and the agent API expose no spec-review decision tool; the actor recorded is always `owner` | A view-only `serve` dashboard may decide (deciding launches nothing new; the paused run belongs to its own process) | TC-3708 |
| FR-3709 | Must | Given the CLI | Then `dmuxai spec-review list [--json] [--all]` lists pending reviews (id, workspace, round, spec paths, age); `dmuxai spec-review show <id> [--json]` prints the record and the spec files' contents (Markdown as text, each capped at 256 KB); `dmuxai spec-review approve <id>`, `request-changes <id> --text <feedback>` and `reject <id> [--text <reason>]` decide. `<id>` is validated before any filesystem access | Unknown id → exit 3; invalid id → exit 2; conflict → exit 4; forbidden → exit 5 | TC-3709 |
| FR-3710 | Must | Given a decision or a request | Then the run journal records `spec_review_requested` (review id, round, spec paths, digest) and `spec_review_decided` (verdict, by, round, feedback length, not the text), and the graph file `.dmuxai/graphs/<id>.json` has `spec_review` on the run with the latest status and round | Secrets are masked in everything recorded | TC-3710 |
| FR-3711 | Must | Given a board item run (`board run`, Run button, auto-run after approve) | Then the board runner passes `DMUXAI_BOARD_ITEM_ID=<item id>` to the child; on a review request the child stores the review id on the item as `spec_review: {review_id, status, round, spec_paths, spec_digest}`, appends a thread message from `planner` `Spec review requested (round N): <paths>`, and the gate raises the board event and notification `spec_review_requested` (SPEC-coordinator-observability FR-3852; the single notification for a pending review, which `[board] notify_events = ["spec_review"]` also selects); on each decision the item's `spec_review.status` is updated and a thread message from `owner` records the verdict (and feedback for Request changes). The item stays in the In progress lane while paused | The child never changes the item's lane or decision; an item id that is invalid or not found is ignored with a warning | TC-3711 |
| FR-3712 | Must | Given a board child run is paused for spec review | Then the board child timeout (`DMUXAI_BOARD_CHILD_TIMEOUT_SECS`) is suspended while the item's `spec_review.status` is `pending` and resumes counting afterwards | — | TC-3712 |
| FR-3713 | Must | Given the dashboard API | Then `POST /api/board/{id}/spec-review` with `{"verdict": "approve" \| "request_changes" \| "reject", "text": "...", "spec_digest": "..."}` decides the item's pending review (FR-3707, FR-3708) and returns the updated item; `GET /api/board/{id}` includes `spec_review`; `GET /api/board/pending` includes items whose `spec_review.status` is `pending` | 400 invalid id/verdict or empty feedback for request_changes; 403 without human headers; 404 unknown item or no review; 409 not pending or digest mismatch | TC-3713 |
| FR-3714 | Must | Given the board UI | Then an item with a pending spec review shows a `Spec review` badge on its card and appears in My decisions; its detail modal shows a `Spec review (round N)` section listing each spec path as a link that opens the existing safe Markdown viewer (FR-3603 rules), and the buttons Approve spec, Request changes (needs a non-empty feedback text field) and Reject spec; after a decision the section shows the verdict and the buttons disappear. The UI sends the human headers and `spec_digest` | Errors (409/403) show a notice in the modal; a digest conflict tells the user to reload | TC-3714 |
| FR-3715 | Must | Given an item with a plan awaiting the owner (`needs_decision` with a plan) | Then its card shows a `Plan review` badge and the detail modal titles the plan block and its Approve / Reject controls `Plan review`; approving it is unchanged (FR-3604, FR-3605) | Items without a plan show no `Plan review` label | TC-3715 |
| FR-3716 | Must | Given the interactive TUI runs a graph with the gate on | Then on a review request the TUI shows a spec review panel with the spec paths, and keys `a` (approve), `c` (request changes: prompts for feedback) and `x` (reject); the decision goes through the same record (FR-3707) and the run reacts as in FR-3704/FR-3705 | Esc hides the panel without deciding; the status line keeps the waiting hint | TC-3716 |
| FR-3717 | Must | Given `dmuxai run --json` of a run that requested a review | Then the JSON output includes `spec_review: {review_id, status, round}`; while waiting, stderr progress (with `--progress`) prints the waiting hint once and heartbeats | — | TC-3717 |
| NFR-3700 | Must | Given a paused run | Then waiting uses < 1 % CPU (poll ≤ 2/s, no busy loop), adds no agent turn, and the time spent waiting is excluded from the dmuxai overhead measured for #301 (≤ 10 % per turn) | — | TC-3718 |
| NFR-3701 | Must | Given any review record or UI | Then no token, dashboard URL or secret appears in the record, the journal, the board thread or the UI | — | TC-3703, TC-3710 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3700 | FR-3700 | unit | TOML fixtures | Parse `[workflow]` with/without `spec_review`, with `spec_first` full/lite/off, invalid values, untrusted workspace `off` | Defaults on/off as specified; warnings; untrusted off ignored | `src/settings.rs` `tc_3700_*` |
| TC-3701 | FR-3701 | unit | — | Parse `run --no-spec-review`, `todo run --no-spec-review`, MCP `no_spec_review`; `--approve-plan` with gate on | Flag set; help lists it; approve-plan leaves the gate on | `src/cli.rs` / `src/mcp_server.rs` `tc_3701_*` |
| TC-3702 | FR-3702 | unit | Graph `spec-1 → step-1 → test-1` plus an independent research step; fake runner | Run until spec-1 succeeds | Record created; step-1 and test-1 not started; research step runs; state `awaiting_spec_review`; hint on progress | `src/graph/spec_review.rs` `tc_3702_*` |
| TC-3703 | FR-3703, NFR-3701 | unit | Temp state dir; spec gate paths | Create a record | File 0600, atomic, fields present, digest stable, secrets masked, paths only under spec_dir | `src/graph/spec_review.rs` `tc_3703_*` |
| TC-3704 | FR-3704 | unit | Paused fake run | Write approved / rejected / delete record / cancel | Continues; rejected ends ok=false with pending steps skipped; deleted → cancelled; cancel → record cancelled | `src/graph/spec_review.rs` `tc_3704_*` |
| TC-3705 | FR-3705 | unit | Paused fake run | Request changes with feedback; empty feedback; max rounds reached | Spec step re-run prompt contains the DATA block; round 2 pending with new digest; empty refused; max refused | `src/graph/spec_review.rs` `tc_3705_*` |
| TC-3706 | FR-3706 | unit | Timeout 1 s (test override) | Wait | Record expired; run fails with timeout message; no implement turn | `src/graph/spec_review.rs` `tc_3706_*` |
| TC-3707 | FR-3707 | unit | Pending record | Two concurrent approvals; stale digest; decide a non-pending record | First wins, second conflict; digest conflict; not-pending conflict | `src/graph/spec_review.rs` `tc_3707_*` |
| TC-3708 | FR-3708 | unit+http | Router with a pending item review; CLI helper | POST without human headers; CLI non-TTY; CLI with `DMUXAI_RUN_ID`; list MCP tools | 403 and record unchanged; exit 5; exit 5; no decide tool | `src/web/board_api.rs` / `src/cli.rs` / `src/mcp_server.rs` `tc_3708_*` |
| TC-3709 | FR-3709 | integration | Temp state with records | `spec-review list/show --json`, decide with a pseudo-TTY test hook, bad id, unknown id | Output fields; exit codes 2/3/4 | `tests/spec_review_cli.rs` `tc_3709_*` |
| TC-3710 | FR-3710, NFR-3701 | unit | Paused fake run with journal | Request and decide | Journal events with fields, no feedback text; graph file `spec_review` | `src/graph/spec_review.rs` `tc_3710_*` |
| TC-3711 | FR-3711 | unit | Board store; child env with item id | Request review, decide approve and request changes | Item `spec_review` updated, thread messages, no separate `spec_review` notification (the gate's `spec_review_requested` is covered by TC-3852), lane unchanged; bad item id ignored | `src/board/runner.rs` `tc_3711_*` |
| TC-3712 | FR-3712 | unit | Board child timeout 2 s; fake child pausing 4 s with item review pending | Run | Child not killed while pending; killed after timeout once not pending | `src/board/runner.rs` `tc_3712_*` |
| TC-3713 | FR-3713 | http | Router with items: pending review, none, decided | POST each verdict, bad verdict, empty feedback, stale digest, GET item, GET pending | 200/400/404/409 as specified; pending list includes the item | `src/web/board_api.rs` `tc_3713_*` |
| TC-3714 | FR-3714, FR-3715 | e2e (browser, ignored) | Real `dmuxai --web` in a temp git repo, real board child (no `DMUXAI_BOARD_CHILD_BIN`), fake cursor-agent script that writes `docs/specs/SPEC-x.md` in the spec step and a file in the implement step | Approve plan (badge `Plan review`) → run pauses → card shows `Spec review` → open spec link in viewer → Request changes with feedback → round 2 → Approve spec → item Done; second item: Reject spec → no implement file written | All UI states and files as specified; screenshots with token masked | `tests/web_board_e2e.rs` `tc_3714_*` |
| TC-3715 | FR-3715 | unit (asset) + e2e | Board assets; item with and without plan | Render | `Plan review` badge and section only with a plan | `src/web/assets.rs` `tc_3715_*` |
| TC-3716 | FR-3716 | unit | App with fake runner and gate on | Reach review, press `a`, `c`+feedback, `x`, Esc | Record decisions as specified; panel hides on Esc | `src/app.rs` `tc_3716_*` |
| TC-3717 | FR-3717 | integration | Fake runner binary, git repo | `dmuxai run --agent <fake> --approve-plan --json --progress` (gate on), approve via record from the test | JSON has `spec_review` approved round 1; stderr hint printed once | `tests/spec_first_headless.rs` `tc_3717_*` |
| TC-3718 | NFR-3700 | unit | Paused run for 3 s | Measure polls and CPU time | ≤ 7 polls; CPU < 1 % of wall time; no extra turn | `src/graph/spec_review.rs` `tc_3718_*` |

## 6. Decisions (Dongjin 2026-11-28)

- Spec review and plan review need a human check and must appear on the board (t793u).
- Implementation runner: Cursor Sonnet 5.5 via dmuxai (t793u).

## 7. Amendments

- 2026-11-28 (#374): the plan shown under `Plan review` (FR-3715) is the plan after spec-first expansion (SPEC-spec-first-workflow FR-3503); `board plan --edit`, dashboard Save plan and `board add --plan-hash` store that expanded plan, so the approved hash and the shown steps equal what the board child runs. Inserted steps carry the note `added by spec-first`. Tests: `src/board/plan.rs`, `src/web/board_api.rs`, `src/app.rs` and `tests/web_board_e2e.rs` `tc_3715_*`.
