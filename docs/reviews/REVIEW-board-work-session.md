# REVIEW: Board item work-session links

Spec: [SPEC-board-work-session](../specs/SPEC-board-work-session.md) ·
Plan: [PLAN-board-work-session](../plans/PLAN-board-work-session.md) ·
Tasks: [TASKS-board-work-session](../tasks/TASKS-board-work-session.md)

Scope: FR-3820..FR-3826, NFR-3820.

## Spec compliance

| Requirement | Verdict | Evidence |
| --- | --- | --- |
| FR-3820 | Met | `run_item_execute` appends a `run` link with status `running`, `workspace_path` and `started_ms` (actor = lock actor) before spawning the child; `sessions` is `#[serde(default)]`, capped at 50 (oldest dropped), and each change writes a `session` event. TC-3820 also loads an item JSON without `sessions`. |
| FR-3821 | Met (partly residual) | After the child, `finished_link` fills `run_id`, `conversation_id`, `graph_id`, `agent`, `model`, `exit_code`, `ended_ms` and status (manifest status first, else exit 0/7/130/other) and `md_paths` from two `git status` snapshots; the link is written before `record_run_result`, whose lane rules are unchanged. Crash and spawn error → `failed`. Non-git workspace → empty `md_paths`. TC-3821. Graph files do not record spec-gate paths, so only the git part of the union contributes today. |
| FR-3822 | Met | `append_ask_link` records an `ask` link (by `planner`) for answers and `--revise` (exit 7 → `awaiting_approval`, failure → `failed`). TC-3822. |
| FR-3823 / NFR-3820 | Met | `session_view` returns `work_session`, `sessions` and `bindings` (agent + status only) via `read_binding_statuses`, which opens the DB read-only and never creates or migrates it; missing DB/workspace → `warning`. Same function backs CLI `board session`, MCP `board_session` and `GET /api/board/{id}/session`. TC-3823 checks no `SECRET` in output, DB bytes unchanged, 10k-turn DB under 200 ms, unknown id → 3, invalid id → 2. List budgets: existing FR-3111 tests pass in `cargo test --lib board`. |
| FR-3824 | Met | `sessions` is serialized on every item, so `board show --json`, MCP `board_show` and `GET /api/board/{id}` include it; text links are masked and re-validated in `mask_item_fields`. TC-3824 (MCP), TC-3823 (HTTP GET item). |
| FR-3825 | Met | Board modal "Work session" section: newest first, status, agent/model, buttons for Run (`dmuxaiOpenRun`), Graph (`dmuxaiShowGraphRun`), Open in Chat (`--web` only; workspace switch or `load_conversation`), and `.md` links (`dmuxaiOpenMdViewer`); only `textContent`, no `href`. TC-3825 (asset unit test and two `#[ignore]` browser e2e tests, `--web` and `serve`). |
| FR-3826 | Met | `board link <id> --run-id R` and MCP `board_link` add a finished link from the run manifest; invalid run id → 2, unknown item or run → 3; re-linking the same run is a no-op. TC-3826 (CLI), TC-3824 (MCP). |
| Approval hardening | Kept | Run lock, launch token and `record_run_result` hash/lane rules untouched; TC-3800..TC-3808 pass in `cargo test --lib board`. |
| Non-goals | Respected | No provider session id exposed; conversation DB and run manifests are read only; `.github/` untouched; no use of `--allow-sensitive` or `--trust-workspace-config`. README has no board subcommand list, so it is unchanged. |

## Commands and results

- `cargo fmt` / `cargo fmt --check` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --lib board` — 81 passed, 0 failed
- `cargo test --lib` — 953 passed, 0 failed, 1 ignored
- `cargo test --lib -- tc_382 tc_3113` — 9 passed
- `cargo test --bin dmuxai -- tc_3826 tc_3114 tc_3102` — 3 passed
- `cargo test --bin dmuxai board` — 39 passed
- `cargo test --test web_board_e2e -- --ignored tc_3825` — 2 passed (headless Chrome)
- `bash scripts/check-spec.sh docs/specs/SPEC-board-work-session.md` — PASS
- `python3 scripts/check-spec.py --all` — 118/118 pass
- Not run: the full web e2e suite (out of scope), live providers (not needed).

## Residual risks

- Graph spec-gate paths are not stored in graph files, so `md_paths` comes only
  from the `git status` diff. A `.md` file that was already modified before the
  run and is modified again keeps the same status and is not listed.
- The Run and Graph buttons open the run or graph in the dashboard's current
  workspace; links from another workspace's item need a workspace switch first.
- Session writes are best effort: a store error while recording a link does not
  fail the run. An `ask` whose child fails to spawn records no link.
- Session writes bump the item version, so a concurrent `--if-version` update
  can get a conflict during a run start or finish.
