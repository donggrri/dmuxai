# SPEC-env-config-isolation

Status: Implemented

## Decision summary
Settings loading no longer reads `std::env` directly: callers pass an explicit
`RuntimeEnv`, so tests and presets are isolated from the process environment.
A global `--config PATH` flag (and `DMUXAI_CONFIG`) selects the user config file
without depending on `XDG_CONFIG_HOME`, which previously broke `gh`/`git`
credentials in agent child processes. Every agent, runner, MCP server and notify
child process has DmuxAI-internal variables stripped while `XDG_*` and `HOME`
stay untouched.

## Scope
In scope: `RuntimeEnv` and `DmuxSettings::load_with_env` / `_from` / `_with_env`
helpers; the global `--config` flag and `DMUXAI_CONFIG`; `process::INTERNAL_ENV_VARS`
and `apply_child_env_policy[_tokio]`; one shared test env lock
`state_path::test_env_lock`.
Out of scope: redesigning configuration schema or storage layout; changing
provider protocols.

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-1673 | Must | Settings loading takes an explicit `RuntimeEnv`; presets and starter config ignore the process environment when one is given. |
| FR-1674 | Must | One shared test environment lock serializes every test that mutates process environment variables. |
| FR-1675 | Must | `--config PATH` and `DMUXAI_CONFIG` select the config file without `XDG_CONFIG_HOME`; precedence is `--config` > `DMUXAI_CONFIG` > `XDG_CONFIG_HOME` > `HOME`. |
| FR-1676 | Must | An explicit config path that does not exist exits with status 2 and prints `config file not found: <path>`. |
| FR-1677 | Must | Every agent, runner, MCP server and notify child process has `INTERNAL_ENV_VARS` (`DMUXAI_WEB_TOKEN`, `DMUXAI_MCP_CONFIG`, `DMUXAI_CONFIG`) removed; a per-run `DMUXAI_MCP_CONFIG` set after the policy and a custom agent's `env` table still win; `XDG_*` and `HOME` are never changed. |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-1673 | FR-1673 | `tc_1673_presets_ignore_process_bin_env` |
| TC-1674 | FR-1674 | `tc_1674_single_shared_test_env_lock` |
| TC-1675 | FR-1675 | `tc_1675_config_flag_and_env_select_config_file` |
| TC-1676 | FR-1676 | `tc_1676_missing_config_file_exits_2` |
| TC-1677 | FR-1677 | `tc_1677_child_env_policy_snapshot` |

## Non-goals
- No change to config file format or agent runner protocols.
- No removal of `XDG_*` / `HOME` handling for locating default config paths.
