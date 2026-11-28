# PLAN: Chat beside the Markdown viewer, with edits in the work session

Spec: [SPEC-md-viewer-chat](../specs/SPEC-md-viewer-chat.md)
(FR-3840..FR-3846, NFR-3840). Builds on
[SPEC-board-ask-resume](../specs/SPEC-board-ask-resume.md) (hybrid resume,
`run --conversation`),
[SPEC-board-work-session](../specs/SPEC-board-work-session.md) (`sessions`,
changed `.md` paths) and
[SPEC-board-approval-hardening](../specs/SPEC-board-approval-hardening.md)
(run lock, owner checks).

## Order

1. `src/docs/md_view.rs`
   - Split the FR-3415 path checks out of the size check.
   - `md_file_context`: display path plus content (≤ 64 KiB UTF-8, else path only).
2. `src/board/session.rs` and `src/board/mod.rs`
   - `SessionLink.mode` (optional, only `"edit"` is kept).
   - `change_snapshot` / `changed_paths`: git status plus a content fingerprint,
     so files that were already dirty are still detected.
3. `src/board/resume.rs`
   - `item_seed_summary`, the FR-3831 summary for items without a work session.
4. `src/board/runner.rs`
   - `AskExtra { file, edit }` and the `_with` variants of prepare/report. This
     keeps the existing `AskParams` literals unchanged.
   - File section in ask prompts and the `[file · P]` / `[edit · P]` tags.
   - `execute_edit_turn` builds the argv with `edit_turn_run_args` (no
     `--read-only`, `--no-spec-first`, hybrid resume flags). It snapshots
     changes before and after, then writes the ask link `mode: "edit"` and the
     planner message (changed files, exit 7 text).
   - Live run → conflict; `--revise` with file/edit → usage.
5. Surfaces
   - HTTP ask `file`/`mode` with `human_browser_rejection` for edit.
   - CLI `board ask --edit --file`, owner only through `confirm_owner_action_cli`.
   - MCP `board_ask` rejects `edit`/`mode`.
6. Dashboard
   - `board.js` `dmuxaiMdViewerItemContext` (item modal links in `--web` only).
   - `md-viewer.js` docked chat: Ask/Edit, resume selector, Send and
     Ctrl/Cmd+Enter, poll only while an answer is pending, reload after edit.
   - `board.css` layout (right on wide screens, below on narrow ones).
7. Tests from spec §5, then the SPEC-board-ux-042 FR-3420 amendment and
   `tc_3846_fr3420_amended`.
8. Docs: API.md, README, CHANGELOG, spec status, TASKS, REVIEW.

## Risks

- Edit turns write to the workspace. Mitigations:
  - the normal write policy and sandbox apply, with no approval or sensitive
    flags;
  - only the owner can start one (human browser headers or TTY CLI);
  - a turn is refused during a live board run.
- Changed-path detection relies on `git status`. Without a git repository it
  reports no changes, and the edit still runs.
- The fingerprint reads each dirty file up to 1 MiB. Large dirty trees make
  the before/after snapshot slower, but only for edit turns.
- The viewer picks the panel's thread messages by matching path substrings, so
  a message that only mentions the file name in another form is not shown.

## Rollback

Revert the change set. Persisted data stays compatible:

- `SessionLink.mode` is optional and skipped when absent, so older readers
  ignore it;
- no store layout, schema or provider protocol changes.
