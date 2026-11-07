# SPEC: opencode workspace cwd

Status: in_progress
Owner: DmuxAI
Created: 2026-11-07
Baseline: `fix/opencode-workspace-cwd` uncommitted workspace-cwd changes
Related: FR-875 (runner workspace directory), SPEC-workspace-dir, TC-2450, TC-2451

## 1. Decision summary

DmuxAI already launches runner children in the resolved workspace, but it
inherits its own `PWD` environment variable. Some runner CLIs, notably opencode,
derive their project root from `PWD`, so `dmuxai -C <target> run … --agent
opencode` could operate on the caller's directory even though the child was
spawned in the target. The decision is to set a runner child's working directory
and inherited `PWD` together. This is a regression fix; the root cause is still
being confirmed by the parent verification run.

## 2. Scope

### In scope

- Agent and custom-runner child processes launched for a workspace: both the
  kernel working directory and inherited `PWD` must equal that workspace.
- The runner model-listing and workspace-fingerprint subprocesses changed with
  the same fix.
- Deterministic unit coverage for the shared helper and end-to-end coverage for
  the built-in opencode runner with `-C`.

### Out of scope / non-goals

- Changing provider argv, prompts, protocols, parsing, or event normalization.
- Changing environment isolation or sandbox/approval policy beyond `PWD`.
- Reworking non-runner subprocesses (docs, git diff, dashboard, notifications).
- Defining how providers other than opencode interpret their project root.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2450 | Root-cause confirmation (inherited `PWD` versus child cwd) is owned by the parent verification run; this documentation does not claim a pass. | DmuxAI | Review approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| workspace | The directory resolved for the current run from `-C`, `DMUXAI_WORKSPACE`, config, or the launch directory. | DmuxAI's own launch directory |
| child cwd | The kernel working directory set with `Command::current_dir`. | Inherited `PWD` |
| `PWD` | The conventional shell environment variable naming the current directory. | The actual kernel cwd |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2450 | Must | Given DmuxAI resolves workspace `W`, when it launches an agent/custom-runner child or one of the runner model/fingerprint subprocesses changed by this fix, then the child's kernel cwd and inherited `PWD` both equal `W`. | No changed child path may leave `PWD` pointing at DmuxAI's launch directory while running in `W`. The environment isolation policy is otherwise unchanged. | TC-2450, TC-2451 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2450 | FR-2450 | integration (Linux CLI) | A fake `opencode` on `PATH` reports its actual `pwd` and the `PWD` inherited from DmuxAI, read from `/proc/$$/environ`; the `-C` target differs from the caller cwd. | Run `dmuxai -C <target> run hi --agent opencode --json`. | Both reported values equal the canonical target workspace, not the caller cwd. | `tests/agent_api_cli.rs` `tc_2450_opencode_run_uses_the_workspace_cwd_and_pwd`; `cargo test --test agent_api_cli tc_2450_opencode_run_uses_the_workspace_cwd_and_pwd` |
| TC-2451 | FR-2450 | unit | A std and Tokio `Command`, plus the custom-agent command builder with a workspace different from the test process cwd. | Apply the shared cwd helper and build a custom command. | `get_current_dir` and the explicit `PWD` entry both equal the workspace; the spawned probe also prints the workspace. | `src/process/mod.rs` `child_cwd_sets_current_dir_and_pwd`; `src/custom_agent.rs` `runner_cwd_is_workspace`; `cargo test --lib child_cwd_sets_current_dir_and_pwd` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema or stored-data change. | Revert the code and docs together. | TC-2450, TC-2451 |
| Provider protocol | No request, event, or parsing change; only the child's cwd and `PWD` are aligned. | Removing the `PWD` assignment restores the previous behavior. | TC-2450 |
| Security / permissions | Does not add credentials, shell invocation, or new environment values; the existing internal-env stripping still applies. | No sandbox or approval policy is weakened. | TC-2451 |
| Platform | The helper is platform-independent. TC-2450 is Linux-only because its probe reads `/proc`; the unit test covers both command types without a live provider. | Unsupported-platform e2e remains untested by TC-2450. | TC-2450, TC-2451 |

## 7. Implementation plan

See `../plans/PLAN-opencode-workspace-cwd.md` and
`../tasks/TASKS-opencode-workspace-cwd.md`.

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add a shared helper that sets cwd and `PWD`, with std and Tokio entry points. | `src/process/mod.rs` | FR-2450 | TC-2451 |
| 2 | Replace direct runner/custom-child `current_dir` calls with the helper. | `src/agent/acp/client.rs`, `src/agent/agy.rs`, `src/agent/codex.rs`, `src/agent/cursor.rs`, `src/agent/custom.rs`, `src/custom_agent.rs` | FR-2450 | TC-2450, TC-2451 |
| 3 | Add the opencode `-C` regression test and extend the custom-runner cwd test. | `tests/agent_api_cli.rs`, `src/custom_agent.rs` | FR-2450 | TC-2450, TC-2451 |

## 8. Review and test exit criteria

- [ ] FR-2450 maps to passing TC-2450/TC-2451 evidence.
- [ ] Parent verification confirms the inherited-`PWD` root cause.
- [ ] `cargo fmt --check`, `cargo test --all-targets`, and `cargo clippy --all-targets --all-features -- -D warnings` pass.
- [ ] `bash scripts/check-spec.sh docs/specs/SPEC-opencode-workspace-cwd.md` passes.
- [ ] `../reviews/REVIEW-opencode-workspace-cwd.md` records results, residual risk, and rollback.
