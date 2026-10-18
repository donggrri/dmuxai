# REVIEW: v0.2.2 release audit

Date: 2026-10-18 (Asia/Seoul). Base: `main` at 54637b5 (#71), plus the release PR.

## Scope

Milestone [v0.2.2](https://github.com/donggrri/dmuxai/milestone/2): issues #54-#56 and #61, plus the built-in grok runner (#66, added to v0.2.2 by Dongjin on 2026-10-19 00:00 KST).

| Issue | SPEC | PR (squash SHA) |
| --- | --- | --- |
| #54 Centered prompt box | SPEC-centered-prompt | #58 (b224c50) |
| #55 Left sessions sidebar, TUI picker running mark | SPEC-session-sidebar | #59 (6147561) |
| #56 Workspace directory (`-C`, header path, switch dialog) | SPEC-workspace-dir | #60 (e0c988f) |
| #61 Horizontal swimlane graph view | SPEC-graph-swimlane | #65 (8c6b999) |
| Built-in grok (Grok Build) runner, grok/fast model policy | SPEC-grok-runner, SPEC-model-policy FR-910..912 | #66 (a4c50ae) |
| TUI conversation picker width (found while preparing the release screens) | SPEC-session-sidebar FR-865 | #70 (f1b36ae) |
| grok turns no longer hang after the final reply; text around tool calls is separated | SPEC-grok-runner FR-913..917 | #72 (a5de430) |
| grok tool-call activity, dashboard activity detail, cancel notice for all runners, welcome lists runners, dashboard token kept out of snapshots | SPEC-grok-runner FR-980, SPEC-running-indicators FR-981..982, SPEC-first-run FR-983, SPEC-web-dashboard FR-984 | #74 (c6a12b8) |
| Resizable left and right dashboard sidebars (drag, keyboard, clamp, persisted width, prompt stays centered) | SPEC-sidebar-resize FR-970..974, UX-970..971 | #71 (54637b5) |

The SPECs, PLAN, TASKS and ROADMAP landed in #57 (2374989) and #62 (2d776d5). Also in this release: CI on `ubuntu-latest` (#63, 77ef270) and the `kill -- -<pgid>` fix (MCP server in #63; custom agent, Cursor and Office in #64, 787436b).

Not in this release: the plan approval gate (#67) and the dashboard parity spec (#68). They are open PRs and are not on `main`.

## Method

These are the v0.2.1 checks 1-4, run by a throwaway script over the five SPECs (plus the SPEC-model-policy FR-910..912 rows):

1. Every FR/NFR/UX row names a TC.
2. Every TC has a TEST-PLAN row that is not ⏳.
3. Every test name in backticks in those rows exists as an `fn` in `src/` or `tests/`.
4. SPEC Status is `Implemented (v0.2.2)` and every TASKS-v0.2.2 row is done.

Then: CHANGELOG checked against the merged PRs, `scripts/spec-index.sh`, and `tests/spec_index.rs`.

## Result

- Five SPECs, 43 requirements, 48 TCs (four SPECs with 26 requirements and 24 TCs, including FR-865/TC-865 from #70, plus SPEC-grok-runner with 17 requirements and 24 TCs, including FR-913..917/TC-913..918 from #72), and SPEC-model-policy FR-910..912 / TC-910..912. Each TC has a ✅ TEST-PLAN row and each named test exists. All five SPECs say `Implemented (v0.2.2)`, and TASKS-v0.2.2 rows 0-13 are done (row 14, the release itself, is checked off after the tag).
- Fixed in this PR:
  - the CHANGELOG had no entry for the `kill --` fix or the CI change, so it gets `Fixed` and `CI` sections;
  - the Unreleased section becomes `[0.2.2] - 2026-10-18`, with a Linux-only platform note and a compare link;
  - ROADMAP rows 1-3 said `planned` although #58-#60 had merged; they now name the PR and SHA, and row 5 (release) is added;
  - the PLAN-v0.2.2 Status and Specs line (SPEC-graph-swimlane was missing), and TASKS-v0.2.2 row 14 (release, open until the tag).
- Platform: only Linux was checked by hand. The Windows target compiles and passes clippy in CI. The live workspace switch returns 501 on Windows (ROADMAP "Later").
- Grok runner (#66): TEST-PLAN rows TC-890..907 and TC-910..912 and the SPEC Status `Implemented (v0.2.2)` were added in this PR. Verified end to end before merge with real grok-4.7 inside `dmuxai --web` (TUI picker and F2 list, dashboard prompt streamed and completed, cancel left no `grok` process); evidence in REVIEW-grok-runner.md, screenshots `grok-runner-*.png`.
- grok exit fix (#72): root cause was grok 1.0.62 auto-wake queueing a prompt that headless mode never runs after the turn; the preset now sets `GROK_AUTO_WAKE=0` and `--no-wait-for-background`, and the `end` line ends the turn. TEST-PLAN rows TC-913..918 were added in this PR. Evidence, smoke turns and reviewer notes: REVIEW-grok-runner-exit.md.
- v0.2.2 dogfood fixes (#74): adds FR-980..984 (5 requirements) and TC-980..987 plus TC-816 (9 TCs) across SPEC-grok-runner, SPEC-running-indicators, SPEC-first-run and SPEC-web-dashboard, on top of the counts above. TEST-PLAN rows were added in this PR. Implementation by Grok Build (grok-4.7), review by opencode beam-free (follow-up approved by nemotron-3-ultra-free), live grok-4.7 verification inside dmuxai: see docs/reviews/REVIEW-v022-dogfood.md.
- Resizable sidebars (#71): adds SPEC-sidebar-resize with FR-970..974 and UX-970..971 (7 requirements) and TC-970..972 (3 TCs), on top of the counts above; SPEC Status `Implemented (v0.2.2)` and TEST-PLAN rows were added in that PR. Implementation by Grok Build (grok-4.7); review by cursor-agent claude-sonnet-5-5-high after the opencode free models timed out (two rounds, approved): see docs/reviews/REVIEW-sidebar-resize.md. Light screenshots: `v022-sidebar-resize-default.png`, `-wide.png`, `-narrow.png`.
- Screens: light-theme screenshots of each user-visible change were taken from the release binary with demo data and shown to Dongjin before the tag.

## Review

- opencode `deepseek-v4.1-flash-free` reviewed the release diff: no factual blockers after the fixes above (it flagged the PR range, which included issue #61, the `kill --` attribution across #63 and #64, and TASKS row 14 being checked before the tag).
- opencode `glm-5.3-flash-free` timed out after 400 seconds without an answer.

## Release checks

- `cargo fmt --check`.
- `cargo clippy --locked --all-targets -- -D warnings`, on Linux and on `--target x86_64-pc-windows-gnu`.
- `cargo test --locked --all-targets`, plus the ignored headless Chrome e2e tests.
- `cargo build --release --locked`; `dmuxai --version` prints 0.2.2.
- After Dongjin's OK, merge, then tag `v0.2.2` on the merge commit. The Release workflow publishes `dmuxai-linux-x86_64.tar.gz` and `.sha256`. Check the checksum and the layout (`dmuxai-linux-x86_64/dmuxai`), and run `dmuxai --version` (0.2.2) and `dmuxai init` in a temporary HOME.
