# SPEC: Generic headless custom agent provider (`[[agents.custom]]`)

Status: Implemented (PR #8)  
Owner: DmuxAI  
Created: 2026-10-17  
Baseline: `73e3f4d`  
Related: MISSION §2, TEST-PLAN §2.4

## 1. Decision summary

Any agent CLI with a headless command (opencode, Hermes, pi, ...) can be connected by declaring `[[agents.custom]]` in `dmuxai.toml`. DmuxAI expands a command template, runs it without a shell in its own process group, parses text or JSONL output, applies failure rules, and exposes the agent in `/agent`, F2 and graph nodes.

## 2. Scope

### In scope

- Config: `name`, `command` (template with `{prompt}`, `{model}`, `{workspace}`, `{image}`, `{session_id}`), `prompt_via` (arg/stdin), `image_arg`, `models_command` + `models_filter`, `output` (`text`/`jsonl` with text/session/error JSON paths), `env`, `temp_config_template`, `resume_arg`, per-mode args (ask/plan/implement), `timeout_secs`, failure rules (exit code, stderr regex), `color`, `allowed_models` regex
- Process-group kill on cancel/timeout

### Out of scope / non-goals

- Interactive PTY agents
- Shell-string commands

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| Q-1 | Proposed TCs (marked *proposed*) have no code yet and must be implemented before this spec is Approved. | DmuxAI | Release gate |

## 3. Terms and state model

- Template token: `{name}` placeholder replaced by one argv element.
- Session id: value extracted from output and passed back via `resume_arg`.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-330 | Must | `dmuxai.toml` contains `[[agents.custom]]`. | Valid entries register agents; invalid entries produce a clear config error naming the field. | Duplicate names rejected. | TC-330 |
| FR-331 | Must | A turn is started. | Placeholders expand to single argv elements; `prompt_via=stdin` writes the prompt to stdin; no shell is used. | `{image}` repeats `image_arg` per image; absent images drop the token. | TC-331 |
| FR-332 | Must | Provider output arrives. | `text` appends stdout; `jsonl` extracts text/session/error by configured paths. | Malformed JSONL lines are ignored with a notice. | TC-332 |
| FR-333 | Must | Process exits non-zero, matches a failure regex, or exceeds `timeout_secs`. | Turn is Failed with reason; on timeout/cancel the whole process group is killed. Timeout reason starts with `<label> timed out after <N>s`, names `timeout_secs` and the agent `name`, notes partial workspace edits, and for built-in presets tells the user to copy the full built-in preset into `dmuxai.toml` as `[[agents.custom]]` (using `dmuxai init` for a commented example) and raise `timeout_secs`. | No orphan processes remain. | TC-333, TC-334, TC-338 |
| FR-339 | Must | `timeout_secs` is omitted on a custom agent or built-in preset. | Effective timeout is 1800 seconds. | Explicit `timeout_secs` is honoured. | TC-339 |
| FR-334 | Must | Agent registered. | Appears in `/agent`, F2 (from `models_command` filtered by `models_filter`) and graph nodes; models outside `allowed_models` are rejected. | models_command failure shows an error, not a hang. | TC-335 |
| FR-335 | Should | Second turn in the same conversation. | `resume_arg` with stored session id is passed; per-mode args, `env` and rendered `temp_config_template` are applied. | Temp config is deleted after the turn. | TC-336 |
| FR-1610 | Must | The built-in opencode preset (and the documented example in `README.md`, `docs/agents/presets.toml`, `docs/agents/custom-agents.md`) | The command is `opencode run --format json -m {model} {prompt}` with **no `--dir`**; the workspace is passed only as the child's cwd, which every opencode version honours | opencode builds that reject `--dir` ("Unrecognized flag: --dir in command opencode run") now work | TC-1610, TC-1612 |
| FR-1611 | Must | A user-defined custom agent whose program basename is `opencode` (or `opencode.exe`) and whose command contains the pair `--dir {workspace}` | `build_argv` drops that pair (cwd is already the workspace) so old configs keep working on every opencode version | Other programs, and `--dir` with any value other than `{workspace}`, are left unchanged | TC-1611 |
| UX-330 | Should | Custom agent replies. | Lines use the configured `color`. | — | TC-337 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-330 | FR-330 | unit | valid / invalid toml | load | registered / field error | `test_deserialize_full_spec`, `test_validate_constraints` (src/custom_agent.rs); invalid toml → startup notice |
| TC-331 | FR-331 | unit | prompt with spaces/quotes, 2 images | build argv | exact argv, no shell | `test_build_argv_prompt_injection_prevented`, `substituted_values_are_not_rescanned`, `test_build_argv_images_and_resume` |
| TC-332 | FR-332 | integration | fake script emitting jsonl | run turn | text + session captured | `jsonl_agent_streams_text_and_keeps_prompt_as_one_arg`, `test_parse_jsonl_opencode_sample` (unix; fake agent `tests/fixtures/fake-agent.sh`) |
| TC-333 | FR-333 | integration | fake script exit 1 / stderr match | run turn | Failed with reason | `failures_are_detected`, `test_failure_reason` |
| TC-334 | FR-333 | integration | script spawning child, sleeps | timeout | group killed, no orphans; error mentions `timeout_secs` and partial edits | `timeout_kills_the_process_group` (unix) |
| TC-338 | FR-333 | unit | user and built-in `CustomAgentSpec` | `timeout_message` | prefix, `timeout_secs`, agent name, built-in override hint | `timeout_message_mentions_config_for_user_agent`, `timeout_message_mentions_override_for_builtin_preset` (src/custom_agent.rs) |
| TC-339 | FR-339 | unit | minimal `[[agents.custom]]` without `timeout_secs`; built-in presets | `timeout_secs()` | 1800 | `test_deserialize_minimal_spec`, `builtin_presets_use_default_timeout` (src/custom_agent.rs) |
| TC-335 | FR-334 | unit | one custom agent, models_command fake | open picker | filtered list; disallowed model rejected | `models_are_filtered_and_policy_enforced`; picker listing verified live (TEST-PLAN L-12) |
| TC-336 | FR-335 | unit | stored session id | build second turn | resume arg + env + temp config | `test_build_argv_images_and_resume`, `temp_config_is_written_passed_and_removed` (unix) |
| TC-337 | UX-330 | manual | opencode example config, `opencode/glm-5.3-flash-free` | send prompt in TUI | reply in configured color | TEST-PLAN L-12 |
| TC-1610 | FR-1610 | unit | opencode preset | build argv | no `--dir` element; cwd of the built command is the workspace | `cargo test tc_1610_opencode_preset_has_no_dir_flag` |
| TC-1611 | FR-1611 | unit | custom spec `opencode … --dir {workspace} …`; and a non-opencode program with `--dir {workspace}` | build argv | pair dropped for opencode only | `cargo test tc_1611_opencode_dir_pair_dropped` |
| TC-1612 | FR-1610 | manual | dashboard, runner opencode `opencode/glm-5.3-flash-free` | ask "run pwd" | reply shows the workspace path; no "Unrecognized flag" | dashboard screenshot |

## 6. Data, integration, and safety contracts

- Commands are argv arrays; env values are not logged.
- Temp config files live under the workspace `.dmuxai/` and are removed.

## 7. Implementation plan

See status above; proposed tests are tracked in [TEST-PLAN.md](../TEST-PLAN.md).

## 8. Review and test exit criteria

- All TCs implemented and passing; L-12 passes; status → Implemented.
