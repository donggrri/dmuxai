# TASKS: first-run

Status: complete
Spec: [SPEC-first-run](../specs/SPEC-first-run.md)
Summary row: `docs/tasks/TASKS-v0.2.1.md` tasks 2-4.

- [x] T1 — Preset and `builtin` flag, policy-aware listing with timeout.
  Requirements: FR-812, FR-813. Verify:
  `cargo test --locked --lib custom_agent::tests::preset_without_allowed_models_explains_policy`
- [x] T2 — `[agents] builtin_opencode`, PATH/`DMUXAI_OPENCODE_BIN` detection,
  preset registration, `allowed_runners = ["opencode"]` validation.
  Requirements: FR-810, FR-811. Verify:
  `cargo test --locked --lib settings::builtin_opencode_tests::builtin_opencode_preset_registration`
- [x] T3 — Graph routing exception (`graph_runner_allowed`, policy, status).
  Requirements: FR-813. Verify:
  `cargo test --locked --lib settings::builtin_opencode_tests::builtin_preset_is_not_graph_routed_unless_listed`
  and `cargo test --locked --test agent_api_cli fresh_install_lists_opencode`
- [x] T4 — `dmuxai init [--workspace] [--force]` and the starter template.
  Requirements: FR-814. Verify:
  `cargo test --locked --test agent_api_cli init_writes_starter_config`,
  `cargo test --locked --lib settings::builtin_opencode_tests::starter_config_parses`
- [x] T5 — README First run and Linux tarball steps. Requirements: FR-815,
  FR-816. Verify: `cargo test --locked --test readme_install`
- [x] T6 — Gates. Verify: `cargo fmt --check`,
  `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked --all-targets` (see the review for results).
