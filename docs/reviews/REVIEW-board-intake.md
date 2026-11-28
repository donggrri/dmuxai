# REVIEW: Board intake from a prompt (planner auto plan)

Spec: [SPEC-board-intake](../specs/SPEC-board-intake.md) ·
Plan: [PLAN-board-intake](../plans/PLAN-board-intake.md) ·
Tasks: [TASKS-board-intake](../tasks/TASKS-board-intake.md)

Scope: FR-3860..FR-3866, NFR-3860 (fixes #375). SPEC-board-approval-hardening
(launch token, `DMUXAI_RUN_ID` on children) is kept and reused.

## Spec compliance

| Requirement | Verdict | Evidence |
| --- | --- | --- |
| FR-3860 | Met | `headless_save_pending_plan` (src/app.rs) saves the latest revision without errors **or** the fallback revision (#375 root cause: the fallback revision carries the planner failure in `errors` and was skipped). The pending file is written from the revision produced after `apply_spec_first_invariants`, the same as planner plans. `HeadlessOutcome` adds `plan_source` (`planner`/`fallback`) and `planner_error` (masked with `mask_sensitive_tokens`, ≤ 500 chars). TC-3860 checks exit 7, hash, source, masked error, pending file, and that `--approve-plan-id` runs it; it fails with the old revision filter. |
| NFR-3860 | Met | No extra agent turn: only the revision choice and one masking pass at the gate. TC-3860 runs without a planner turn beyond the failed ones. |
| FR-3861 | Met | `intake_add` checks plan hash, empty prompt and workspace before creating; rules/FR-3105 through `add_reporting_created`; `planning` → child `-C <ws> run --graph --plan-only --json -` (prompt on stdin, 900 s) → `attach_plan` (FR-3106 validation, FR-3130 copy) → `planned` plus a `planner` note with the step count; other exits → `failed` with a masked error. CLI blocking / `--detach`, MCP and HTTP (detached). TC-3861 (runner: blocking, detached + token-checked `board intake`, failure, invalid input), `tc_3861_cli_board_add_prompt_flags`, `tc_3861_http_board_create_and_prompt_refused_in_serve`. |
| FR-3862 | Met | `planning.warning` = `planner unavailable — fallback plan` on fallback; `board show` (JSON field, stderr line in text), `board pending` (`warning`), modal and cards (`board.js`). Owner approval unchanged. TC-3862, TC-3865. |
| FR-3863 | Met | A key match returns the existing item before any planning state, lock or child. TC-3863 (no child, no lock, no new event). |
| FR-3864 | Met | `intake_run_args` has no approve/trust flags; `spawn_child_dmuxai` strips `DMUXAI_WEB_TOKEN` and the launch token, sets `DMUXAI_RUN_ID`. One planning run per item via `runs/<id>.plan.lock` (conflict, exit 4 / HTTP 409). TC-3864. |
| FR-3865 | Met | New task button + form (title, kind, workspace prefilled from the current workspace, prompt) → `POST /api/board`; hidden unless `launch_allowed`; `Planning…` on the card until the SSE refresh. TC-3865 asset test and two browser tests (`--web` flow with fallback warning; hidden + 403 in `serve`). |
| FR-3866 | Met | `HeadlessRequest.plan_only` starts planning directly (no judge, no trivial shortcut, no slash path), never auto-approves, exits 7 with `plan_hash`; CLI/MCP conflicts exit 2. TC-3866 (`hi` and a complex prompt with a judge that answers `simple`: exit 7, nothing executed, no workspace changes; fallback case; five conflict cases). |
| Non-goals | Respected | No spec-first steps for board-edited plans (#374 untouched); no change to approval rules; `.github/` untouched; no `--allow-sensitive` / `--trust-workspace-config`. |

Additions beyond the spec text (observable but compatible): `planning.warning`
field (read by show/pending/dashboard), the hidden `board intake <id>`
subcommand used by the detached child, and `POST /api/board` without `prompt`
also works in `serve` (normal create, as FR-3120 allows dashboard writes).
Blocking `board add --prompt` exits 0 when planning fails; the failure is in
`planning.state = failed` (like `board run` recording `unknown`).

## Commands and results

- `cargo fmt` / `cargo fmt --check` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --lib board` — 79 passed, 0 failed
- `cargo test --lib` — 951 passed, 0 failed, 1 ignored
- `cargo test --bin dmuxai` — 733 passed, 0 failed, 6 ignored (first run failed
  `tc_1771_help_lists_progress_not_read_only` until `--plan-only` was added to
  the `run` line of the top-level usage; fixed)
- `cargo test --test headless_fallback_plan` — 2 passed
- `cargo test --test headless_runs_journal` — 8 passed
- `cargo test --test spec_index` — 16 passed
- `cargo test --test web_board_e2e -- --ignored tc_3865` — 2 passed (headless Chrome)
- `bash scripts/check-spec.sh docs/specs/SPEC-board-intake.md` — PASS
- `python3 scripts/check-spec.py --all` — 118/118 pass, 0 allowlisted
- Not run: the rest of the web suite (out of scope), live providers (not needed).

## Residual risk / untested paths

- A detached intake child that dies before recording a result leaves the item
  in `planning`; the lock goes stale so a new intake can run, but the state is
  not reset automatically.
- `planning.warning` stays on the item if the owner later replaces the fallback
  plan through `board plan --edit`; it is only shown while a plan is attached.
- #301 overhead was not benchmarked; the `run --graph` hot path only gains a
  revision filter and string masking at the approval gate.
- MCP `board_add` with `prompt` shares `intake_add` with CLI/HTTP; it has no
  dedicated test beyond the schema check.
