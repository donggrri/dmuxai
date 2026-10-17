# REVIEW: v0.2.1 release audit

Date: 2026-10-18 (Asia/Seoul). Base: `main` at 0c25cac (#52), plus the release PR.

## Scope

Milestone [v0.2.1](https://github.com/donggrri/dmuxai/milestone/1): issues #40-#46, all closed.

| Issue | SPEC | PR (squash SHA) |
| --- | --- | --- |
| #40 Cursor text shown twice | SPEC-cursor-stream-dedupe | #39 (049dc68) |
| #45, #46 Fresh install, `dmuxai init`, README tarball | SPEC-first-run | #48 (e394ab7) |
| #42 Running indicators | SPEC-running-indicators | #49 (f6165fe) |
| #43 Documents folder tree | SPEC-docs-tree | #50 (9c258eb) |
| #41 Cursor latency / pre-start | SPEC-cursor-latency | #51 (bf31a2e) |
| #44 To-dos sidebar | SPEC-todo-sidebar | #52 (0c25cac) |

The SPECs, PLAN, TASKS and ROADMAP landed in #47 (9225af6).

## Method

These are the v0.2.0 checks 1-5, run by a throwaway script over the six SPECs:

1. Every FR/NFR/UX row names a TC.
2. Every TC has a TEST-PLAN row that is not ⏳.
3. Every test name in backticks exists as an `fn` in `src/` or `tests/`.
4. SPEC Status is `Implemented (v0.2.1)` and every TASKS-v0.2.1 row is done.

Then: CHANGELOG checked against the SPECs, `scripts/spec-index.sh`, and `tests/spec_index.rs`.

## Result

- Six SPECs, 30 requirements, 25 TCs. Each TC has a ✅ TEST-PLAN row and each named test exists. The only names the script did not find are config keys in backticks (`call_id`, `allowed_runners`, `allowed_models`, `builtin_opencode`, `started_at`, `has_children`), which are not test names.
- Fixed in this PR:
  - the CHANGELOG had no entry for the #40 Cursor duplicate fix, so it gets a `Fixed` section;
  - the Unreleased section becomes `[0.2.1] - 2026-10-18`, with a Linux-only platform note;
  - ROADMAP rows 6 and 8, the TASKS-v0.2.1 rows 9 (owner), 11 and 12, and the PLAN-v0.2.1 Status are updated.
- Platform: only Linux was checked by hand. The Windows target compiles and passes clippy in CI (windows-latest) and locally (`x86_64-pc-windows-gnu`). Windows behaviour of the v0.2.1 features is postponed (ROADMAP "Later").

## Release checks

- `cargo fmt --check`.
- `cargo clippy --locked --all-targets -- -D warnings`, on Linux and on `--target x86_64-pc-windows-gnu`.
- `cargo test --locked --all-targets`, plus the ignored headless Chrome e2e tests.
- After tagging: the Release workflow publishes `dmuxai-linux-x86_64.tar.gz` and `.sha256`. Check the checksum and the layout (`dmuxai-linux-x86_64/dmuxai`), and run `dmuxai --version` (0.2.1) and `dmuxai init` in a temporary HOME.
