# TASKS: Built-in grok runner

Status: Implemented
Spec: [SPEC-grok-runner](../specs/SPEC-grok-runner.md)
Plan: [PLAN-grok-runner](../plans/PLAN-grok-runner.md)

Phase 2 is implemented. Each item below was checked against the spec. `agent` and `app` tests live on the `dmuxai` binary, so those filters use `cargo test --bin dmuxai`.

- [x] T1 — Mock fixture and `grok_preset(program)`.
  Requirements: FR-892, FR-893, FR-894, FR-895, FR-896, FR-898, NFR-890.
  Files: `tests/fixtures/fake-grok.sh`, `src/custom_agent.rs`.
  Verify: `cargo test --lib custom_agent::tests::grok_preset_argv_is_headless_and_not_a_shell grok_streaming_json_extracts_text_and_session grok_models_parse_default_filter_and_fast grok_auth_is_models_stdout_only grok_preset_color_is_distinct`

- [x] T2 — `[agents] builtin_grok`, `DMUXAI_GROK_BIN` / `PATH` / `~/.grok/bin/grok`, preset append, custom name replaces the preset.
  Requirements: FR-890, FR-891.
  Files: `src/settings.rs`.
  Verify: `cargo test --lib settings::builtin_grok_tests::builtin_grok_preset_registration`

- [x] T3 — Turn runtime on the mock: text, session id, resume `-r`, `-m grok-4.7`, errors, login gate with no `-p`, process-group cancel via existing `kill_group`.
  Requirements: FR-893, FR-894, FR-895, FR-896, FR-897.
  Files: `src/agent/custom.rs`.
  Verify: `cargo test --bin dmuxai grok_mock_turn_returns_text_and_reports_errors grok_unauthenticated_send_explains_login grok_cancel_kills_the_process_group`

- [x] T4 — `validate_model_for_runner`: `fast` rejected for every runner; `grok-*` allowed only for grok; cursor stays composer-2.5 only.
  Requirements: FR-910, FR-911.
  Files: `src/model_policy.rs`.
  Verify: `cargo test --lib model_policy::tests::fast_models_are_rejected_for_every_runner grok_models_are_allowed_only_for_the_grok_runner`

- [x] T5 — Graph exclusion, `/graph` status text, effective model skips composer-2.5, TUI and dashboard color, select probes models.
  Requirements: FR-896, FR-898, FR-899.
  Files: `src/graph/policy.rs`, `src/app.rs`.
  Verify: `cargo test --lib graph::policy::tests::builtin_grok_is_not_graph_routed_unless_listed` and `cargo test --bin dmuxai grok_chat_and_dashboard_use_preset_color grok_effective_model_skips_composer_default graph_status_names_builtin_grok_exception selecting_grok_requests_a_model_probe`

- [x] T6 — `dmuxai status --json` lists grok and marks it not graph-allowed until `allowed_runners` includes it.
  Requirements: FR-890, FR-899.
  Files: `tests/agent_api_cli.rs`.
  Verify: `cargo test --test agent_api_cli fresh_install_lists_grok`

- [x] T7 — Init starter, README First run, CHANGELOG, `docs/agents/custom-agents.md`.
  Requirements: FR-900.
  Files: `src/init.rs`, `README.md`, `CHANGELOG.md`, `docs/agents/custom-agents.md`.
  Verify: `cargo test --lib init::tests::starter_config_mentions_builtin_grok` and `cargo test --test readme_install readme_and_changelog_mention_builtin_grok`

- [x] T8 — Comment above `disallow_patterns` in `docs/dmuxai-graph.yaml`. Node models stay `composer-2.5`. `*grok*` stays in the list.
  Requirements: FR-912.
  Files: `docs/dmuxai-graph.yaml`, `tests/model_policy_repo.rs`.
  Verify: `cargo test --test model_policy_repo dmuxai_graph_yaml_keeps_grok_disallow_and_explains_why`

- [x] T9 — Ignored live smoke. Do not remove `#[ignore]`.
  Requirements: FR-892, FR-893.
  Files: `src/agent/custom.rs` `grok_live_smoke_headless_turn`.
  Verify: present and ignored. Run only by hand: `cargo test --bin dmuxai grok_live_smoke_headless_turn -- --ignored`

- [x] T10 — Gates after the code lands.
  Requirements: all Must rows in SPEC-grok-runner and FR-910..FR-912.
  Verify: `cargo fmt --check`, `cargo test --all-targets`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --test spec_index`, `cargo test --test model_policy_repo`. Record `grok_live_smoke_headless_turn` as ignored, not as passed.
