# TASKS: turn outcome without a final reply (#146) and narrow catalog-hint reclassification

Status: complete  
Specs: [SPEC-custom-agent-provider.md](../specs/SPEC-custom-agent-provider.md) (FR-2125),
[SPEC-model-resolver.md](../specs/SPEC-model-resolver.md) (FR-2126, FR-1729)

- [x] T1 - Extract `failure_signal` from `failure_reason` in
  `src/custom_agent.rs` and keep `failure_reason` as a thin wrapper (same
  check order, same reasons). Requirements: FR-2125, FR-1849.
  Verify: `cargo test --all-targets custom_agent::tests`.
- [x] T2 - Add `TurnOutcome` and the pure `classify_turn_outcome` helper:
  empty reply + workspace change -> `SuccessWithoutReply`; empty reply without
  a change -> `Failed("empty reply text")`; JSONL error, `failure_stderr` match
  and non-zero exit win over the workspace change. Requirements: FR-2125.
  Verify: `cargo test --all-targets tc_2125`.
- [x] T3 - Sample the workspace with
  `git status --porcelain=v1 -z --untracked-files=normal` before and after the
  child process in `src/agent/custom.rs` (edit-allowed turns only, 5 s timeout,
  `None` without git or outside a repository) and pass the result to
  `classify_turn_outcome`. Requirements: FR-2125.
  Verify: `cargo test --all-targets tc_2126`.
- [x] T4 - Emit `AgentEvent::Notice("<label> finished without a final message;
  workspace files changed")` and finish the turn as `Completed` (not `Error`)
  on `SuccessWithoutReply`; keep the "empty reply text" failure when the
  workspace did not change. Requirements: FR-2125.
  Verify: `cargo test --all-targets tc_2126`, `cargo test --all-targets tc_2127`.
- [x] T5 - Add `dmuxai::runner_fallback::is_model_unavailable_message` and use
  it in `App::attach_model_catalog_hint` so the kind becomes `ModelUnavailable`
  only for unknown/unavailable-model messages; every other failure keeps its
  kind, the hint is still attached. Requirements: FR-2126, FR-1729.
  Verify: `cargo test --all-targets tc_2128`, `cargo test --all-targets tc_2129`,
  `cargo test --all-targets tc_1748`.
- [x] T6 - Update the specs (`FR-1729` wording, new `FR-2125` / `FR-2126`
  rows, `TC-2125..TC-2129` rows), `docs/agents/custom-agents.md`, the
  CHANGELOG, and the review artifact. Requirements: all.
  Verify: `python3 scripts/check-spec.py --all`,
  `python3 scripts/check-tc-links.py`.
- [x] T7 - Run the narrow Rust gates for the changed area. Requirements: all.
  Verify: `cargo check --all-targets`, `cargo fmt --all -- --check`, and the
  test commands listed above.
