# REVIEW: Board approval and run lifecycle hardening

Spec: [SPEC-board-approval-hardening](../specs/SPEC-board-approval-hardening.md) ·
Plan: [PLAN-board-approval-hardening](../plans/PLAN-board-approval-hardening.md) ·
Tasks: [TASKS-board-approval-hardening](../tasks/TASKS-board-approval-hardening.md)

Scope: FR-3800..FR-3809, NFR-3800 (#354, #361, #362).

## Spec compliance

| Requirement | Verdict | Evidence |
| --- | --- | --- |
| FR-3800 | Met | `run_item_prepare` writes a JSON run lock (`atomic_write`, 0600; `ensure_dir`, 0700) with `pid`, `token_sha256`, `plan_hash`, `actor`, `created_ms`, `prepared_lane`, and fails before any launch if the write fails. The token is 64 hex chars from two `uuid` v4 values (CSPRNG-backed, 244 random bits) and travels only in `DMUXAI_BOARD_LAUNCH_TOKEN` (also in `INTERNAL_ENV_VARS`, so agent children never inherit it). Applies to CLI, MCP `board_run`, HTTP and auto-run because they all go through `run_item_prepare` + `launch_detached_board_run`. TC-3800 checks modes, hash, argv, env and that no file under the state dir contains the token. |
| FR-3801 | Met | `BoardLockGuard::take_over` verifies the token hash (constant-time compare) and plan hash under the store flock, then clears `token_sha256`. The actor comes from the lock; the CLI ignores `--by` and reads then removes the env var before any agent child starts. TC-3801 (unit and CLI): no token, empty, wrong token, wrong hash, `--by owner` without token, reuse → exit 5, item unchanged. |
| FR-3802 | Met | `ensure_prepared_run_allowed`: `prepared_lane == auto`, or decision `approve` by `owner` (else 5) with `plan_hash == H` (else 4). TC-3802 covers needs_decision, rejected, stale approval, approved, auto. |
| FR-3803 | Met (CLI, MCP, store) | `move_lane` → `enforce_in_progress_move` inside the store lock. TC-3803 covers store and MCP (`tc_3803_move_into_in_progress_restricted`) and CLI (`tc_3803_cli_move_into_in_progress_restricted`). HTTP `lane` always acts as `owner` (dashboard), so a non-owner HTTP move cannot be built; the shared store check is the HTTP path. |
| FR-3804 | Met | `process::apply_run_id_env` on custom agents (after `spec.env`, so config cannot clear it), ACP spawn (`acp_command`), headless children; notify hooks pass the parent's id; fan-out exports one id for all legs (`RunIdEnvGuard`). TC-3804: `tc_3804_run_id_on_all_children`, `tc_3804_run_id_on_acp_child`, `tc_3804_run_id_on_fanout_legs`. |
| FR-3805 | Met | `send`/`send --keys`/`exec` requests carry the caller's run id; the host refuses with `forbidden` ("ask a human to type it") when the session spec has no `DMUXAI_RUN_ID`. Reopen reuses the stored spec. TC-3805 (integration). |
| FR-3806 | Met | `record_run_result(.., plan_hash)` applies `done`/`unknown` only while `in_progress` on the run's plan hash; otherwise appends a `run` note with the exit code and "not applied", lane and plan untouched, event kind `message`. TC-3806. |
| FR-3807 | Met | `attach_plan`/`edit_plan` → `ensure_no_run_in_flight` (conflict); `ask --revise` was already refused. TC-3807 (store, `ask_item_prepare`) and `tc_3807_http_plan_refused_while_running` (HTTP 409, then 200 after the run ends). |
| FR-3808 | Met | `launch_detached_board_run` records the child pid in the lock (only while the token is unused) and reaps the child on a `board-run-reaper` thread. TC-3808 checks lock pid, `/proc/<pid>` gone, lock inactive. |
| FR-3809 / NFR-3800 | Met | Existing TC-3133/TC-3138/TC-3232..3235 pass; TC-3133 and TC-3138 changed only to present the launch token. Added cost per board run: one small lock rewrite and one SHA-256; `run` without the board only sets `DMUXAI_RUN_ID` explicitly (it was already set). The term overhead bounds (TC-2691, TC-2692) pass. |
| Non-goals | Respected | No new approval path or UI change; no web access rule changes (#355 untouched). `.github/` untouched; no use of `--allow-sensitive` or `--trust-workspace-config`. |

## Commands and results

- `cargo fmt` / `cargo fmt --check` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --lib` — 945 passed, 0 failed, 1 ignored
- `cargo test --bin dmuxai` — 732 passed, 0 failed, 6 ignored (on the first run,
  `agent::cursor::tests::lingering_cli_is_stopped_after_grace_and_queued_prompt_runs`
  failed once under machine load; it passed 3/3 alone and the suite passed on re-run)
- `cargo test --test term_cli` — 36 passed, 0 failed, 1 ignored (on the first
  run, `tc_2622_wait_match_prompt_across_writes_with_ansi_and_cr` failed once; it
  passed 3/3 alone and the suite passed on re-run)
- `bash scripts/check-spec.sh docs/specs/SPEC-board-approval-hardening.md` — PASS
- `bash scripts/check-spec.sh docs/specs/SPEC-decision-board.md` — PASS
- `python3 scripts/check-spec.py --all` — 117/117 pass, 0 allowlisted
- Not run: the web e2e suite (out of scope), live providers (not needed).

## Residual risk / untested paths

- FR-3800 "lock cannot be written → nothing launched": the code returns the
  error before `move_lane`, but there is no test; `ensure_dir` repairs
  directory modes, so the failure is hard to provoke without root tricks.
- A run prepared by an older binary (plain `pid=N` lock, no token) cannot be
  executed by the new `--exec-prepared` and stays `in_progress` until the lock
  goes stale. Only affects runs in flight during an upgrade.
- An older term host ignores the new `run_id` field and does not enforce FR-3805
  until it restarts (hosts exit after the idle grace).
- FR-3805 trusts the caller's environment: a process that unsets
  `DMUXAI_RUN_ID` is treated as a human, as in the existing `term approve` guard.
