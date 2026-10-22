# REVIEW: Drop Windows support (review-fix pass)

Status: Reviewed
Created: 2026-10-22
Plan: ../plans/PLAN-drop-windows.md
Tasks: ../tasks/TASKS-drop-windows.md

## Review source

This pass applies the findings from a free-model review pass (glm + deepseek)
of the original Windows-removal PR.

### Blockers

1. **CI Windows cross-target lint + release workflow still referenced Windows.**
   Needs the `.github/workflows/` change. It is applied **separately**, not in
   this pass, because the current token lacks the `workflow` scope; the workflow
   diff is saved outside the repo (per [PLAN-drop-windows.md](../plans/PLAN-drop-windows.md))
   and `.github/workflows/` is explicitly out of scope here. The binding change
   is documented in [CONTRIBUTING.md](../../CONTRIBUTING.md) as "being removed".
2. **BASELINE NFR-004 wording was wrong** (claimed Linux resolves Cursor's
   bundled Node entrypoint). Fixed in this pass: Linux runs `cursor-agent`
   from PATH (honouring `DMUXAI_CURSOR_BIN`), per `src/process/mod.rs:36`.

### Minor items (fixed in this pass)

- Deferred markers and deferred/stale test links in
  [SPEC-windows-desktop.md](../specs/SPEC-windows-desktop.md) and
  [SPEC-cursor-windows-implement-guard.md](../specs/SPEC-cursor-windows-implement-guard.md).
- Leftover `scripts/check-spec.ps1` references in `scripts/check-spec.py` and
  [PULL_REQUEST_TEMPLATE.md](../../.github/PULL_REQUEST_TEMPLATE.md).
- CONTRIBUTING Windows wording.
- Dead `#[cfg(not(unix))]` branches (`src/agent/custom.rs`,
  `src/web/discovery.rs`, `src/settings.rs`) removed and unix code made
  unconditional; `dunce_canonicalize` renamed to `canonicalize_path`.
- Historical note added to [ANALYSIS.md](../ANALYSIS.md).

## Gates

| Check | Command | Result |
| --- | --- | --- |
| Format | `cargo fmt --check` | Pass |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` (with `CARGO_TARGET_DIR`) | Pass |
| Tests | `cargo test --all-targets` (with `CARGO_TARGET_DIR`) | Pass |
| Spec check | `bash scripts/check-spec.sh <changed specs>` | Pass |
| Windows-target clippy | not run by design | N/A |

Recorded exact commands and results for this pass are in the PR description /
commit message; deterministic gates were run on Linux with
`CARGO_TARGET_DIR=a separate target dir`.

## Residual risk

- Workflow changes are deferred (see blocker 1); until applied, CI may still
  attempt the Windows cross-target lint or reference Windows runners.
- Historical documents (old reviews, plans, graph-runs, tasks) still mention
  PowerShell/Windows helpers; they are kept as history by plan scope.
- Original Windows-removal scope (code, packaging, spec deferral) landed in
  earlier commits on this branch and is not re-verified here.
