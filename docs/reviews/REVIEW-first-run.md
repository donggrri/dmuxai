# REVIEW: first-run

Spec: [SPEC-first-run](../specs/SPEC-first-run.md)
Plan: [PLAN-first-run](../plans/PLAN-first-run.md)

## Spec compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-810 | Met | `builtin_opencode_preset_registration` (preset from the binary; absent when not installed); `fresh_install_lists_opencode` (status lists `opencode` from PATH with no config, and not when PATH has none) |
| FR-811 | Met | `builtin_opencode_preset_registration` (user `opencode` entry wins; `builtin_opencode = false` hides it; missing binary gives no preset and no warning) |
| FR-812 | Met | `preset_without_allowed_models_explains_policy` (no match, non-zero exit, and timeout each name `allowed_models` and `dmuxai init`; a valid listing keeps only allowed ids). Default model is the first allowed listed model (`ensure_builtin_model`, `ModelsListed.selected`) |
| FR-813 | Met | `builtin_preset_is_not_graph_routed_unless_listed`, `builtin_opencode_is_not_graph_routed_unless_listed` (routing default falls back to cursor), `fresh_install_lists_opencode` (status `allowed` false, `allowed_runners` null). Listing is lazy and bounded by a 10 s timeout |
| FR-814 | Met | `init_writes_starter_config` (writes, refuses without `--force`, `--force` overwrites, `--workspace` writes the workspace file, the starter parses with no warnings); `starter_config_parses`; `opencode_example_equals_builtin_preset` |
| FR-815 | Met | `readme_matches_release_layout` (`## First run` present, names `dmuxai init`) |
| FR-816 | Met | `readme_matches_release_layout` (archive directory taken from `release.yml`; `mkdir -p ~/.local/bin`, `install`, `tar --strip-components=1`, PATH note, `dmuxai --version`) |

## Deviations and decisions

- The dashboard and TUI "allowed" flag (`runner_is_allowed` in `app.rs`) keeps
  the old rule. It also disables selection in the dashboard, so applying the
  graph rule there would block manual use of the preset. FR-813 names
  `src/app.rs` graph runner selection; graph selection is covered through
  `assign_runner` and `/graph override`, and the app's own graph-run checks
  only restrict when `allowed_runners` is set.
- `opencode` is launched by the resolved absolute path, not the bare name.
- `DMUXAI_OPENCODE_BIN` set to a non-file disables the preset. It does not fall
  back to `PATH`. This is intentional so an explicit override is never ignored.
- `[agents]` without `[[agents.custom]]` no longer clears other configs'
  custom agents. This is a small fix that the new `[agents]` key needed.

## Test evidence

Environment: Linux, `CARGO_TARGET_DIR=/workspace/dmux-wt/agent-api/target-me`.

- `cargo fmt --check`: clean.
- `cargo clippy --locked --all-targets -- -D warnings`: clean.
- `cargo test --locked --all-targets`: 538 passed, 0 failed, 9 ignored
  (ignored tests are pre-existing live or browser checks).
- Not run: `powershell ... scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-first-run.md`
  (`pwsh` is not installed here; the only spec change is the Status line).
- Not run: Windows tests and live opencode smoke tests. The shell-script tests
  are `#[cfg(unix)]`. The Windows `.cmd` path in `find_on_path` is untested.

## Residual risks

- The built-in's first send blocks the custom-agent runtime loop for up to
  10 s while models are listed. Cancel and model commands queue behind it.
- Existing v0.2.0 users with opencode on `PATH` get a new runner, as stated in
  the CHANGELOG.

## Cross-review: AGY (gemini-3.8-flash-high): APPROVE

No blockers; FR-810..FR-816 and TC-810..TC-815 are satisfied. The `AgentsToml.custom` → `Option` change is intended: a workspace `[agents] builtin_opencode = false` must not wipe the user's custom agents.

| Nit | Resolution |
| --- | --- |
| A custom agent named `OpenCode` in different case did not suppress the preset | `eq_ignore_ascii_case` |
| Windows PATH lookup missed `.bat`; Unix lookup ignored the execute bit | `.bat` added; Unix requires an execute bit |
| `opencode models failed:` with empty stderr | Now reports the exit status |
| An empty-stderr turn failure had no hint | Message names the program and suggests running it directly (found in a manual fresh-HOME run) |
| Graph-mode inline check vs `is_runner_allowed` | Kept: `assign_runner` already excludes the preset; noted |
| No App-level test for the policy error | Covered by the unit test plus a manual run; noted |

Maintainer evidence, on rebased `main`, with its own target dir: fmt and clippy `-D warnings` are clean; `cargo test --locked --all-targets` gives 541 passed, 0 failed, 9 ignored.

Manual check with an empty HOME and opencode 1.17.20 on PATH:
- `status --json` lists `opencode` (graph-allowed `false`);
- `dmuxai init` writes the file and refuses to run again without `--force` (exit 1).
