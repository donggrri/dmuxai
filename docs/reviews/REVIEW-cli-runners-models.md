# REVIEW: cli-runners-models

Status: complete
Spec: [SPEC-cli-runners-models.md](../specs/SPEC-cli-runners-models.md)
Plan: [PLAN-cli-runners-models.md](../plans/PLAN-cli-runners-models.md)
Tasks: [TASKS-cli-runners-models.md](../tasks/TASKS-cli-runners-models.md)

## Scope and compliance (`FR-2830..FR-2838`)

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-2830 | Met | `tc_2830_subcommand_help_is_local` (`src/main.rs`), `tc_2830_help_lists_parser_flags` (`src/cli.rs`) |
| FR-2831 | Met | `tc_2831_runners_json_fields_and_admission` (`src/agent_api.rs`), `tc_2831_runners_spawns_nothing` (`tests/agent_api_cli.rs`) |
| FR-2832 | Met | `tc_2832_parse_runners_and_models` (`src/cli.rs`), `tc_2832_models_unknown_agent_exits_3` (`tests/agent_api_cli.rs`), `tc_2833_models_json_policy_and_errors`, `tc_2833_models_json_custom_allowed_models`, `tc_2833_models_listing_keeps_disallowed_ids` (`src/agent_api.rs`), `tc_2838_mcp_models_tool_dispatch` (`src/mcp_server.rs`) |
| FR-2833 | Met | `tc_2834_parse_agy_models_output` (`src/agent_api.rs`) |
| FR-2834 | Met | `tc_2835_agy_accepts_gemini_4_hydrogen` (`src/model_policy.rs`) |
| FR-2835 | Met | `tc_2836_agy_rejection_message_reflects_policy` (`src/model_policy.rs`) |
| FR-2836 | Met | `tc_2837_login_failure_is_auth` (`src/agent/mod.rs`), `tc_2837_acp_error_mapping_is_auth` (`src/agent/acp_runner.rs`), `tc_2837_cursor_acp_login_emits_single_auth_error` (`src/agent/cursor.rs`) |
| FR-2837 | Met | `tc_2838_mcp_documents_auth_and_models_tool` (`src/mcp_server.rs`), `docs/API.md`, `CHANGELOG.md` |
| FR-2838 | Met | `tc_2839_status_allowed_matches_headless_run` (`src/agent_api.rs`), `tc_2839_status_allowed_matches_headless_run_cli` (`tests/agent_api_cli.rs`) |

## Reviewers, findings, and resolutions

Reviews ran across three passes: **opencode free plan review x2 + agy review** (prior fixes landed in `65e203f` and `76599df`, plus the final review fixes below).

- **Custom-agent `models_json` verdict bypassed global empty/`fast` rules** (`src/agent_api.rs`): Updated `models_json` to reject empty and case-insensitive `fast` ids before calling `validate_custom_model`, and extended `tc_2833_models_json_custom_allowed_models` with `x/a-fast` (`false`) and an unrestricted custom agent (`foo` `true`, `foo-fast` `false`).
- **`dmuxai models` dropped policy-disallowed ids for built-in presets** (`src/custom_agent.rs`, `src/agent_api.rs`): Added `list_listed_model_ids_with_env` / `list_listed_model_ids` (keeps `models_filter`, skips policy filtering; `grok` retains `list_grok_model_ids` incl. login detection) and wired `collect_model_listing` to it, leaving `list_model_ids` unchanged for `run`/fallback/status. Added `tc_2833_models_listing_keeps_disallowed_ids`.
- **`SPEC-grok-runner.md` FR-896 overstated `auth` error kind for spawn/timeout** (`docs/specs/SPEC-grok-runner.md`): Aligned FR-896 with code—only not-authenticated listings produce `grok_login_message()` and `error_kind: "auth"`; spawn failure and 10 s timeout keep the underlying error (`model_unavailable` via `builtin_probe_error_kind`; `error_kind: null` in `dmuxai models`).
- **Cursor managed ACP double `AgentEvent::Error` and lowercase `"cursor"` hint** (`src/agent/acp/client.rs`, `src/agent/acp_runner.rs`, `src/agent/cursor.rs`, `src/agent/custom.rs`, `tests/fixtures/fake_acp_agent.rs`): Changed `AcpRunnerKind::Cursor` label to `"Cursor"`, tracked managed ACP error emission via `error_emitted` so `spawn_turn` does not emit a duplicate error event, and added `tc_2837_cursor_acp_login_emits_single_auth_error`.
- **Bare `"gemini-4-hydrogen"` coverage** (`src/model_policy.rs`): Added `"gemini-4-hydrogen"` assertion to `tc_2835_agy_accepts_gemini_4_hydrogen`.
- **Earlier review fixes (`65e203f`, `76599df`)**: Classified ACP login failures as `Auth` on grok/cursor paths, fixed codex rejection message, added `kill_on_drop` to `agy models`, listed parser flags in subcommand help, and set `error_kind` on grok model listings.

## Residual risks

- Deterministic tests mock `agy`, `grok`, `opencode`, and `cursor-agent` (`tests/fixtures/fake_acp_agent.rs` and temp shell scripts); live CLI stdout/error formats are not exercised in CI.
- `dmuxai models` invokes each runner's model command sequentially with a 10 s timeout, so slow or hung CLIs delay multi-runner output up to 10 s each.
- `status --json` `allowed` changed from graph admission to headless `run` admission (`graph_allowed` carries graph admission); callers relying on the old meaning must read `graph_allowed` (noted in `CHANGELOG.md` and `docs/API.md`).
- Full `cargo test --all-targets` and `gitleaks` were not run in this verification pass; see commands below for what ran.

## Test evidence

- `cargo fmt` (with `--check` clean)
- `cargo test --locked tc_283`: 19 passed (10 lib, 6 bin, 3 `agent_api_cli`), 0 failed
- `cargo test --locked tc_89`: 10 passed (6 lib, 3 bin, 1 `agent_api_cli`), 0 failed
- `cargo test --locked --test agent_api_cli`: 24 passed, 0 failed
- `cargo clippy --all-targets --all-features -- -D warnings`: clean (0 warnings)
- `python3 scripts/check-spec.py --all`: 99 specs, 99 pass, 0 fail, 0 allowlisted, 0 stale
- `python3 scripts/check-tc-links.py --base origin/main`: 1150 defined, 1010 strong, 0 orphan, 0 undefined-test, 0 stale
