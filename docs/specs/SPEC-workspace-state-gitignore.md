# SPEC: workspace-state-gitignore

Status: complete
Owner: DmuxAI
Created: 2026-11-07
Baseline: `b1ff818`
Related: GitHub issue #189

## 1. Decision summary

DmuxAI stores generated workspace state under `<workspace>/.dmuxai`: the live
dashboard discovery file and token, the event log, graph runs, task exports,
skill caches and records, the notify log, and image attachments. In a user
project that directory is not necessarily ignored by Git, so a normal
`git add -A` can stage a live token and generated state. DmuxAI will create a
self-ignoring `<workspace>/.dmuxai/.gitignore` containing `*` whenever it
creates the state directory, without editing the project root `.gitignore`.
Existing `.dmuxai/.gitignore` content and unrelated user files are preserved.

## 2. Scope

### In scope

- A shared workspace-state creation helper that creates `.dmuxai` and a
  `.gitignore` containing `*` when the ignore file is absent.
- Applying that helper to every DmuxAI production path that creates `.dmuxai`
  or a subdirectory below it.
- Idempotent behavior when `.dmuxai` or `.dmuxai/.gitignore` already exists.
- README documentation of the self-ignoring workspace state directory.
- Deterministic regression tests for first creation and existing state.

### Out of scope / non-goals

- Editing, creating, or parsing the project root `.gitignore`.
- Running Git commands, changing the index, or untracking already committed
  files.
- Moving the state directory, changing its permissions, or changing any file
  format other than the new `.gitignore`.
- Changing provider protocols, conversation schema, or workspace isolation.
- Cleaning up existing user workspaces or deleting existing state.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | An existing `.dmuxai/.gitignore` belongs to the user and must never be rewritten or appended to. | DmuxAI | Spec approval |

No implementation starts while a Must requirement depends on an unanswered
question.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Workspace state directory | The `<workspace>/.dmuxai` directory owned by DmuxAI for generated state. | The user's project source or root `.gitignore`. |
| Self-ignoring file | `<workspace>/.dmuxai/.gitignore` whose content is `*` (one pattern line), so Git ignores everything inside that directory. | A `.gitignore` in the workspace root. |
| Creation path | Any production code path that creates `.dmuxai` directly or creates a file/directory below it. | Read-only paths that only inspect an existing directory. |
| Preserve existing content | Leave an existing file byte-for-byte unchanged. | Append a missing pattern to a user file. |

State transitions for the ignore file:

