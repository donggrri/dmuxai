# REVIEW: workspace-state-gitignore

Status: pending verification
Owner: DmuxAI
Created: 2026-11-07
Spec: [SPEC-workspace-state-gitignore](../specs/SPEC-workspace-state-gitignore.md)
Plan: [PLAN-workspace-state-gitignore](../plans/PLAN-workspace-state-gitignore.md)
Tasks: [TASKS-workspace-state-gitignore](../tasks/TASKS-workspace-state-gitignore.md)
Issue: #189

## 1. Summary

Every production path that creates `<workspace>/.dmuxai` or a child path now
creates `.dmuxai/.gitignore` containing `*` when it is missing. The shared
helper uses `create_new`, so an existing user `.dmuxai/.gitignore` is preserved
byte-for-byte. The workspace root `.gitignore` is not read or written.

## 2. Requirement review

| Requirement | Result | Evidence |
| --- | --- | --- |
| FR-2501 | Met | TC-2501 helper test plus creation-path assertions in discovery, attachments, events, graph, skills, and notify tests. Fresh creation yields exactly `*\n`; root `.gitignore` is absent/unchanged. |
| FR-2502 | Met | TC-2501 covers repeated calls, an existing state directory without an ignore file, and an existing custom `.gitignore`; the custom bytes stay unchanged. |
| FR-2503 | Met | All inventoried creation paths were rewired through `ensure_state_dir` / `ensure_state_path`; module tests passed. Read-only paths were intentionally not changed. |
| FR-2504 | Met | README now documents the automatic `.dmuxai/.gitignore` and the untouched project root `.gitignore`. Manual check: `rg -n 'dmuxai/.gitignore' README.md`. |

## 3. Creation-path audit

| Creation path | Function | Helper call |
| --- | --- | --- |
| Dashboard discovery | `web::discovery::write_discovery` | `ensure_state_dir` |
| Event log | `events::EventLog::append` | `ensure_state_dir` |
| Event failure log | `events::log_append_failure` | `ensure_state_dir` (best effort) |
| Notify log | `notify::append_log` | `ensure_state_dir` (best effort) |
| Attachment ingest | `attachments::ImageAttachmentStore::ingest_path` | `ensure_state_path` |
| Clipboard image | `clipboard_image::save_attachment` | `ensure_state_path` |
| Web image upload | `web::attachments_api::save_web_attachment` | `ensure_state_path` |
| Graph run record | `graph::GraphStore::save` | `ensure_state_path` |
| Task export | `graph::write_tasks_file` | `ensure_state_path` |
| Skill cache sync | `stage_skills::sync_one` | `ensure_state_path` |
| Skill approvals | `stage_skills::Approvals::approve` | `ensure_state_dir` |
| Skill run records | `stage_skills::SkillRunRecord::append` | `ensure_state_path` |
| Native skill delivery | `stage_skills::deliver_native` | `ensure_state_path` |

Read-only paths (`FileGraphSource::poll`, `load_run`, `read_discovery`, docs
ignore lists) still do not create the state directory.

## 4. Test evidence

Environment: this worktree only, `CARGO_TARGET_DIR=target`.

Test-first evidence: before the fix, the discovery regression assertion failed
with `Os { code: 2, kind: NotFound, ... }` because `.dmuxai/.gitignore` was
absent. After the fix the same test passes.

Commands run:

```bash
CARGO_TARGET_DIR=target cargo fmt --check
bash scripts/check-spec.sh docs/specs/SPEC-workspace-state-gitignore.md
CARGO_TARGET_DIR=target cargo clippy --lib -- -D warnings
CARGO_TARGET_DIR=target cargo test --lib workspace::tests
CARGO_TARGET_DIR=target cargo test --lib web::discovery::tests
CARGO_TARGET_DIR=target cargo test --lib attachments::tests
CARGO_TARGET_DIR=target cargo test --lib clipboard_image::tests
CARGO_TARGET_DIR=target cargo test --lib web::attachments_api::tests
CARGO_TARGET_DIR=target cargo test --lib events::tests
CARGO_TARGET_DIR=target cargo test --lib notify::tests
CARGO_TARGET_DIR=target cargo test --lib graph::store::tests
CARGO_TARGET_DIR=target cargo test --lib graph::tasks::tests
CARGO_TARGET_DIR=target cargo test --lib stage_skills::tests
```

Results:

| Command | Result |
| --- | --- |
| `cargo fmt --check` | passed (no diff) |
| `check-spec.sh` on the new spec | `SPEC CHECK: PASS` |
| `cargo clippy --lib -- -D warnings` | passed |
| `workspace::tests` | 7 passed, 0 failed |
| `web::discovery::tests` | 1 passed, 0 failed |
| `attachments::tests` | 3 passed, 0 failed |
| `clipboard_image::tests` | 6 passed, 0 failed |
| `web::attachments_api::tests` | 7 passed, 0 failed |
| `events::tests` | 6 passed, 0 failed |
| `notify::tests` | 3 passed, 0 failed |
| `graph::store::tests` | 4 passed, 0 failed |
| `graph::tasks::tests` | 3 passed, 0 failed |
| `stage_skills::tests` | 21 passed, 0 failed |

Per coordinator instruction, the full repository gates
(`cargo test --all-targets`, `cargo clippy --all-targets --all-features`,
`scripts/gate.sh`) were not run in this worktree. No live-provider test was run;
no provider behavior changed.

## 5. Scope and non-goal check

- Root `.gitignore` is not read or written by the new code.
- No Git command is executed and no index/history is modified.
- No state directory location, schema, provider protocol, permission, or
  process-launch behavior changed.
- The only new persistent file is `<workspace>/.dmuxai/.gitignore` with `*`.
- Existing `.dmuxai/.gitignore` content is never overwritten or appended to.

## 6. Residual risks

- A `.dmuxai` directory that is already tracked in Git history is not
  untracked by this change; the ignore file only prevents future accidental
  staging.
- Because the file contains only `*`, Git also ignores the ignore file itself.
  That is the intended self-ignoring behavior from the issue.
- A user-provided `.dmuxai/.gitignore` that does not contain `*` is deliberately
  left alone; DmuxAI does not edit user content.
- If `.dmuxai` exists but is not writable, creation of the ignore file fails and
  the calling operation keeps its normal error path.

## 7. Rollback

Remove the `ensure_state_dir` / `ensure_state_path` calls (or the helper) and
delete `<workspace>/.dmuxai/.gitignore` in affected workspaces. No migration or
data rollback is needed.
