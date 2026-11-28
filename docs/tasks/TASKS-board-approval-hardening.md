# TASKS: Board approval and run lifecycle hardening

Spec: [SPEC-board-approval-hardening](../specs/SPEC-board-approval-hardening.md) ·
Plan: [PLAN-board-approval-hardening](../plans/PLAN-board-approval-hardening.md)

- [x] T1 — Run lock JSON with token hash, plan hash, actor, created_ms (0600/0700);
  token only in `DMUXAI_BOARD_LAUNCH_TOKEN`; added to `INTERNAL_ENV_VARS`.
  FR-3800. `cargo test --lib tc_3800`
- [x] T2 — `--exec-prepared` consumes the token under the store lock, actor from the
  lock, `--by` ignored; CLI reads and clears the env var first. FR-3801.
  `cargo test --lib tc_3801` · `cargo test --bin dmuxai tc_3801`
- [x] T3 — Prepared run re-checks owner approval for H or `auto` at prepare time.
  FR-3802. `cargo test --lib tc_3802`
- [x] T4 — Non-owner move into `in_progress` restricted in the store (CLI, MCP, HTTP
  share it). FR-3803. `cargo test --lib tc_3803` · `cargo test --bin dmuxai tc_3803`
- [x] T5 — `DMUXAI_RUN_ID` on custom agents, ACP, headless/fan-out legs, notify hooks.
  FR-3804. `cargo test --lib tc_3804` · `cargo test --bin dmuxai tc_3804`
- [x] T6 — Term host refuses run callers on sessions opened without a run id; reopen
  keeps it. FR-3805. `cargo test --test term_cli tc_3805`
- [x] T7 — `record_run_result` applies lane only if still `in_progress` on the run plan
  hash, else a `run` note. FR-3806. `cargo test --lib tc_3806`
- [x] T8 — Plan attach/edit (CLI, MCP, HTTP) refused while a live run lock exists.
  FR-3807. `cargo test --lib tc_3807`
- [x] T9 — Detached child reaped; lock stores child pid. FR-3808.
  `cargo test --lib tc_3808`
- [x] T10 — Existing flows unchanged; TC-3133/TC-3138 updated only for the token.
  FR-3809, NFR-3800. `cargo test --lib tc_31 tc_32` · `cargo test --bin dmuxai tc_31 tc_32`
- [x] T11 — Docs: SPEC-decision-board FR-3133/FR-3138 amendment notes, CHANGELOG,
  REVIEW; `bash scripts/check-spec.sh docs/specs/SPEC-board-approval-hardening.md`,
  `python3 scripts/check-spec.py --all`.
