# TASKS: Multiple DmuxAI instances in one workspace

Status: complete  
Spec: [SPEC-multi-instance-workspace.md](../specs/SPEC-multi-instance-workspace.md)

- [x] T1 - Add the transactional additive runtime-session schema, session UUID,
  owner column, heartbeat, clean close, and live-peer queries. Requirements:
  BR-050, NFR-050. Verify: `cargo test runtime_session_schema_extension_preserves_existing_data`
  and `cargo test closed_or_expired_session_turns_are_recovered`.
- [x] T2 - Scope startup recovery to turns owned by missing, closed, or expired
  sessions and preserve live peer turns. Requirements: FR-051, BR-050. Verify:
  `cargo test multiple_live_instances_start_in_separate_conversations` and
  `cargo test closed_or_expired_session_turns_are_recovered`.
- [x] T3 - Start a peer instance in a fresh conversation while retaining latest
  conversation restore when no peer exists. Requirements: FR-050, FR-052.
  Verify: `cargo test multiple_live_instances_start_in_separate_conversations`
  and `cargo test a_new_app_instance_loads_the_latest_conversation`.
- [x] T4 - Make the same-conversation/agent busy invariant atomic across
  SQLite connections and cover same-agent execution in separate instance
  conversations. Requirements: BR-051. Verify: `cargo test busy_agent_is_rejected_across_runtime_sessions`
  and `cargo test same_agent_can_run_in_separate_instance_conversations`.
- [x] T5 - Update README, baseline requirements/evidence, and deterministic
  tests; run the spec check, format, all-target tests, and clippy. Requirements:
  FR-050, FR-051, FR-052, BR-050, BR-051, NFR-050. Verify: `scripts/check-spec.ps1`
  for the feature spec and baseline, `cargo fmt --check`, `cargo test --all-targets`,
  and `cargo clippy --all-targets --all-features -- -D warnings`.
