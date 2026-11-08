# PLAN: workspace-state-gitignore

Status: complete
Owner: DmuxAI
Created: 2026-11-07
Spec: [SPEC-workspace-state-gitignore](../specs/SPEC-workspace-state-gitignore.md)
Issue: #189

## 1. Goal

Make every DmuxAI-created workspace state directory self-ignoring by creating
`<workspace>/.dmuxai/.gitignore` with `*`, without touching the project root
`.gitignore` and without overwriting existing user content.

## 2. Approach

1. Add one shared helper in `src/workspace.rs`:
   - `ensure_state_dir(dir)` creates the directory and a `.gitignore` with `*`
     only when the ignore file is missing.
   - `ensure_state_path(path)` finds the owning `.dmuxai` ancestor, applies the
     helper, then creates the requested path.
2. Replace direct production `create_dir_all` calls that create `.dmuxai` or a
   child path with the helper.
3. Add regression tests before the behavior change where practical, then keep
   them as evidence. Tests assert first creation, existing state, idempotency,
   and preservation of a custom `.gitignore`.
4. Document the behavior in README.

## 3. Creation-path inventory

| Path | Function | Change |
| --- | --- | --- |
| Dashboard discovery token | `web::discovery::write_discovery` | ensure `.dmuxai` |
| Event log + failure log | `events::EventLog::append`, `log_append_failure` | ensure `.dmuxai` |
| Notify log | `notify::append_log` | ensure `.dmuxai`, best-effort |
| Image attachments | `attachments::ImageAttachmentStore::ingest_path` | ensure `.dmuxai/attachments` |
| Clipboard images | `clipboard_image::save_attachment` | ensure `.dmuxai/attachments` |
| Web image uploads | `web::attachments_api::save_web_attachment` | ensure `.dmuxai/attachments` |
| Graph runs | `graph::GraphStore::save` | ensure `.dmuxai/graphs` |
| Task exports | `graph::write_tasks_file` | ensure `.dmuxai/tasks` |
| Skill cache | `stage_skills::sync_one` | ensure `.dmuxai/skills-cache/<...>` |
| Skill approvals | `stage_skills::Approvals::approve` | ensure `.dmuxai` |
| Skill run records | `stage_skills::SkillRunRecord::append` | ensure `.dmuxai/skill-runs` |
| Native skill delivery | `stage_skills::deliver_native` | ensure `.dmuxai/run-skills/<...>/<...>` |

Read-only paths (`FileGraphSource::poll`, `load_run`, `read_discovery`, docs
ignore lists) are intentionally unchanged and do not create the state
directory.

## 4. Dependency order

1. Tests that fail against the old behavior (creation paths and helper).
2. Shared helper and creation-path rewiring.
3. README.
4. Review evidence.

## 5. Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Adding a file changes directory-content assumptions | Tests count files in specific subdirectories, not `.dmuxai`; the ignore file lives at the state root. |
| Existing user `.gitignore` is clobbered | Use `create_new`; on `AlreadyExists`, leave the file untouched. |
| Best-effort logging now fails loudly | Keep ignoring helper errors in `notify` and event failure logging. |
| Creating `.dmuxai` on read paths | Only creation paths call the helper; read paths are unchanged. |
| Race between two processes creating the ignore file | `create_new` makes one win and the other treat `AlreadyExists` as success. |
| User workspace has a `.gitignore` directory or unusual file | Existing path is preserved; the operation keeps its normal behavior. |

## 6. Rollback

Delete `src/workspace.rs` helper usage and remove the new `.gitignore` creation.
The only persistent artifact is `<workspace>/.dmuxai/.gitignore`; deleting it
restores the old behavior. No schema, provider, or permission changes are
involved.
