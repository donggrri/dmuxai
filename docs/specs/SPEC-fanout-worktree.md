# SPEC-fanout-worktree

Status: In progress · Series: orchestrator gap plan 4/5 · Builds on SPEC-fanout-readonly and SPEC-fanout-judge

## 1. Decision summary
Read-only fan-out cannot compare real code changes. This adds `--write` to `dmuxai run --agents ...`: each leg gets its own detached git worktree under `.dmuxai/fanout/<run_id>/<agent>/`, runs with write access only inside that worktree, and leaves a patch. DmuxAI never merges, applies, or commits anything to the main workspace; the user applies the patch they pick. Worktrees are cleaned up automatically, and a `fanout clean` command removes leftovers. Without `--write` nothing changes.

## 2. Scope

In scope: `--write` and `--keep-worktrees` flags, per-leg worktree creation and cleanup, per-leg patch and diff stat, interruption cleanup, `dmuxai fanout clean`, sensitive gate, free-runner exclusion for company workspaces, disk guard.
Out of scope: automatic merge or apply, choosing a winner automatically, non-git workspaces, TUI and dashboard UI, write legs combined with `--judge` input of patches (the judge may only see the text answer and diff stat).

## 3. Terms and state model

- Leg worktree: `git worktree add --detach <path> HEAD` at `.dmuxai/fanout/<run_id>/<agent>/`. The leg's runner uses it as its working directory.
- Patch: `.dmuxai/fanout/<run_id>/<agent>.patch`, produced from the leg worktree including new files; kept after cleanup.
- States of a leg worktree: `created`, `running`, `patched`, `removed` or `kept`.

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-2930 | Must | `--write` requires `--agents` (exit 2 otherwise), a git workspace (exit 2 if not), and a clean-enough HEAD (a repository with no commits exits 2). Each leg runs with `TurnPolicy::Agent` and its working directory set to its own worktree. |
| FR-2931 | Must | The main workspace is never modified by a write fan-out: the worktree fingerprint of the main workspace outside `.dmuxai/` is identical before and after. A change fails the run with `error_kind` `policy`. |
| FR-2932 | Must | After a leg ends, its changes (modified, deleted, and new files) are written to a patch file and summarized as `diff_stat` (files, insertions, deletions) in the leg output; `patch_path` and `worktree_path` (or `null` after removal) are included. A leg with no changes has an empty patch and `changed: false`. |
| FR-2933 | Must | Worktrees are removed after patch creation unless `--keep-worktrees` is given. The cleanup also runs after a leg failure, a timeout, and on SIGINT or SIGTERM. Patch files are never deleted by cleanup. |
| FR-2934 | Must | `dmuxai fanout clean [<run_id>|--all]` removes leftover leg worktrees and their `git worktree` registrations for that run or all runs under `.dmuxai/fanout/`; it never removes patch files and never touches worktrees outside that directory. |
| FR-2935 | Must | The sensitive gate applies to the prompt as for any write run: a sensitive prompt exits 7 until `--approve-plan` is given. |
| FR-2936 | Must | Free runners are skipped with a warning when `[fanout] company_workspace = true`, as in SPEC-fanout-readonly. |
| FR-2937 | Must | Before creating worktrees, free disk space on the workspace filesystem must be at least `[fanout] min_free_gb` (default 5) plus the repository working tree size per leg; otherwise exit 2 with a clear message and no worktree is created. |
| FR-2938 | Should | Each leg's runner fallback may only use runners not used by another leg (as FR-2914), and a fallback leg keeps using the same worktree. |
| NFR-2930 | Must | Without `--write` no worktree code runs; benchmark within 10% of main (#301). |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-2930 | FR-2930 | `tc_2930_write_flag_preconditions_and_leg_cwd` |
| TC-2931 | FR-2931 | `tc_2931_main_workspace_unchanged_by_write_fanout` |
| TC-2932 | FR-2932 | `tc_2932_patch_and_diff_stat_cover_new_modified_deleted_files` |
| TC-2933 | FR-2933 | `tc_2933_worktrees_removed_after_success_failure_and_interrupt` |
| TC-2934 | FR-2934 | `tc_2934_fanout_clean_removes_only_fanout_worktrees` |
| TC-2935 | FR-2935 | `tc_2935_sensitive_prompt_requires_approve_plan` |
| TC-2936 | FR-2936 | `tc_2936_company_workspace_skips_free_runners_in_write_mode` |
| TC-2937 | FR-2937 | `tc_2937_disk_guard_blocks_before_worktree_creation` |
| TC-2938 | FR-2938 | `tc_2938_fallback_keeps_worktree_and_avoids_other_leg_runners` |
| TC-2939 | NFR-2930 | `tc_2939_no_write_flag_creates_no_worktree` |

## 6. Data and safety contracts
Persistence: only files under `.dmuxai/fanout/`, which is already git-ignored by DmuxAI. No auto-merge: no `git merge`, `git apply`, `git checkout`, `git commit`, or `git push` is ever run against the main workspace. Security: leg runners receive no access to the main workspace path in their working directory.

## 7. Exit criteria
All TCs pass, gate and web tests pass, benchmark within 10%, no leftover worktrees after the test suite.
