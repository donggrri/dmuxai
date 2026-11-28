# REVIEW: coordinator observability

Spec: [SPEC-coordinator-observability.md](../specs/SPEC-coordinator-observability.md) · Plan: [PLAN-coordinator-observability.md](../plans/PLAN-coordinator-observability.md) · Tasks: [TASKS-coordinator-observability.md](../tasks/TASKS-coordinator-observability.md)

## Spec compliance

| Requirement | Evidence |
| --- | --- |
| FR-3850 `run_started` | `src/board/runner.rs` tc_3850 (event between the `in_progress` lane event and the result, `plan_hash`, notify payload) |
| FR-3851 `awaiting_approval` | `src/board/store.rs` tc_3851 (add with plan, re-entry with same revision, new revision, plan attached outside `needs_decision`, no plan) |
| FR-3852 spec review hook | `src/board/notify.rs` tc_3852 (thread message, event, notify payload, 20-path cap, outside/`..` paths dropped, unknown and invalid run ids leave the board unchanged, invalid id logged; `DMUXAI_BOARD_ITEM_ID` names an `in_progress` item → that item, `needs_decision`/invalid/empty → `run_ids` fallback); `src/board/runner.rs` tc_3852 (run child gets the item id, ask child and a stale parent value do not); `src/process/mod.rs` tc_3852 (agent children do not inherit it); `src/graph/spec_review.rs` tc_3852 (the gate's wait point calls the hook once per pending round with review id, spec paths and board item id, never on poll/heartbeat/decision; through the real hook one `spec_review_requested` event and one notification, an unknown item does not fail the run) |
| FR-3853 `notify_events` names | `src/settings.rs` tc_3853 |
| FR-3854 wait/events | `src/board/store.rs` tc_3854 (`any` wakes, `decision` does not, self-writes filtered, `board events` lists detail) |
| FR-3855 `runs list` / `runs_list` | `src/cli.rs` tc_3855, `src/mcp_server.rs` tc_3855 |
| FR-3856 `runs show` / `runs_show` | `src/cli.rs` tc_3856, `src/mcp_server.rs` tc_3856 |
| FR-3857 docs | `tests/spec_index.rs` tc_3857; README CLI block and MCP note; CHANGELOG |
| NFR-3850 one line, no thread | tc_3850 (exactly one `run_started` line; `BOARD_NOTIFY_THREAD_SPAWNS` unchanged without a target) |

Non-goals respected: the spec review gate itself comes from SPEC-spec-review-gate (#372); this change only wires the hook into its wait point. No new push channels.

## Interpretations

- **`wait_for any` and actors.** FR-3112 wakes waiters only on `owner`/`planner` writes. The new kinds are written by bots or the runner, so `any` wakes on them regardless of actor (FR-3854); a waiter still never wakes on its own writes. `decision`/`message` are unchanged.
- **Hook matching.** No item field holds a session link. The spec review gate pauses a run before the board child has returned its JSON, so the run id is not yet in `run_ids`. The board therefore sets `DMUXAI_BOARD_ITEM_ID` on the `board run` child (one place, `run_item_execute`, shared by `board run`, auto-run and `--exec-prepared`); it is in `process::INTERNAL_ENV_VARS`, so agent children and the ask/revise children never carry it. The hook uses that item while it is `in_progress`, else matches `run_ids`; an invalid run id is still rejected and logged before either lookup.
- **Rebase onto SPEC-spec-review-gate (#372) and board-approval-hardening (#379).** One pending-review notification: #372 dispatched notify event `spec_review` from the board mirror (`board::runner::sync_spec_review`); that dispatch is removed and `SpecReviewGate::after_node_finished` (the point where the run enters `awaiting_spec_review`) calls `board::notify_spec_review_requested_in` with the record's review id, spec paths and board item id (record `board_item_id`, else `DMUXAI_BOARD_ITEM_ID`, else `run_ids`). It fires once per entry into the pause, so a Request changes round that pauses again announces again; polls, heartbeats and decisions never do. `spec_review` stays a valid `notify_events` name and selects `spec_review_requested` (FR-3711 row updated). The board item keeps both thread messages (#372's `planner` "Spec review requested (round N)" and FR-3852's `spec_review` one). The board base is derived from the review store's state dir, like the mirror, so tests never touch the user's board. `DMUXAI_BOARD_LAUNCH_TOKEN` (#379) and `DMUXAI_BOARD_ITEM_ID` are both in `INTERNAL_ENV_VARS`; `BOARD_ITEM_ID_ENV` has one definition (`board`), re-exported from `board::runner`. tc_3851 re-enters `needs_decision` through `unknown` because #379 forbids a direct move into `in_progress`.
- **`awaiting_approval` and `decide`.** `request_changes` keeps the plan revision, so no second event; a revised plan (`board plan` / `board ask --revise`) emits it.
- **`runs list --json` shape.** Prints `{"runs": [...]}` like `GET /api/runs` and MCP (TC-3855 "empty dir → []" is checked on the `runs` array). The legacy `runs [--limit] [--failed] [--json]` array output is unchanged.
- **`runs show --json` shape.** The `GET /api/runs/{id}` object plus the manifest fields at top level, so FR-2562 callers that read `run_id`, `schema_version`, `links` keep working; manifest strings are now masked as on the dashboard.
- **Errors.** `runs show --json` errors print the standard `{ok:false, exit_code, error}` object on stdout (other subcommands do the same); exit codes 2/3/6 are unchanged from FR-2562.

## Regression risk

- `GET /api/runs` and `/api/runs/{id}` now call `src/runs_view.rs` (moved code, same limits); `tests/serve_runs_api.rs` and `tests/web_all_projects.rs` pass.
- `events.jsonl` readers now take the last `seq` from the `seq` field only; older binaries skip unknown kinds when computing it (downgrade note in the plan).
- Settings are loaded once per write that emits `needs_decision` and/or `awaiting_approval`.

## Test evidence

Deterministic, run on this branch:

- `cargo fmt --check` — pass.
- `cargo clippy --all-targets --all-features -- -D warnings` — pass.
- `cargo test --lib -- board:: settings mcp_server runs_view web::runs_api web::global_api` — 156 passed.
- `cargo test --bin dmuxai` — 730 passed, 6 ignored.
- `cargo test --test run_journal_cli` (7), `serve_runs_api` (5), `spec_index` (17), `mcp_client` (5, 1 ignored), `headless_runs_journal` (8), `run_journal_store` (8), `web_all_projects` (4, 2 ignored) — pass.
- `bash scripts/check-spec.sh docs/specs/SPEC-coordinator-observability.md`, `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` — pass.
- Raw stdio session against `dmuxai mcp`: `tools/list` includes `runs_list`/`runs_show`; `runs_list` → `{ok:true, runs:[]}`; unknown `runs_show` → `isError`, `exit_code: 3`. CLI: `runs list --status bogus` exit 2, `runs show <unknown> --json` exit 3.

Follow-up (`DMUXAI_BOARD_ITEM_ID`): `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --lib board` (72 passed), `cargo test --test spec_index` (17 passed), `cargo test --lib -- process:: term::backend` (31 passed with `DMUXAI_RUN_ID` unset; `headless_command_applies_child_env_policy` fails when the shell already exports `DMUXAI_RUN_ID`, unrelated), `cargo test --bin dmuxai agy` (50 passed), `bash scripts/check-spec.sh docs/specs/SPEC-coordinator-observability.md`, `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` — pass.

Overhead (#301), release builds, `origin/main` vs this branch, same machine:

- `board add` into `needs_decision` (no notify target), 120 runs interleaved: median 3.89 ms vs 3.97 ms (+2%).
- `board events --since 0 --json` over 100 000 lines: median 432 ms vs 245 ms (faster).

Rebase onto #372/#373/#379/#380: `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --lib board` (93 passed), `cargo test --lib graph` (108 passed), `cargo test --lib web` (257 passed, 1 ignored), `cargo test --lib -- process:: settings` (141 passed), `cargo test --lib -- tc_3855 tc_3856` (2), `cargo test --bin dmuxai -- tc_3855 tc_3856` (2), `cargo test --test spec_index` (17), `run_journal_cli` (7), `serve_runs_api` (5), `spec_first_headless` (5), `bash scripts/check-spec.sh` on SPEC-coordinator-observability and SPEC-spec-review-gate, `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` — pass.

Not run: the web e2e suites (`--ignored` browser tests), as instructed; no live provider tests (none in scope).

## Residual risks

- A run without `DMUXAI_RUN_ID` and without a board item id (plain TUI) skips the hook, since no item could match.
- Subscribers that relied on the `spec_review` notify payload name now receive `event: "spec_review_requested"` (with `review_id`/`spec_paths`).
- Bots that use `board wait --any` will now also wake on runner/bot `run_started` and `awaiting_approval` events and must tolerate them.
