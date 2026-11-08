# REVIEW: MCP run parity with headless CLI run

Status: pass with one follow-up note
Reviewer: DmuxAI
Date: 2026-11-08
Commit under review: working tree on top of `origin/main` (`2b610bf`); branch `verify/pr260` at `0d839ee`

## Scope compliance

- FR-2509 (`model` forwarding) — covered by `tc_2509_mcp_run_forwards_parity_fields`
  (src/mcp_server.rs) and `tc_2509_mcp_run_argv_forwards_parity_flags` (src/cli.rs,
  binary target).
- FR-2510 (forward `--approve-plan` / `--allow-sensitive` only when true) — same
  two tests assert exact argv and omitted-flag behavior.
- FR-2511 (reject `mode`, validate `model` vs `agent`/`graph`, schema shape) —
  `tc_2510_mcp_run_rejects_mode_and_validates_model` plus the run-tool schema
  assertions in `mcp_stdio_protocol_and_tools`, which now carry the explicit
  `TC-2512 / FR-2511` traceability citation (comment link to the already-shipped
  coverage).
- FR-2512 (exit 7 surfaces `isError` + `approval_required: true`) —
  `tc_2511_mcp_run_surfaces_exit_7_approval_required`.
- Requirement and test IDs were renumbered `2450–2453` → `2509–2512` to avoid
  colliding with pending PRs; all spec, plan, task, review, and source test
  references use the new IDs consistently.
- Non-goals held: CLI parsing, gates, HTTP API and provider calls untouched;
  child spawning unchanged; no allowlist entries added (allowlist count stayed
  at 227; the orphan was resolved by mapping, not by shrinking rules).

## Commands and results (focused gates only)

| Command | Result |
| --- | --- |
| `cargo fmt --check` | clean (exit 0) |
| `cargo test --lib mcp_server` | 6 passed, 0 failed (TC-2509..TC-2512 plus the schema test) |
| `cargo test --bin dmuxai cli::tests` | 10 passed, 0 failed (includes `tc_2509_mcp_run_argv_forwards_parity_flags`) |
| `cargo test --bin dmuxai agent::custom::tests::tc_2084_no_runner_gets_mcp_args_or_files` | 1 passed, 0 failed |
| `cargo test --test spec_index` | 10 passed, 0 failed (incl. `tc_1803_tc_links_have_tests_or_allowlist`) |
| `python3 scripts/check-tc-links.py` | 1058 defined, 396 strong, 435 comment-only, 227 allowlisted, 0 orphan, 0 undefined-test, 0 stale |
| `bash scripts/check-spec.sh docs/specs/SPEC-mcp-run-parity.md` | PASS |
| `bash scripts/check-spec.sh docs/specs/SPEC-agent-api.md` | PASS |
| `python3 scripts/check-spec.py --all` | 87 specs, 87 pass, 0 fail |

`cli` and `agent::custom` tests live in the `dmuxai` binary target, so they are
run with `--bin dmuxai`; `mcp_server` is a library module and runs with `--lib`.
No full `scripts/gate.sh` run was part of this completion pass.

## Isolation note

`tc_2084_no_runner_gets_mcp_args_or_files` (src/agent/custom.rs) is hermetic on
main: it calls `DmuxSettings::load_with_user_config_and_env(ws.path(), None, &env)`
with an explicit `None` user config, so a real user-level dmuxai config cannot add
a third MCP server to the assertion. The parity change does not modify that test.

## Residual risks

- TC-2512 is covered via a comment-only (not `fn tc_2512_*`) link; the schema
  assertions live inside the broad `mcp_stdio_protocol_and_tools` test. A future
  dedicated `tc_2512_*` test would upgrade it to a strong link without changing
  behavior.
- MCP `run` child spawning is exercised only through stub backends and argv
  assertions; no live-provider coverage was added (intentional, per spec
  assumptions).

## Rollback

Revert the `src/mcp_server.rs`, `src/cli.rs`, docs changes; MCP clients regain
the prior `run` tool schema (no parity fields, no mode rejection, no explicit
exit-7 marker). No persistence or migration step is involved.
