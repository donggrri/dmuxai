# REVIEW: v0.2.1 specs (PR #47)

Reviewer: opencode-haiku (`dmuxai run --agent opencode-haiku --mode ask`). Cursor is not a reviewer for v0.2.1.

## Round 1: REQUEST_CHANGES

| # | Finding | Resolution |
| --- | --- | --- |
| 1 | SPEC-cursor-stream-dedupe missing | It is in PR #39, which merges first; this PR is then rebased |
| 2 | TEST-PLAN rows separated from the table by a blank line | Fixed |
| 3 | FR-841 filter/search has no behavioural test | TC-842 (headless Chrome) now asserts the filter and search results |
| 4 | Pre-warm leak risks: crash, process group, exited branch, pid assertions, idle timer, spawn side effects | FR-805/806/807 updated: own process group / job object; 1 s App tick for idle expiry; no side effects before the prompt; measured that stdin EOF makes cursor-agent exit with no model call; TC-805..807 assert the pid is gone; new TC-808 for an exited warm process |
| 5 | Built-in opencode changes v0.2.0 behaviour and graph routing; no model-listing timeout; template lacks `builtin_opencode` | Behaviour change stated (CHANGELOG at release); FR-813 keeps graph routing opt-in via `allowed_runners`; 10 s listing timeout; template includes `[agents] builtin_opencode` |
| 6 | `/api/docs/tree` contract under-specified | Response shape `{dir, entries, truncated}`; directory fields defined; status per error case (403/404/400); the cap is a parameter, tested with 3 |
| 7 | ROADMAP/PLAN/TASKS order and kill triggers disagree | The release waits for 1–7; PLAN lists agent removal |
| nits | Cancelled pill; "next interaction"; NFR-820 test; template oracle; TC-815 oracle; benchmark baseline; tab shell ownership and Esc; TASKS UX-821 | All applied |

`scripts/check-spec.ps1` needs PowerShell, which is not installed on this box; `tests/spec_index.rs` passes (3/3).
