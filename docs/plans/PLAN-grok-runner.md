# PLAN: Built-in grok runner

Status: draft
Spec: [SPEC-grok-runner](../specs/SPEC-grok-runner.md)
Model policy: [SPEC-model-policy](../specs/SPEC-model-policy.md) FR-910..FR-912
Tasks: [TASKS-grok-runner](../tasks/TASKS-grok-runner.md)

Phase 1 (this change) is the spec, the model-policy rows, this plan, and the task list. Phase 2 implements the tasks below and does not add behavior past those IDs.

The spec index is generated. `scripts/spec-index.sh` prints it and `tests/spec_index.rs` checks Status lines and ID uniqueness. There is no checked-in index table to edit (SPEC-spec-index FR-472, FR-473).

## Order of work

1. **Fixture and preset** (`src/custom_agent.rs`, `tests/fixtures/fake-grok.sh`). Add `grok_preset(program)` with `builtin: true`, color `#6366f1`, `prompt_via = arg`, `output = jsonl`, `text_path = "/data"`, `text_when` type `text`, `session_id_path = "/sessionId"`, `resume_arg = ["-r", "{session_id}"]`, `timeout_secs = 900`, and mode args from FR-892. Command shape: program, `-p`, `{prompt}`, `-m`, `{model}`, `--cwd`, `{workspace}`, `--output-format`, `streaming-json`. No temp config, no env, no image arg, no shell. Parse `grok models` with the FR-895 line rules (do not reuse `filter_models`, which returns the whole decorated line). Auth is FR-896 / A-3, from stdout plus exit status only. FR-892, FR-893, FR-894, FR-895, FR-896, FR-898, NFR-890. Tests: TC-892, TC-893, TC-895, TC-896, TC-899.

2. **Registration** (`src/settings.rs`). Add `[agents] builtin_grok` with the same `Option<bool>` merge as `builtin_opencode`. Add `find_grok_bin`: `DMUXAI_GROK_BIN`, then `find_on_path("grok")`, then the executable at `~/.grok/bin/grok` (and `grok.exe` on Windows). Register `grok_preset` only when the flag is not false and no config names `grok`. Append it after the opencode preset so existing indexes stay put. A missing authoritative `DMUXAI_GROK_BIN` does not fall through. FR-890, FR-891. Test: TC-890.

3. **Runtime** (`src/agent/custom.rs`). Reuse `build_custom_command` / `parse_jsonl_line` / `kill_group`. Do not add a grok killer and do not change `kill -TERM -- -<pgid>` (A-2). The first send and the models probe must run the auth check even if a default model string is already set: `ensure_builtin_model` returns early today when `selected` is `Some`, and `list_models` inserts `default_model` when it is missing. Neither behavior may send `-m grok-4.7` when that id was filtered out, nor skip the login message. Prefer `grok-4.7` only when the allowed list contains it. Login and empty-catalog failures return before `grok -p`. FR-893, FR-894, FR-895, FR-896, FR-897. Tests: TC-894, TC-897, TC-898. TC-907 is `#[ignore]`.

4. **Model policy** (`src/model_policy.rs`). At the start of `validate_model_for_runner`, reject any trimmed id whose lowercase form contains `fast`, using the existing fast-policy error. Then accept `(?i)^grok-[a-z0-9][a-z0-9._-]*$` only for runner `grok`, and reject those ids for every other runner before `allowed_models` is consulted. Reject `composer-2.5` for grok when the registry is empty. Leave cursor on `composer-2.5` only. `flash` stays allowed for agy. FR-910, FR-911. Tests: TC-910, TC-911.

5. **Graph and UI** (`src/graph/policy.rs`, `src/app.rs`). `graph_runner_allowed` already excludes every `builtin` spec; cover grok with the same rule and extend the `/graph` status string so an unset list names each excluded builtin (`grok`, and `opencode` when that preset is registered). Effective model for grok follows the non-Cursor rule: F2 wins, and a planned `composer-2.5` is not forwarded. TUI `/agent` confirm and dashboard `SelectAgent` call `request_models` on grok so the login error from step 3 is shown. Chat selection does not use the graph exclusion. Dashboard and TUI colors come from the preset `#6366f1`, overridable by `[theme.agents] grok`. FR-896, FR-898, FR-899. Tests: TC-900, TC-901, TC-902, TC-903, TC-904.

6. **Status and docs** (`tests/agent_api_cli.rs`, `src/init.rs`, `README.md`, `CHANGELOG.md`, `docs/agents/custom-agents.md`, `docs/dmuxai-graph.yaml`). Status lists grok and reports `allowed: false` until it is in `allowed_runners`. Init comments `builtin_grok` and, when grok is detected, a commented custom example equal to the preset. README First run and the CHANGELOG state detection, the model rule, `grok login`, and the graph exception. `custom-agents.md` says the built-in path is `grok -p`, not `grok agent stdio`. Yaml: add the FR-912 comment above `disallow_patterns` and leave every node on `composer-2.5`. FR-890, FR-899, FR-900, FR-912. Tests: TC-891, TC-905, TC-906, TC-912.

## Risks

- Setting `default_model = Some("grok-4.7")` would skip today's builtin listing on send and would also be inserted when the listing omitted it. The grok path has to probe auth and must not invent a missing id.
- `filter_models` cannot turn `* grok-4.7 (default)` into `grok-4.7`. A grok-specific parser keeps the opencode line parser unchanged.
- The regex crate has no lookahead, so the preset `allowed_models` pattern cannot express "without `fast`". FR-910 is the fast gate and must run before custom `allowed_models`.
- `graph_runner_allowed` treats every builtin as excluded. A user `[[agents.custom]]` named `grok` is not builtin and stays in the default graph set. Tests need both shapes.
- Appending the preset keeps existing custom indexes stable. Inserting it earlier would renumber `Agent::Custom` values.
- v0.2.x users with `grok` on `PATH` and no custom `grok` will see a new chat runner. The CHANGELOG says it takes no graph nodes until listed.
- The shared kill path stays SIGTERM. Changing it to SIGKILL would affect opencode and every other custom agent.

## Rollback

Remove the `builtin_grok` registration block and the `grok_preset` call. Config keys are additive. No schema migration. Revert the `validate_model_for_runner` prefix checks to restore the previous runner match. The yaml comment can stay or go; node models are unchanged either way.
