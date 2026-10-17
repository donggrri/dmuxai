# PLAN: first-run

Spec: [SPEC-first-run](../specs/SPEC-first-run.md)
Tasks: [TASKS-first-run](../tasks/TASKS-first-run.md)

## Order of work

1. `custom_agent.rs`: `builtin` flag (`serde(skip)`), `opencode_preset(program)`,
   `list_model_ids` with an optional timeout, and `policy_message` (FR-812).
2. `settings.rs`: `[agents] builtin_opencode`, `find_opencode_bin` / `find_on_path`,
   `load_with_opencode` (injected binary), and the preset decision before
   `allowed_runners` validation (FR-810, FR-811). `graph_runner_allowed` (FR-813).
3. Graph and status call sites: `graph/policy.rs`, `agent_api.rs` (FR-813).
4. `agent/custom.rs`: default model = first allowed listed model; the built-in
   resolves it before the first turn; listing uses the 10 s timeout (FR-812).
5. `init.rs` template and writer, `cli.rs` subcommand, USAGE text (FR-814).
6. README (FR-815, FR-816), CHANGELOG, TEST-PLAN, TASKS-v0.2.1.

## Risks

- Behaviour change: v0.2.0 users with `opencode` on `PATH` and no opencode custom
  agent get a new runner in chat. Stated in the CHANGELOG.
- The preset's program is the resolved absolute path, not the bare name
  `opencode`, so the runner launches the binary that detection found.
- The built-in's first send blocks the custom-agent runtime loop for up to 10 s
  while the model list is fetched. Other commands queue behind it.
- `[agents]` without `[[agents.custom]]` no longer clears custom agents from
  the other config (`Option` instead of an empty default).

## Rollback

Remove the `builtin` registration in `load_with_opencode` (one block) and the
`init` subcommand. The config keys are additive; no persisted data changes.
