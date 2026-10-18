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
| FR-911 | Met | `grok_models_are_allowed_only_for_the_grok_runner` (grok ids are rejected before any custom-agent lookup, so an unknown or custom runner never accepts them; the test no longer mutates the global registry) |
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
- `cargo test --locked --all-targets` (after merging main with #65 and the review fixes): 600 passed, 0 failed, 21 ignored. `cargo test --bin dmuxai -- --test-threads=1`: 242 passed.
- `cargo clippy --locked --all-targets --all-features --target x86_64-pc-windows-gnu -- -D warnings` (MinGW installed): clean.
- `grok_live_smoke_headless_turn` is ignored (`live grok smoke; requires a logged-in grok binary`). It was not run.
- `cargo test --locked --test spec_index --test model_policy_repo` was run again after the Status edits: 7 passed, 0 failed (`dmuxai_graph_yaml_keeps_grok_disallow_and_explains_why` included).
- Not run: `powershell ... scripts/check-spec.ps1` (`pwsh` is not installed; spec edits are the Status line, grok-row wording, and section 8 checkboxes).
- `cargo check --locked --all-targets --target x86_64-pc-windows-gnu`: clean. Windows tests were not executed. Unix-only fixtures stay behind `#[cfg(unix)]`; the process-group cancel test and its `/proc` helpers are `#[cfg(target_os = "linux")]`.

## Residual risks

- The first grok send blocks the custom-agent runtime loop for up to 10 s while `grok models` runs. Cancel and other commands queue behind that probe.
- `selecting_grok_requests_a_model_probe` installs the grok preset into the process-wide custom-agent registry (`OnceLock`, first call wins). Built-in Cursor, Agy, and Codex indexes stay stable because the preset is appended. A later test that indexes past those three agents can see the extra entry.
- The ignored live test reuses a 5 s wait. A hand run with `--ignored` can time out on a slow grok turn even when login works.
- The Windows `grok.exe` lookup uses the same PATH helper as opencode. The cross compile succeeded; those tests were not run on Windows.
- Text from separate grok assistant messages in one turn (before and after a tool call) is concatenated with no separator (FR-893 as specified), e.g. `...one sentence.The README says...`. A follow-up could insert a paragraph break between assistant messages.

## Cross-review (opencode free models)

`opencode/deepseek-v4.1-flash-free` timed out (10 min, no output) and `opencode/glm-5.3-flash-free` did not answer a one-word ping within 90 s; `beam-free` was refused by OpenCode (IP flagged). The review ran on `opencode/nemotron-3-ultra-free` (primary) and `opencode/space-bunny-free` (second opinion), both on the full `src`/`tests` diff plus this spec. No paid or haiku model was used.

| Finding | Source | Disposition |
| --- | --- | --- |
| Windows compile break from `PermissionsExt` / `/proc` in tests | both | Not reproducible: the test module is `#[cfg(all(test, unix))]` and Windows-target clippy is clean. `/proc` helpers and the cancel test are now `#[cfg(target_os = "linux")]` |
| Global registry mutation in parallel tests | both | Fixed in `model_policy.rs` (no install). The app test keeps one install; documented as residual risk; full and single-threaded runs pass |
| `fake_grok()` chmods the checked-in fixture | space-bunny | Fixed: fixture is committed `100755`; tests no longer rewrite it |
| `self.adapters[adapter_index(agent)]` may panic | space-bunny | Fixed: `adapters.get(...)` |
| `grok_effective_model_skips_composer_default` was vacuous | space-bunny | Fixed: test installs the grok preset in settings first |
| Timing-based cancel test, 5 s deadlines, models env substitution, fixture path | nemotron | Low; kept (2 s/5 s deadlines pass under load; preset env is empty) |

## End-to-end smoke (real grok-4.7 through dmuxai)

2026-10-18 ~23:56 KST, debug build of this branch, `dmuxai --web` in tmux, scratch worktree of origin/main, isolated `XDG_CONFIG_HOME`/`DMUXAI_STATE_DIR`, `DMUXAI_GROK_BIN` pointing at Grok Build 1.0.62 (logged in).

- `/api/state` agents: cursor, agy, codex, grok (`#6366f1`). TUI `/agent` lists `grok`; F2 lists `grok-4.7`, `grok-4.6`, `grok-4.5` (no `grok-4.7-build-fast`); default `grok-4.7`.
- TUI Ask prompt: grok listed the repo's top-level files and ended with the marker; turn done in 8 s.
- Dashboard (`POST /api/prompt`): argv was `grok -p <prompt> -m grok-4.7 --cwd <worktree> --output-format streaming-json --permission-mode plan -r <sessionId>` (resume works). The transcript grew while running (streaming), then `cancel` stopped it after ~3 s: run state `cancelled`, no `grok -p` process left after 1 s.
- Dashboard textarea prompt in a new conversation: streamed, completed in 7 s, scratch worktree unchanged (`git status` clean).