| Current state | Trigger | Result |
| --- | --- | --- |
| `.dmuxai` absent | Any creation path | `.dmuxai` exists and `.dmuxai/.gitignore` contains `*` |
| `.dmuxai` present, `.gitignore` absent | Any creation path | Missing `.gitignore` is created with `*`; other content untouched |
| `.dmuxai/.gitignore` present | Any creation path | File is not opened for writing, truncated, or appended to |
| Creation fails | Any creation path | The operation keeps its existing error behavior; no root `.gitignore` is created or modified |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2501 | Must | Given a workspace with no `.dmuxai` directory, when a DmuxAI creation path first needs the state directory, then `.dmuxai/.gitignore` exists and its content is the single pattern `*`; the workspace root `.gitignore` is not created or changed. | Directory/file creation errors keep the operation's existing error behavior. | TC-2501, TC-2502, TC-2503, TC-2504, TC-2505, TC-2506, TC-2507 |
| FR-2502 | Must | Given `.dmuxai` already exists without `.gitignore`, when a creation path runs, then `.dmuxai/.gitignore` is created with `*`. Given `.dmuxai/.gitignore` already exists, when any creation path runs again, then its bytes are unchanged and no other user file is removed. | Repeated creation is idempotent; an existing ignore file is never overwritten or appended to. | TC-2501, TC-2503 |
| FR-2503 | Must | Given the supported workspace-state creation paths (dashboard discovery, event log, notify log, image attachments, graph runs, task exports, skill cache/approvals/run records/native delivery), when each creates state, then `.dmuxai/.gitignore` is present. | Best-effort logging paths keep swallowing their own errors; no provider or process behavior changes. | TC-2502, TC-2503, TC-2504, TC-2505, TC-2506, TC-2507 |
| FR-2504 | Should | Given the README, when a user looks for where workspace state lives, then it documents that `.dmuxai/.gitignore` with `*` is created automatically and that the project root `.gitignore` is not edited. | Documentation-only; no runtime effect. | TC-2508 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2501 | FR-2501, FR-2502 | unit | Temporary workspace; first with no state directory, then with an existing custom `.gitignore`, then with `.dmuxai` but no ignore file | Call the shared creation helper repeatedly and inspect bytes | First call creates `*`; custom content survives; missing file is filled in; root `.gitignore` stays absent | `src/workspace.rs`: `tc_2501_state_dir_gitignore_is_created_idempotently` |
| TC-2502 | FR-2501, FR-2503 | unit | Temporary workspace | `web::discovery::write_discovery` | `.dmuxai/.gitignore` exists with `*` and the discovery file still carries the token | `src/web/discovery.rs`: `discovery_file_and_token_env` |
| TC-2503 | FR-2501, FR-2502, FR-2503 | unit | Temporary workspace and source image | `ImageAttachmentStore::ingest_path`, `clipboard_image::save_attachment`, and `web::attachments_api::save_web_attachment` | Each first creation leaves `.dmuxai/.gitignore` present and attachments intact | `src/attachments.rs`, `src/clipboard_image.rs`, `src/web/attachments_api.rs` attachment tests |
| TC-2504 | FR-2501, FR-2503 | unit | Temporary workspace | `EventLog::append` | Event log is written and `.dmuxai/.gitignore` exists | `src/events.rs`: `read_since_skips_malformed_and_filters` |
| TC-2505 | FR-2501, FR-2503 | unit | Temporary workspace and graph run fixture | `graph::save_run` and `graph::write_tasks_file` | Graph record/tasks file are written and `.dmuxai/.gitignore` exists | `src/graph/store.rs`, `src/graph/tasks.rs` graph tests |
| TC-2506 | FR-2501, FR-2503 | unit | Temporary workspace and skill fixtures | Skill run record append and native skill delivery | Skill files are written under `.dmuxai` and `.dmuxai/.gitignore` exists | `src/stage_skills.rs` skill tests |
| TC-2507 | FR-2501, FR-2503 | unit | Temporary workspace | Best-effort notify log append | Log is written and `.dmuxai/.gitignore` exists | `src/notify.rs`: `tc_2507_notify_log_creates_state_gitignore` |
| TC-2508 | FR-2504 | unit | `README.md` | Read the workspace-state section | It states the automatic `*` ignore file and the untouched root `.gitignore` | `tests/readme_install.rs`: `tc_2508_readme_documents_workspace_state_gitignore` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Adds only `.dmuxai/.gitignore`; existing state files and formats are unchanged. | Delete the new ignore file to roll back; existing user content is never modified. | TC-2501 |
| Provider protocol | No change. | No rollback needed. | TC-2502..TC-2507 |
| Security / permissions | The new file has normal file permissions and contains only `*`; it must not expose tokens. It prevents accidental Git staging but is not an access-control boundary. | No permission changes; the dashboard token remains owner-only in `web.json`. | TC-2502 |
| Platform | Uses standard filesystem create semantics; no platform-specific branch. | Missing or unwritable state directory keeps the existing operation error. | TC-2501 |

## 7. Implementation plan

Detailed plan: [PLAN-workspace-state-gitignore](../plans/PLAN-workspace-state-gitignore.md).
Task checklist: [TASKS-workspace-state-gitignore](../tasks/TASKS-workspace-state-gitignore.md).

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add shared state-directory helper and unit tests | `src/workspace.rs` | FR-2501, FR-2502 | TC-2501 |
| 2 | Route discovery, events, notify, attachments, graph, and skills creation through the helper | `src/web/discovery.rs`, `src/events.rs`, `src/notify.rs`, `src/attachments.rs`, `src/clipboard_image.rs`, `src/web/attachments_api.rs`, `src/graph/store.rs`, `src/graph/tasks.rs`, `src/stage_skills.rs` | FR-2503 | TC-2502..TC-2507 |
| 3 | Document the behavior in README | `README.md` | FR-2504 | TC-2508 |
| 4 | Record review evidence | `docs/reviews/REVIEW-workspace-state-gitignore.md` | All | TC-2501..TC-2508 |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] External-provider smoke tests are marked run, skipped, or blocked with a
  reason; skipped is never reported as passing.
- [ ] `../reviews/REVIEW-workspace-state-gitignore.md` records commands, result, residual risk, and rollback
  notes.
