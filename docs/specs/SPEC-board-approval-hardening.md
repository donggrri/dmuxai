# SPEC: Board approval and run lifecycle hardening

Status: implemented
Owner: Dongjin
Created: 2026-11-28
Baseline: `068b920`
Related: #354, #361, #362, SPEC-decision-board FR-3133 FR-3138, SPEC-board-approve-autorun FR-3232, SPEC-terminal-sessions

## 1. Decision summary

Approval on the decision board is human-only (OQ-3104). Three gaps let a
non-human caller get around that rule or corrupt the item's state: the internal
`board run --exec-prepared` step trusts its `--by` argument and does not check
the decision (#354); some child processes do not carry `DMUXAI_RUN_ID`, and an
agent can type into a terminal session that was opened without it (#362); a
finished run overwrites a newer lane or plan, and detached runs leave zombie
processes in long-lived servers (#361). This spec closes those gaps without
changing any owner-facing workflow.

## 2. Scope

### In scope

- One-time launch token for the detached board run child; decision checks in the
  prepared-run step; lane move restrictions into `in_progress`.
- `DMUXAI_RUN_ID` on every agent/notify child spawned during a run; refusal of
  run-originated input into terminal sessions opened outside a run.
- Run result recording that respects a newer lane or plan; plan edits refused
  while a run is in flight; reaping detached children.

### Out of scope / non-goals

- New approval paths or UI changes.
- The loopback peer check (#355) and other web access rules.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3800 | `--exec-prepared` stays an internal flag (hidden from help); only the board's own detached launcher may use it | Dongjin | Answered 2026-11-28 |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| launch token | A random one-time secret (≥128 bits, hex) written by `run_item_prepare` into the item's run lock file (0600) and handed to the detached child out of band (environment variable `DMUXAI_BOARD_LAUNCH_TOKEN` or stdin), never on the command line | The dashboard access token |
| run lock | `<state base>/board/runs/<id>.lock` (existing), now holding `{pid, token_sha256, plan_hash, actor, created_ms}` | The board store lock |
| run-originated caller | A process whose environment has `DMUXAI_RUN_ID` set | The owner at a TTY |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3800 | Must | Given `run_item_prepare` (CLI `board run`, MCP `board_run`, HTTP `/run`, auto-run on approve) | Then it records the verified actor, the plan hash and the SHA-256 of a fresh launch token in the run lock (file 0600, dir 0700) and passes the token to the detached child only through its environment or stdin; the token never appears in argv, logs, thread messages, events or JSON output | If the lock cannot be written, nothing is launched and the call fails (exit 6 / HTTP 500) | TC-3800 |
| FR-3801 | Must | Given `board run <id> --exec-prepared --plan-hash H` | Then it runs only when the presented launch token hashes to the value in the run lock for that item and plan hash; the actor recorded for the run is the one stored in the lock, and any `--by` value is ignored for authorization | Missing, wrong or reused token → exit 5 (forbidden), item unchanged, no agent started; the token is consumed (cleared from the lock) on first successful use | TC-3801 |
| FR-3802 | Must | Given a prepared run is about to execute | Then it additionally requires either lane `in_progress` with a decision `approve` by `owner` whose `plan_hash` equals H, or an item that was in lane `auto` when prepared | Otherwise exit 5 / conflict 4 as today, item unchanged | TC-3802 |
| FR-3803 | Must | Given `board move <id> in_progress` (CLI, MCP, HTTP) | Then only the board run path may move an item into `in_progress`; a direct move is refused for non-owner callers unless the item is already approved for its current plan hash or in lane `auto` | Refusal is exit 5 / HTTP 403 with a hint to use `board run` | TC-3803 |
| FR-3804 | Must | Given a run started by dmuxai (`DMUXAI_RUN_ID` set in the parent) | Then every agent child it spawns, including custom agents (`[[agents.custom]]`), ACP agents, fan-out legs (`run --agents`) and notify command hooks, has `DMUXAI_RUN_ID` set in its environment | — | TC-3804 |
| FR-3805 | Must | Given a terminal session opened without `DMUXAI_RUN_ID` (by a human) | Then `term send`, `term send --keys` and `term exec` from a run-originated caller into that session exit 5 with `error_kind` `forbidden` and a message to ask a human; `term reopen` keeps the run id the session was opened with | Sessions opened inside a run keep working for run-originated callers | TC-3805 |
| FR-3806 | Must | Given a run finishes for an item | Then the item moves to `done`/`unknown` only if its lane is still `in_progress` and its plan hash still equals the hash that was run; otherwise the lane and plan are left as they are and a `run` thread message records the exit code and that the result was not applied | — | TC-3806 |
| FR-3807 | Must | Given an item in `in_progress` with a live run lock | Then `board plan` (attach or edit), `POST /plan` and `board ask --revise` are refused with conflict (exit 4 / HTTP 409) | After the run ends the operations work again | TC-3807 |
| FR-3808 | Must | Given a long-lived process (MCP server, dashboard) launches a detached board run | Then the child is reaped when it exits (no zombie remains), and the run lock stores the child pid so a stale-lock check sees a dead child as stale | — | TC-3808 |
| FR-3809 | Must | Given the existing owner flows (dashboard Approve with auto-run, TTY `board decide` then `board run`, auto lane run by a bot) | Then they behave exactly as before | — | TC-3809 |
| NFR-3800 | Must | Any `run`, board or term command | No measurable added latency (one extra small file write per board run); `run` without board involvement is unchanged | — | TC-3809 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3800 | FR-3800 | unit | Temp board, approved item | `run_item_prepare` | Lock is 0600 and holds token hash, plan hash, actor; launcher argv has no token | `src/board/runner.rs` `tc_3800_prepare_writes_token_hash_not_argv` |
| TC-3801 | FR-3801 | unit + cli | Approved item, prepared lock | exec-prepared with no token, wrong token, `--by owner` without token, valid token twice | Only the first valid call runs; others exit 5 and leave the item unchanged | `src/board/runner.rs` / `src/cli.rs` `tc_3801_exec_prepared_requires_launch_token` |
| TC-3802 | FR-3802 | unit | Items in `needs_decision`, `rejected`, approved with stale hash, `auto` | Force lane to `in_progress` in the store, then exec-prepared with a valid token | Only approved-matching and auto items run | `src/board/runner.rs` `tc_3802_exec_prepared_checks_decision` |
| TC-3803 | FR-3803 | unit + http | `needs_decision` item | Bot `move in_progress` via CLI/MCP/HTTP | exit 5 / 403; approved item path unaffected | `src/board/store.rs` `tc_3803_move_into_in_progress_restricted` |
| TC-3804 | FR-3804 | unit | Parent env with `DMUXAI_RUN_ID` | Build custom agent command, ACP spawn command, fan-out leg command, notify command | Each has `DMUXAI_RUN_ID` | `src/custom_agent.rs` etc. `tc_3804_run_id_on_all_children` |
| TC-3805 | FR-3805 | integration | Session opened without run id | `term send`/`exec` with `DMUXAI_RUN_ID` set; reopen | exit 5 forbidden; reopened session keeps its run id | `tests/term_cli.rs` `tc_3805_run_caller_cannot_type_into_human_session` |
| TC-3806 | FR-3806 | unit | Item rejected / re-planned while running | `record_run_result` exit 0 | Lane/plan unchanged; thread note added | `src/board/store.rs` `tc_3806_run_result_respects_newer_state` |
| TC-3807 | FR-3807 | unit + http | `in_progress` item with live lock | attach/edit plan, POST /plan, ask --revise | exit 4 / 409 | `src/board/store.rs` `tc_3807_plan_change_refused_while_running` |
| TC-3808 | FR-3808 | unit | Launcher in-process | Launch a short child, wait | No zombie (`/proc/<pid>/stat` state not `Z` or pid gone); lock has pid | `src/board/runner.rs` `tc_3808_detached_child_reaped` |
| TC-3809 | FR-3809, NFR-3800 | unit | Existing approve/auto-run fixtures | Run existing board tests (TC-3133, TC-3138, TC-3232..3235) | Pass unchanged in intent | existing tests |
