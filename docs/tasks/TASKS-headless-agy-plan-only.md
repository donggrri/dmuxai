# TASKS: headless AGY plan-only runs must not report success

Status: complete  
Spec: [../specs/SPEC-plan-gate.md](../specs/SPEC-plan-gate.md) (FR-2121)

- [x] T1 - Add the headless edit-policy flags to `App` and set them in
  `run_headless` for a direct headless run on an explicit agent with the
  write-capable policy (`--approve-plan` requires file edits; without it a
  plan-only reply needs approval); clear them when the run ends.
  Requirements: FR-2121. Verify: `cargo test tc_2121_headless_approve_plan_requires_file_edits`.
- [x] T2 - Make `graph_node_expects_file_edits` honour the flag so the quiet
  1-node headless run sends `require_file_edits = true`, while the FR-937
  interactive quiet behaviour is unchanged.
  Requirements: FR-2121. Verify: `cargo test direct_one_node_run_does_not_require_file_edits`
  and `cargo test tc_2121_headless_approve_plan_requires_file_edits`.
- [x] T3 - Emit the `PLAN_ONLY_APPROVAL_NOTICE` marker from the AGY adapter when
  an Agent-policy turn completes with no observed writes and a plan-like reply,
  and keep the stricter-edit retry for runs that require file edits.
  Requirements: FR-2121. Verify: `cargo test tc_2123_agy_plan_only_reply_emits_approval_notice`
  and `cargo test tc_2124_agy_plan_only_with_required_edits_fails_after_one_retry`.
- [x] T4 - Map the marker in the app: a headless run without `--approve-plan`
  reports exit 7 with the plan-only note and keeps the plan text; the notice
  never reaches the transcript. Requirements: FR-2121. Verify: `cargo test tc_2122_headless_plan_only_reply_exits_seven`.
- [x] T5 - Update the spec, the API docs, and run the gates.
  Requirements: all. Verify: `python3 scripts/check-spec.py --all`,
  `python3 scripts/check-tc-links.py`, `cargo fmt --all -- --check`,
  `cargo check --all-targets`, and the named tests above.
