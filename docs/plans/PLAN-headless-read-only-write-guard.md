# PLAN: headless read-only workspace write guard

Status: complete
Issue: [#257](https://github.com/dmuxai/dmuxai/issues/257)
Spec: [SPEC-headless-read-only-write-guard.md](../specs/SPEC-headless-read-only-write-guard.md)

1. Add content-aware `git_worktree_fingerprint` / `read_only_workspace_write_detected` in
   `src/workspace.rs`: porcelain v2 status excluding `.dmuxai/`, plus per-entry staged
   blob hash and a worktree content summary, so content edits to already-dirty or
   untracked files are detected even when filenames and status lines do not change
   (FR-2540, TC-2540).
2. Sample before submit and compare after the headless loop in `App::run_headless`
   (FR-2540, TC-2542).
3. Extend `tests/fixtures/fake-agent.sh` with a write stub; add CLI integration test
   (FR-2540, FR-2541, TC-2541).
4. Spec, tasks, review; run `check-spec.py --all` and `check-tc-links.py`.
5. 2026-11-08 tail-edit follow-up: make the content summary whole-file — stream every
   byte through the hash in bounded 64 KiB chunks instead of hashing a fixed 1 MiB
   prefix — because a same-length edit past the first MiB of an already-dirty tracked
   file or an already-untracked file is still an issue-#257 write that prefix hashing
   misses (FR-2540, TC-2543). No new requirement ID: this was inside #257's requested
   write detection, not an accepted residual.

Risks:

- Extra worktree reads during fingerprinting ride on the bounded 5 s wait; huge or
  ignored-on-filesystem untracked trees add latency but cannot corrupt data.
- Whole-file hashing raises per-file I/O; very large worktrees can push the sample
  past the 5 s cap and disable the guard for that run (fail open, honestly tracked
  in the review).
- Porcelain pathspec exclusion assumes the repo root may sit above the workspace; paths
  resolve against `rev-parse --show-toplevel`.

Rollback: revert steps 1–2 (and 5) only; behaviour returns to pre-#257.
