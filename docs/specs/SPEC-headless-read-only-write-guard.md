# SPEC: headless read-only workspace write guard

Status: complete
Owner: dmuxai
Created: 2026-11-07
Baseline: `2b610bf` (latest main); work started from the `b1ff818` WIP
Related: [#257](https://github.com/dmuxai/dmuxai/issues/257), [SPEC-agent-api.md](./SPEC-agent-api.md) (FR-1824)

## 1. Decision summary

Headless read-only runs (`--read-only`, legacy `ask`/`plan` mode, MCP child `--read-only`) can still succeed when a runner CLI writes files despite sandbox or permission settings. DmuxAI takes a worktree fingerprint before submitting the prompt and again after the run; when only DmuxAI's own `.dmuxai/` state changed, the run succeeds as today. Any other change makes the headless outcome a policy failure (`ok: false`, exit 1, `error_kind: policy`). Per review feedback on issue #257 the fingerprint must not compare only changed filenames/status: content edits to a file that was already dirty or untracked before the run must also be detected, so the fingerprint includes each changed path's staged blob hash and a worktree content summary that covers the entire file. Per the 2026-11-08 tail-edit follow-up the content summary must be position-independent across the whole file: a same-length edit past any early prefix (for example byte 1 MiB) of an already-dirty or already-untracked file must be detected, so the summary is a bounded-memory streaming hash of every byte, not a fixed prefix.

## 2. Scope

### In scope

- Worktree snapshot around `App::run_headless` when `TurnPolicy::ReadOnly`.
- Ignore `.dmuxai/` in the snapshot.
- Detect content edits to already-dirty tracked files and already-untracked files, not only added/removed/renamed paths.
- Worktree content summaries cover the entire file using bounded-memory streaming (no fixed prefix shortcut).
- JSON and CLI exit behaviour for stub runners that write anyway.

### Out of scope / non-goals

- Blocking writes inside each runner adapter (already done elsewhere).
- Non-git workspaces (no snapshot → no new check).
- Interactive TUI turns (headless only).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2540 | Repos without git keep the previous behaviour (no post-run workspace check). | dmuxai | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Fingerprint | Deterministic summary of `git status --porcelain=v2 -z --untracked-files=all --no-renames` for `.` excluding `:(exclude).dmuxai`, where each entry contributes its status letters, index modes, staged blob hash (`hI`), path, and a worktree content summary: the stored file length plus a hash of every byte of the file, streamed in bounded 64 KiB chunks (whole-file, no prefix shortcut). Raw-pathspec context: the repository root may sit above the workspace; paths resolve against it. | a bare `git status` filename/status listing, which cannot see a content edit to an already-dirty or untracked file |
| Read-only run | Headless run whose turn policy is `TurnPolicy::ReadOnly`. | write-capable runs (`TurnPolicy::Agent`) |
| Already-dirty file | A file already modified in the worktree/index or listed untracked at the before snapshot | a file that first appears in the porcelain listing after the run |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behaviour | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2540 | Must | Given a git worktree and a headless read-only run whose agent turn completes without an earlier failure | When the fingerprint after the run differs from the fingerprint taken just before the prompt is submitted — including when an already-dirty or untracked file got new content at any offset (for example a same-length single-byte edit past the first MiB) and its porcelain line stayed identical | Then `HeadlessOutcome` has `ok: false`, `exit_code` 1, `error_kind` `policy`, and `error` explains that the read-only run changed workspace files outside `.dmuxai/`. A clean worktree relative to the before snapshot keeps the run successful. | TC-2540, TC-2541, TC-2542, TC-2543 |
| FR-2541 | Must | Given legacy hidden `--read-only` on `dmuxai run` (maps to `ask` mode) | Then the same guard as FR-2540 applies | write-capable runs without `--read-only` are unchanged | TC-2541 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2540 | FR-2540 | unit | temp git repo | write only under `.dmuxai/`; then edit dirty tracked content twice; stage content; then create and re-edit an untracked file | `.dmuxai/` ignored; every content edit and staging changes the fingerprint | `src/workspace.rs` `tc_2540_git_worktree_fingerprint_excludes_dmuxai_and_sees_content_edits` |
| TC-2541 | FR-2540, FR-2541 | integration (CLI) | git sandbox + `fake-agent.sh readonly_write` | `dmuxai run hi --agent fake --read-only --json` | exit 1, `ok: false`, `error_kind: policy`, created file still present | `tests/agent_api_cli.rs` `tc_2541_headless_read_only_detects_workspace_write` |
| TC-2542 | FR-2540 | unit (app) | git temp workspace, recording adapter writes a file | `run_headless` with `mode: ask` | policy failure with `READ_ONLY_WORKSPACE_WRITE_ERROR` on the success path | `src/app.rs` `tc_2542_headless_read_only_workspace_write_is_policy_error` |
| TC-2543 | FR-2540 | unit | temp git repo with a committed file larger than 1 MiB | dirty the file once, then change only one byte past the first MiB keeping the length and the porcelain status line identical; repeat the same tail-only edit on an already-untracked file larger than 1 MiB | both tail-only edits change the fingerprint although the porcelain lines and file lengths are unchanged (the first-MiB-prefix design would have missed both) | `src/workspace.rs` `tc_2543_fingerprint_detects_tail_edit_beyond_first_mib` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema or stored-data change; snapshots stay in memory per run. | Revert the code and these artifacts together. | TC-2540 |
| Provider protocol | No request, event, or parsing change; the check runs around the turn, not inside it. | Removing the before/after snapshot restores prior behaviour. | TC-2542 |
| Security / permissions | Defense in depth on top of #248 clamp/sandbox settings; the existing per-runner read-only argv/config handling is unchanged and not weakened. | Agent errors and cancellation outcomes keep precedence over the guard. | TC-2541 |
| Platform | Sampling shells out to `git` with a bounded (5 s) wait; non-UTF-8 paths are lossy-converted, worktree files are hashed byte-oriented over their entire length in bounded 64 KiB chunks. Losing git, a repo, or the 5 s wait still returns a fingerprint of `None` and keeps prior behaviour. | Timeout or missing git ⇒ no guard, recorded as residual risk; full-file hashing increases the chance very large files push the sample past the 5 s cap beyond what a 1 MiB prefix would have. | TC-2540, TC-2543 |

## 7. Implementation plan

See `../plans/PLAN-headless-read-only-write-guard.md` and `../tasks/TASKS-headless-read-only-write-guard.md`.

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add `git_worktree_fingerprint` / `read_only_workspace_write_detected` with content-aware entries and a whole-file streaming content summary in `src/workspace.rs`. | `src/workspace.rs` | FR-2540 | TC-2540, TC-2543 |
| 2 | Sample before submit and compare after the headless loop in `App::run_headless`. | `src/app.rs` | FR-2540 | TC-2542 |
| 3 | Extend `tests/fixtures/fake-agent.sh` with a write stub; add the CLI integration test. | `tests/fixtures/fake-agent.sh`, `tests/agent_api_cli.rs` | FR-2540, FR-2541 | TC-2541 |

## 8. Review and test exit criteria

- [x] FR-2540 maps to passing TC-2540/TC-2541/TC-2542/TC-2543 evidence.
- [x] The diff is reviewed against scope and non-goals (`../reviews/REVIEW-headless-read-only-write-guard.md`), including the 2026-11-08 tail-edit follow-up covering the whole file with bounded memory.
- [x] `cargo fmt --check` and the narrow tests pass (`cargo test --lib tc_2540 tc_2543 tc_2542`, `cargo test --test agent_api_cli tc_2541`).
- [x] `python3 scripts/check-spec.py --all` and `python3 scripts/check-tc-links.py` pass.
