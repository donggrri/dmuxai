# PLAN: Board approval and run lifecycle hardening

Spec: [SPEC-board-approval-hardening](../specs/SPEC-board-approval-hardening.md)
(FR-3800..FR-3809, NFR-3800; issues #354, #361, #362).

## Order

1. `src/board/runner.rs` — run lock becomes JSON `{pid, token_sha256, plan_hash,
   actor, created_ms, prepared_lane}` (0600 in a 0700 dir); `run_item_prepare`
   generates a launch token (two `uuid` v4 values, CSPRNG-backed, 244 random
   bits, no new crate) and stores only its SHA-256 (`sha2`). The detached
   launcher passes the token in `DMUXAI_BOARD_LAUNCH_TOKEN`, records the child
   pid in the lock and reaps the child on a background thread (FR-3800,
   FR-3808).
2. `run_item_execute_prepared` takes the token, consumes it under the store
   lock, takes the actor from the lock and re-checks the owner approval for the
   plan hash or the `auto` lane at prepare time (FR-3801, FR-3802). The CLI reads
   and removes the env var before any agent child starts; `--by` is ignored.
   The env var is in `process::INTERNAL_ENV_VARS`, so agent children never see it.
3. `src/board/store.rs` — `move_lane` refuses non-owner moves into
   `in_progress` unless approved for the current plan or `auto` (FR-3803);
   `record_run_result` applies the lane only when still `in_progress` on the run
   plan hash (FR-3806); `attach_plan`/`edit_plan` refuse while a live run lock
   exists (FR-3807; `ask --revise` already refused).
4. `src/process/mod.rs` — `current_run_id`/`apply_run_id_env`; used by custom
   agents, ACP spawn, headless children and notify hooks; fan-out exports one run
   id for all legs (FR-3804).
5. `src/term/{proto,policy,host}.rs` and `src/cli.rs` — `send`/`send --keys`/`exec`
   carry the caller's run id (optional, serde default); the host refuses run
   callers on sessions opened without one (FR-3805). Reopen reuses the spec, so
   the run id is kept.
6. Tests named by spec section 5; existing TC-3133/TC-3138 updated for the token.
7. Docs: one-line amendments in SPEC-decision-board FR-3133/FR-3138, CHANGELOG,
   TASKS, REVIEW.

## Risks

- Race between the parent recording the child pid and the child consuming the
  token: both run under the store flock, and the parent only rewrites the lock
  while the token is still unused.
- Older lock files (`pid=N` text) still parse for the stale check; a prepared run
  from an older binary cannot be executed by the new one (needs a token) and will
  be refused, leaving the item `in_progress` until the lock goes stale. Accepted:
  only affects runs in flight during an upgrade.
- Term protocol change: the new `run_id` field is optional and skipped when
  absent, so an older host or client keeps working (an older host simply does
  not enforce FR-3805).

## Rollback

Revert the change set. Run locks are short-lived per-run files; a reverted
binary cannot read the pid from a JSON lock and treats it as live until its
age passes the stale-lock limit, so remove `<state base>/board/runs/*.lock` for
items that are not running after a rollback. No persistent item or event format
changes.
