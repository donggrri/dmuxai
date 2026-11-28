# REVIEW: Chat beside the Markdown viewer, with edits in the work session

Spec: [SPEC-md-viewer-chat](../specs/SPEC-md-viewer-chat.md) ·
Plan: [PLAN-md-viewer-chat](../plans/PLAN-md-viewer-chat.md) ·
Tasks: [TASKS-md-viewer-chat](../tasks/TASKS-md-viewer-chat.md)

Scope: FR-3840..FR-3846, NFR-3840.

## Spec compliance

| Requirement | Verdict | Evidence |
| --- | --- | --- |
| FR-3840 | Met | `board.js` `dmuxaiMdViewerItemContext` returns a context only in `--web` (`boardLaunchAllowed`) for a link inside the open item modal (`#board-drawer`: summary, thread and Work session links). `md-viewer.js` then docks `#md-viewer-chat` with the item's thread messages that mention the path, a textarea, Ask/Edit (Ask pressed), the FR-3836 resume selector (shown only with a work session) and Send (Ctrl/Cmd+Enter). It sits right on wide screens and below at ≤ 900 px. There is no panel in `serve` or for a viewer opened outside an item. TC-3840: the asset test, plus e2e for `--web`, a plain viewer and `serve`. |
| FR-3841 | Met | HTTP ask `file`, CLI `--file`. The path is resolved with the FR-3415 rules against the item workspace or allowed home `.md`: out of bounds → 403, missing → 404. The prompt gets a `File in question` section with the workspace-relative path and masked content ≤ 64 KiB; a larger or non-UTF-8 file sends the path only. The owner and planner messages are tagged `[file · P]` and the panel shows the answer. TC-3841. |
| FR-3842 | Met | `mode: "edit"` needs `human_browser_rejection` to pass (403 otherwise). The child is `dmuxai -C <ws> run --conversation <work conv> --agent A --no-spec-first --json -` with hybrid resume; the prompt names the file and asks for focused changes. Without a work session it runs in a new conversation seeded with `item_seed_summary` (report `summary`). Lane `in_progress` with a live run lock → 409. TC-3842. |
| FR-3843 | Met | The edit argv never contains `--read-only`, `--allow-sensitive`, `--trust-workspace-config` or `--approve-plan`. Exit 7 → "needs approval — not applied"; any other non-zero exit → "edit failed (exit N): <masked detail>". TC-3843. |
| FR-3844 | Met | `change_snapshot` before and after (git status plus content fingerprint) gives the changed paths, at most 50. They are listed in the planner message ("Changed files:") and in CLI JSON `changed_paths`. The ask link has `mode: "edit"` and the changed `.md` paths. The viewer reloads the file once the answer arrives. Lane, plan, decision and checked are unchanged. TC-3844: unit test and e2e. |
| FR-3845 | Met | CLI `board ask --edit` goes through the owner checks (`--by` non-owner, non-TTY or `DMUXAI_RUN_ID` → exit 5) and typed item-id confirmation, before any message or child. `--revise` with `--edit`/`--file` is a usage error. MCP `board_ask` rejects `edit`/`mode` and its description says so. TC-3845 (CLI and MCP). |
| FR-3846 | Met | The SPEC-board-ux-042 FR-3420 row ends with "Amended by SPEC-md-viewer-chat FR-3842/FR-3845: …". TC-3846. |
| NFR-3840 | Met | The context hook is a synchronous function; opening a viewer without an item context adds no request. The panel's 3 s poll starts only after Send and stops on answer, timeout or close; the panel opens no `EventSource`. Other routes are unchanged. TC-3840 (asset test checks the poll guard, the `teardownChat` call on close and that there is no `EventSource`). |
| Non-goals | Respected | No chat for files outside an item context, no hand editing in the viewer, no spec review decisions. `.github/` untouched. Spec requirements unchanged; only the Status line was flipped. |

## Commands and results

- `cargo fmt` / `cargo fmt --check` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --lib board` — 92 passed
- `cargo test --lib web` — 251 passed, 1 ignored. One earlier run failed
  `tc_2973_full_view_only_or_stale_dashboard_handling` on its 4 s timing
  bound under load; it passed alone and on the full rerun. It is unrelated
  to this change.
- `cargo test --test spec_index` — 17 passed
- `cargo test --test web_board_e2e -- --ignored tc_3840 tc_3844` — 2 passed (headless Chrome, not skipped)
- Regression checks:
  - `cargo test --test web_board_e2e -- --ignored tc_3601 tc_3603 tc_3825 tc_3836` — 5 passed
  - `cargo test --bin dmuxai board` — 39 passed
  - `cargo test --lib mcp` — 52 passed
- `bash scripts/check-spec.sh docs/specs/SPEC-md-viewer-chat.md` — PASS
- `bash scripts/check-spec.sh docs/specs/SPEC-board-ux-042.md` — PASS
- `python3 scripts/check-spec.py --all` — 120/120 pass
- `python3 scripts/check-tc-links.py` — 0 orphan, 0 undefined
- Not run:
  - the full web e2e suite (out of scope);
  - live providers (deterministic fakes only).

## Residual risks

- Changed paths need a git workspace. Without one the edit still runs but
  reports "No files changed."
- The panel picks the thread messages for a file by path substring
  (absolute, viewer display and workspace-relative forms). A message naming
  the file another way is not shown in the panel; it is still in the modal
  thread.
- In `serve` the work-session MD link of a seeded e2e item resolves outside
  the allowed roots ("path outside allowed roots"). That is existing viewer
  behavior; TC-3840 checks only that no panel appears there.
- The HTTP `202` `resume` for an edit is predicted at prepare time. The
  thread message carries the mode the child actually used.
