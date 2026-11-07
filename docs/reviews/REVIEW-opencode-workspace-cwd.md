# Review: opencode workspace cwd

Status: complete
Date: 2026-11-07
Reviewer: parent verification evidence recorded; final repository gates passed
Scope: `src/process/mod.rs`, `src/agent/acp/client.rs`, `src/agent/agy.rs`,
`src/agent/codex.rs`, `src/agent/cursor.rs`, `src/agent/custom.rs`,
`src/custom_agent.rs`, `tests/agent_api_cli.rs`

## Summary

The uncommitted diff implements FR-2450 by adding `apply_child_cwd` /
`apply_child_cwd_tokio` and replacing direct runner-child `current_dir` calls
with it. The helper sets both the kernel working directory and `PWD`, preventing
a child from running in the workspace while reporting the parent's launch
directory through `PWD`.

Root cause confirmed: inherited `PWD` differed from the subprocess `current_dir`,
causing OpenCode's project root to resolve to the caller's directory. A real
`pwd` smoke now prints the correct target workspace after the helper change.

## Requirement compliance

| Requirement | Result | Evidence |
| --- | --- | --- |
| FR-2450 | Implemented; clippy, full tests, release build, spec gate, and live `pwd` smoke all passed. | TC-2450, TC-2451 |

- TC-2450 exercises `dmuxai -C <target> run … --agent opencode` with a fake
  opencode that reads its actual `pwd` and inherited `PWD` from
  `/proc/$$/environ`; both must equal the canonical target.
- TC-2451 asserts the std/Tokio helper sets `current_dir` and `PWD`, and extends
  the custom-runner test to check both before spawning the probe.

## Diff observations

- `src/process/mod.rs` adds the shared helper and documents why inherited `PWD`
  is insufficient.
- The changed spawn paths in ACP, AGY, Codex, Cursor, custom-agent fingerprint,
  `build_custom_command`, and custom model listing now use the helper.
- No provider protocol, argv, prompt, persistence, or environment-isolation
  policy was changed. The existing internal-env stripping is untouched.
- The integration test comment also cites FR-2451; this spec keeps FR-2450 as
  the single requirement ID and maps both TC-2450 and TC-2451 to it.

## Residual risk

- `PWD` is now explicitly overridden for runner children. A provider that
  deliberately relied on the parent's `PWD` would observe the workspace instead;
  that is consistent with the already-selected child cwd.
- TC-2450 is Linux-only because it reads `/proc`; other platforms rely on the
  helper unit test and command-construction coverage.
- Non-runner subprocesses still set `current_dir` directly and are outside this
  fix's scope.

## Verification

Final results (documentation-only completion; no Rust or test changes were made
during this verification pass):

| Command | Result |
| --- | --- |
| `scripts/gate.sh --keep-going` | Exit 0. Includes `cargo fmt`, clippy `--all-targets --all-features -- -D warnings`, all-features tests, and the all-spec/TC-link gates. |
| `cargo test --locked --all-targets --no-fail-fast` | Exit 0. |
| `bash scripts/check-spec.sh docs/specs/SPEC-opencode-workspace-cwd.md` | Exit 0. |
| `cargo build --release --locked` | Exit 0. |
| prior live free-model `pwd` smoke, run from a different shell cwd | Exit 0, `ok=true`, `error_kind=null`; the child reported exactly the target workspace. |

Prior runs, retained for an honest record:

| Command | Result |
| --- | --- |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | PASSED |
| release build | PASSED |
| real `pwd` smoke for the target workspace | PASSED; prints the correct target workspace, confirming the root cause |
| `cargo fmt --check` | FAILED; `cargo fmt` was run to correct formatting |
| `cargo test --locked --all-targets --no-fail-fast` | One failure: `agent::custom::tests::tc_2084_no_runner_gets_mcp_args_or_files` at `src/agent/custom.rs:1959`, `assert_eq!` left 3 right 2. All other tests, including the new regression test, passed. |

The TC-2084 failure was environment-dependent: `load_with_env` also read the
real user config, which supplied a third MCP server, while the test only wrote
two workspace servers. The test now calls
`load_with_user_config_and_env(ws.path(), None, &env)` so the assertion checks
exactly the two workspace servers. The test's intent (no runner receives MCP
args or files) is unchanged; only the unrelated user-config input is isolated.
That fix is covered by the now-passing full test run above.

Live testing covered OpenCode only. No other provider was exercised live, so
their behavior relies on the deterministic helper and command-construction
coverage (TC-2451).

## Rollback

Revert the code and these artifacts together. No schema migration, stored data,
or provider protocol change is involved.
