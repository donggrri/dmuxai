# REVIEW: grok runner

Spec: [SPEC-grok-runner](../specs/SPEC-grok-runner.md)
Plan: [PLAN-grok-runner](../plans/PLAN-grok-runner.md)
Model policy: [SPEC-model-policy](../specs/SPEC-model-policy.md) FR-910..FR-912

## Spec compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-890 | Met | `builtin_grok_preset_registration` (PATH, `DMUXAI_GROK_BIN`, `~/.grok/bin/grok`, append after opencode); `fresh_install_lists_grok` (`status --json` id `grok`, `allowed` false; empty PATH and a home without the binary drops it) |
| FR-891 | Met | `builtin_grok_preset_registration` (custom name `grok` replaces the preset; `builtin_grok = false` and a missing binary register nothing and add no error; workspace flag overrides the user file; a flag-only file does not clear the other file's custom agents) |
| FR-892 | Met | `grok_preset_argv_is_headless_and_not_a_shell` (Ask/Plan `--permission-mode plan`, Implement `--always-approve`, prompt is one argv element, no shell); `grok_mock_turn_returns_text_and_reports_errors` |
| FR-893 | Met | `grok_streaming_json_extracts_text_and_session`; `grok_mock_turn_returns_text_and_reports_errors` (text, stderr failure, empty reply) |
| FR-894 | Met | `grok_preset_argv_is_headless_and_not_a_shell` (resume `-r`); mock second turn passes the stored `sessionId` |
| FR-895 | Met | `grok_models_parse_default_filter_and_fast` (marker, `(default)`, `fast` dropped, `flash` kept, no invented id); mock turn sends `-m grok-4.7` and never `grok-4.7-build-fast` |
| FR-896 | Met | `grok_auth_is_models_stdout_only` (stdout plus exit status; message contains `grok login`); `grok_unauthenticated_send_explains_login` (no `-p`); `selecting_grok_requests_a_model_probe` |
| FR-897 | Met | `grok_cancel_kills_the_process_group` (existing `kill -TERM -- -<pgid>`; leader and grandchild stop). No grok-specific killer |
| FR-898 | Met | `grok_preset_color_is_distinct`; `grok_chat_and_dashboard_use_preset_color` (TUI `Rgb(0x63, 0x66, 0xf1)`, `AgentView.color` `#6366f1`, theme override `#112233` on both) |
| FR-899 | Met | `builtin_grok_is_not_graph_routed_unless_listed`; `graph_status_names_builtin_grok_exception`; `grok_effective_model_skips_composer_default`; `fresh_install_lists_grok`; `selecting_grok_requests_a_model_probe` (chat select succeeds while graph-allowed is false) |
| FR-900 | Met | `readme_and_changelog_mention_builtin_grok`; `starter_config_mentions_builtin_grok` (commented `builtin_grok`, detected example equals the preset with `builtin` false, `grok agent stdio` is not the built-in path) |
| NFR-890 | Met | Argv tests and `grok_auth_is_models_stdout_only`. The mock fixture is the only grok process the deterministic tests spawn. No test reads an auth file |
| FR-910 | Met | `fast_models_are_rejected_for_every_runner` |
| FR-911 | Met | `grok_models_are_allowed_only_for_the_grok_runner` (including a custom runner whose `allowed_models` is `.*`, and `composer-2.5` rejected for grok with an empty registry before that install) |
| FR-912 | Met | `dmuxai_graph_yaml_keeps_grok_disallow_and_explains_why` (comment sits immediately above `disallow_patterns`, `*grok*` and `*-fast` stay, no `model: grok`) |

## Deviations and decisions

- Chat and dashboard selection keep `runner_is_allowed`. An unset `allowed_runners` list still admits every chat runner, so selecting grok works while `is_runner_allowed("grok")` is false. Graph routing, `/graph override`, and `status --json` use `graph_runner_allowed`, which excludes builtin presets until they are listed. This matches the opencode preset.
- A non-empty `DMUXAI_GROK_BIN` that is not an existing file does not fall through to `PATH` or `~/.grok/bin/grok`.
- `fast` is rejected in `validate_model_for_runner` before `allowed_models`. The preset pattern cannot express "without `fast`" because the regex crate has no lookahead. Grok `SelectModel` goes through that policy so `grok-4.7-build-fast` is not accepted.
- Auth is exit status and a case-insensitive `logged in` substring. A non-zero exit whose stderr says `not logged in` stays unauthenticated. The probe always runs, even when a model string is already filled in.
- The dashboard chip dot already paints `agent.color`. `src/web/assets/app.js` is unchanged.
- `load_with_opencode` stays the deterministic opencode test entry (grok binary `None`). Production `load_with_exe` looks up both binaries. The wrapper is unused in non-test builds and allows `dead_code` there.

## Test evidence

Environment: Linux, `CARGO_TARGET_DIR=/workspace/dmux-wt/grok-runner-target`. `~/.grok/bin/grok` exists and is executable; `grok` is not on `PATH`. Tests did not run that binary and did not read `~/.grok/auth.json`.

- `cargo fmt --all` and `cargo fmt --all -- --check`: clean.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: clean.
- `cargo test --locked --all-targets`: 597 passed, 0 failed, 17 ignored.
- `grok_live_smoke_headless_turn` is ignored (`live grok smoke; requires a logged-in grok binary`). It was not run.
- `cargo test --locked --test spec_index --test model_policy_repo` was run again after the Status edits: 7 passed, 0 failed (`dmuxai_graph_yaml_keeps_grok_disallow_and_explains_why` included).
- Not run: `powershell ... scripts/check-spec.ps1` (`pwsh` is not installed; spec edits are the Status line, grok-row wording, and section 8 checkboxes).
- `cargo check --locked --all-targets --target x86_64-pc-windows-gnu`: clean. Windows tests were not executed. Unix-only fixtures and the process-group cancel test stay behind `#[cfg(unix)]`.

## Residual risks

- The first grok send blocks the custom-agent runtime loop for up to 10 s while `grok models` runs. Cancel and other commands queue behind that probe.
- `selecting_grok_requests_a_model_probe` installs the grok preset into the process-wide custom-agent registry. Built-in Cursor, Agy, and Codex indexes stay stable because the preset is appended. A later test that indexes past those three agents can see the extra entry.
- The ignored live test reuses a 5 s wait. A hand run with `--ignored` can time out on a slow grok turn even when login works.
- The Windows `grok.exe` lookup uses the same PATH helper as opencode. The cross compile succeeded; those tests were not run on Windows.
