# SPEC: first-run

Status: Draft (v0.2.1)
Created: 2026-10-17
Related: Dongjin dogfood 2026-10-17 23:43 (fresh install: no `~/.config/dmuxai/`, opencode 1.17.20 at `~/.opencode/bin/opencode`, cwd `~/tangled-paws`)

## 1. Decision summary

On a fresh install (no user or workspace `dmuxai.toml`), v0.2.0 shows only the built-in Cursor, Codex and AGY runners. opencode is reachable only as a `[[agents.custom]]` entry, so it never appears, even when `opencode` is on `PATH`. Reproduced with an empty `HOME`: the agent list has no opencode.

Decision:

1. Ship a **built-in opencode preset**. This changes behaviour for v0.2.0 users who have `opencode` on `PATH` and no `opencode` custom agent: a new `opencode` runner appears in chat. It is **not** added to graph routing unless listed in `[graph] allowed_runners`. The CHANGELOG states this. When no custom agent named `opencode` is configured and `opencode` is found on `PATH`, register it automatically. It uses the same command, JSON parsing and permission template as the documented custom agent, plus the existing free-model policy (`^opencode/.*-free$`).
2. Add `dmuxai init`. It writes a commented starter config listing the runners it detected.
3. Document first-run setup and the release tarball layout in the README.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-810 | Must | Given no `[[agents.custom]]` entry named `opencode`, `[agents] builtin_opencode` not `false`, and an `opencode` executable on `PATH` (or `DMUXAI_OPENCODE_BIN`), Then the agent registry contains an `opencode` runner built from the preset. It is listed in the TUI, the dashboard, `dmuxai status --json` and the MCP agent list | TC-810, TC-811 |
| FR-811 | Must | A user or workspace `[[agents.custom]]` named `opencode` replaces the preset, so existing configs keep their behaviour. `builtin_opencode = false` hides the preset. When the opencode executable is missing, no preset is registered and no error is shown | TC-810 |
| FR-812 | Must | The preset uses `models_command = ["opencode","models"]`, `models_filter = "-free$"`, `allowed_models = "^opencode/.*-free$"`, and `default_model` = the first allowed model in the listing. When model listing fails or returns no allowed model, the runner still appears. Selecting it or sending a prompt shows a clear message naming the policy and how to override it (`dmuxai init` / `allowed_models`), instead of failing silently | TC-812 |
| FR-813 | Must | Exception to SPEC-graph-allowed-runners (unset `allowed_runners` = every runner): the built-in preset is excluded from the graph runner set unless it is named in `[graph] allowed_runners`. Applies where the allowed set is resolved (`src/graph/policy.rs`, `src/app.rs` graph runner selection, `src/agent_api.rs` status). User-configured custom agents keep the old rule. The CHANGELOG states the exception. Rationale: free opencode models hit quota often, and a fresh install should not silently route graph nodes to them. Preset model listing runs with a timeout (10 s, a parameter so tests can shorten it). On timeout it behaves as a listing failure (FR-812) and never blocks startup (listing is lazy, as for custom agents) | TC-811, TC-812 |
| FR-814 | Must | `dmuxai init [--workspace] [--force]` writes a starter `dmuxai.toml` to the user config dir, or to the workspace with `--workspace`. It holds commented sections for theme, routing, graph, cursor and `[agents]` (including `builtin_opencode`). When opencode is detected, it also holds a commented `[[agents.custom]]` opencode example equal to the preset. It refuses to overwrite an existing file without `--force`, then prints the path and the detected runners | TC-813, TC-814 |
| FR-815 | Must | The README has a "First run" section (no config needed; what `dmuxai init` does; how opencode and its free-model policy are detected) | TC-815 |
| FR-816 | Must | README install steps match the release archive layout: `dmuxai-linux-x86_64.tar.gz` extracts to `dmuxai-linux-x86_64/{dmuxai,README.md,CHANGELOG.md}`. The archive keeps its top directory (it holds README and CHANGELOG; a flat archive would scatter files into the cwd). The README shows `mkdir -p ~/.local/bin`, the install command, a one-line `tar --strip-components=1` alternative and a `PATH` note | TC-815 |

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-810 | FR-810, FR-811 | unit | temp dir holding a fake `opencode` script; settings with and without a custom `opencode`; `builtin_opencode = false` | Preset present only in the first case; a user entry wins; disabled or missing binary gives no preset | `src/settings.rs` `builtin_opencode_preset_registration` |
| TC-811 | FR-810, FR-813 | integration | empty HOME, fake opencode on PATH | `dmuxai status --json` lists `opencode`; the graph runner set without `allowed_runners` does not include it | `tests/agent_api_cli.rs` `fresh_install_lists_opencode` |
| TC-812 | FR-812 | unit | fake `opencode models` printing no `-free` model, exiting non-zero, or sleeping past the timeout (test timeout shortened) | Runner listed; the model catalog error message names `allowed_models` and `dmuxai init` | `src/custom_agent.rs` `preset_without_allowed_models_explains_policy` |
| TC-813 | FR-814 | integration | empty HOME | `dmuxai init` writes the file, parses with `Settings`, and refuses a second run without `--force` | `tests/agent_api_cli.rs` `init_writes_starter_config` |
| TC-814 | FR-814 | unit | — | The starter template parses into `Settings` equal to `Settings::default()` (everything is commented) and contains the keys `[theme]`, `[routing]`, `[graph]`, `[cursor]`, `[agents]`, `builtin_opencode` | `src/settings.rs` `starter_config_parses` |
| TC-815 | FR-815, FR-816 | unit (doc) | README.md, release.yml | README names `dmuxai-linux-x86_64/dmuxai`, the archive dir in release.yml matches; README has `mkdir -p ~/.local/bin`, `--strip-components=1`, a PATH note and a "First run" heading | `tests/readme_install.rs` `readme_matches_release_layout` |
