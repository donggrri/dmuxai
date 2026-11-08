# TASKS: headless read-only workspace write guard

Status: complete
Specs: [SPEC-headless-read-only-write-guard.md](../specs/SPEC-headless-read-only-write-guard.md) (FR-2540, FR-2541)

- [x] T1 — Content-aware workspace fingerprint excluding `.dmuxai/` (porcelain v2 entries
  with staged blob hash and worktree content summary; FR-2540).
  Verify: `cargo test --lib tc_2540`.
- [x] T2 — Headless before/after check on read-only runs (FR-2540).
  Verify: `cargo test --lib tc_2542`.
- [x] T3 — CLI `--read-only` + stub runner writes file (FR-2540, FR-2541).
  Verify: `cargo test --test agent_api_cli tc_2541`.
- [x] T4 — Docs and gates (FR-2540, FR-2541).
  Verify: `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py`.
- [x] T5 — 2026-11-08 tail-edit follow-up: replace the 1 MiB prefix content summary with
  a whole-file bounded-chunk streaming hash so a same-length edit past the first MiB of
  an already-dirty tracked file or an already-untracked file is detected (FR-2540).
  Verify: `cargo test --lib tc_2543`.
