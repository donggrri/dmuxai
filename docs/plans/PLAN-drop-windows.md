# PLAN: Drop Windows support (revertable)

Status: In progress
Created: 2026-10-22

## Decision

Windows is not supported for now. DmuxAI targets Linux only; WSL counts as
Linux and keeps working. Windows may come back once Linux is fully stable, so
the whole removal lands in one squash commit that can be brought back with
`git revert <sha>`.

## Scope

- Specs first: FRs that require Windows behaviour are marked **Deferred
  (Windows dropped 2026-10-22)**; Windows-only specs get a Deferred status.
- Docs: Windows install/usage (README, CONTRIBUTING, AGENTS, RELEASE-PROCESS,
  TEST-PLAN, WORKFLOW) no longer describe Windows binaries or Windows clippy.
- Code: `#[cfg(windows)]` blocks, the Windows-only `dmuxai-launcher` binary and
  `launcher` module, `packaging/windows`, `install.ps1`/`install.cmd`,
  `scripts/package-windows.sh`, `scripts/check-spec.ps1`, Windows-only
  dependencies, Windows-only tests.
- Out of scope: `.github/workflows/*` (the token lacks the `workflow` scope);
  the needed workflow diff is saved outside the repo and applied separately.
- Historical docs (reviews, graph-runs, old plans) stay as history.
