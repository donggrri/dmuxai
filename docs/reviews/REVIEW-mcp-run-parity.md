# REVIEW: MCP run parity with headless CLI run

Status: pass with one follow-up note
Reviewer: DmuxAI
Date: 2026-11-07
Commit under review: working tree on top of `b1ff818` (uncommitted)

## Scope compliance

- FR-2450 (`model` forwarding) — covered by `tc_2450_mcp_run_forwards_parity_fields`
  (src/mcp_server.rs) and `tc_2450_mcp_run_argv_forwards_parity_flags` (src/cli.rs).
- FR-2451 (forward `--approve-plan` / `--allow-sensitive` only when true) — same
  two tests assert exact argv and omitted-flag behavior.
- FR-2452 (reject `mode`, validate `model` vs `agent`/`graph`, schema shape) —
  `tc_2451_mcp_run_rejects_mode_and_validates_model` plus the run-tool schema
  assertions in `mcp_stdio_protocol_and_tools`, which now carry the explicit
  `TC-2453 / FR-2452` traceability citation (comment link to the already-shipped
  coverage).
- FR-2453 (exit 7 surfaces `isError` + `approval_required: true`) —
  `tc_2452_mcp_run_surfaces_exit_7_approval_required`.
- Non-goals held: CLI parsing, gates, HTTP API and provider calls untouched;
  child spawning unchanged; no allowlist entries added (allowlist count stayed
  at 227; the orphan was resolved by mapping, not by shrinking rules).

## Commands and results (CARGO_TARGET_DIR=<repo>/target)

| Command | Result |
| --- | --- |
| `cargo test --test spec_index` | 10 passed, 0 failed (incl. `tc_1803_tc_links_have_tests_or_allowlist`) |
| `python3 scripts/check-tc-links.py` | 1056 defined, 0 orphan, 0 undefined-test, 0 stale |
| `bash scripts/check-spec.sh docs/specs/SPEC-mcp-run-parity.md` | PASS |
| `python3 scripts/check-spec.py --all` | 86 specs, 86 pass, 0 fail |
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean |
| `cargo test --all-targets` | 33 test binaries, 0 failures (667 passed, 1 ignored in lib; 610 passed, 6 ignored in bin; rest per-target) |

## Follow-up fixed during test gate

- `tc_2084_no_runner_gets_mcp_args_or_files` (src/agent/custom.rs) was
  environment-dependent: `RuntimeEnv::from_process()` let a real user-level
  dmuxai config add a third MCP server to a hermetic assertion. Fixed by
  constructing `RuntimeEnv::default()` with `trust_workspace_config: true`
  instead, same isolation class as the binary-test XDG fix.

## Residual risks

- TC-2453 is covered via a comment-only (not `fn tc_2453_*`) link; the schema
  assertions live inside the broad `mcp_stdio_protocol_and_tools` test. A future
  dedicated `tc_2453_*` test would upgrade it to a strong link without changing
  behavior.
- MCP `run` child spawning is exercised only through stub backends and argv
  assertions; no live-provider coverage was added (intentional, per spec
  assumptions).

## Rollback

Revert the `src/mcp_server.rs`, `src/cli.rs`, docs changes; MCP clients regain
the prior `run` tool schema (no parity fields, no mode rejection, no explicit
exit-7 marker). No persistence or migration step is involved.
