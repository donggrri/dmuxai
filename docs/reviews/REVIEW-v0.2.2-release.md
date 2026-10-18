# REVIEW: v0.2.2 release audit

Date: 2026-10-18 (Asia/Seoul). Base: `main` at 8c6b999 (#65), plus the release PR.

## Scope

Milestone [v0.2.2](https://github.com/donggrri/dmuxai/milestone/2): issues #54-#56 and #61.

| Issue | SPEC | PR (squash SHA) |
| --- | --- | --- |
| #54 Centered prompt box | SPEC-centered-prompt | #58 (b224c50) |
| #55 Left sessions sidebar, TUI picker running mark | SPEC-session-sidebar | #59 (6147561) |
| #56 Workspace directory (`-C`, header path, switch dialog) | SPEC-workspace-dir | #60 (e0c988f) |
| #61 Horizontal swimlane graph view | SPEC-graph-swimlane | #65 (8c6b999) |

The SPECs, PLAN, TASKS and ROADMAP landed in #57 (2374989) and #62 (2d776d5). Also in this release: CI on `ubuntu-latest` (#63, 77ef270) and the `kill -- -<pgid>` fix (#63, #64, 787436b).

Not in this release: the grok runner (#66), the plan approval gate (#67) and the dashboard parity spec (#68). They are open PRs and are not on `main`.

## Method

These are the v0.2.1 checks 1-4, run by a throwaway script over the four SPECs:

1. Every FR/NFR/UX row names a TC.
2. Every TC has a TEST-PLAN row that is not ⏳.
3. Every test name in backticks in those rows exists as an `fn` in `src/` or `tests/`.
4. SPEC Status is `Implemented (v0.2.2)` and every TASKS-v0.2.2 row is done.

Then: CHANGELOG checked against the merged PRs, `scripts/spec-index.sh`, and `tests/spec_index.rs`.

## Result

- Four SPECs, 25 requirements, 23 TCs. Each TC has a ✅ TEST-PLAN row and each named test exists. All four SPECs say `Implemented (v0.2.2)`, and TASKS-v0.2.2 rows 0-13 are done.
- Fixed in this PR:
  - the CHANGELOG had no entry for the `kill --` fix or the CI change, so it gets `Fixed` and `CI` sections;
  - the Unreleased section becomes `[0.2.2] - 2026-10-18`, with a Linux-only platform note and a compare link;
  - ROADMAP rows 1-3 said `planned` although #58-#60 had merged; they now name the PR and SHA, and row 5 (release) is added;
  - the PLAN-v0.2.2 Status and Specs line (SPEC-graph-swimlane was missing), and TASKS-v0.2.2 row 14 (release).
- Platform: only Linux was checked by hand. The Windows target compiles and passes clippy in CI. The live workspace switch answers 501 on Windows (ROADMAP "Later").
- Screens: light-theme screenshots of each user-visible change were taken from the release binary with demo data and shown to Dongjin before the tag.

## Release checks

- `cargo fmt --check`.
- `cargo clippy --locked --all-targets -- -D warnings`, on Linux and on `--target x86_64-pc-windows-gnu`.
- `cargo test --locked --all-targets`, plus the ignored headless Chrome e2e tests.
- `cargo build --release --locked`; `dmuxai --version` prints 0.2.2.
- After Dongjin's OK, merge, then tag `v0.2.2` on the merge commit. The Release workflow publishes `dmuxai-linux-x86_64.tar.gz` and `.sha256`. Check the checksum and the layout (`dmuxai-linux-x86_64/dmuxai`), and run `dmuxai --version` (0.2.2) and `dmuxai init` in a temporary HOME.
