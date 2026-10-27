# SPEC-workspace-config-trust

Status: Implemented

Owner: dmuxai
Created: 2026-10-26
Baseline: `55029ac`
Related: #208

## 1. Decision summary

Workspace-local configuration files (`dmuxai.toml` in the repository root) can declare settings that run external commands, read secret environment variables, clone remote repositories, or establish outbound network connections. DmuxAI activates those restricted keys from a workspace configuration only after the user explicitly trusts that exact file content; the user configuration is unaffected.

Trust records are pinned to the SHA-256 hash of the workspace configuration file and persisted in the user's state directory (`trusted-workspaces.json`). The trust hash covers only `dmuxai.toml` itself, not files it references (skills, prompt files); those are confined to the workspace while untrusted and are not hashed. Untrusted workspace configurations have restricted keys stripped during settings loading, with a single summarized warning. User-level configuration remains unchanged and unrestricted. Interactive startup prompts the user to review and trust when run in a TTY, while headless executions never prompt and require either an existing trust record, the `--trust-workspace-config` CLI flag, or the `DMUXAI_TRUST_WORKSPACE_CONFIG=1` environment variable.

## 2. Scope

### In scope

- Content hash calculation (SHA-256 hex digest) of workspace configuration.
- Trust store persistence in the user's state directory with atomic write and 0600 permissions.
- Exact restricted keys identification and stripping when untrusted:
  - `notify.command`
  - `notify.webhook_url`
  - `notify.webhook_secret_env`
  - `notify.secret_header`
  - `agents.custom`
  - `mcp.servers`
  - `skills.sources.<name>` (for git sources or filesystem paths outside workspace root)
  - `memory.vault` (when outside workspace root)
  - `memory.dir` (when absolute path outside workspace root or contains path traversal `..`)
  - `todo.jira_base_url`
  - `runners.<id>.system_prompt_file` (when outside workspace root)
  - `runners.<id>.models.<model>.system_prompt_file` (when outside workspace root)
- Single sanitized warning emitted on untrusted configurations listing only key names.
- Configuration change invalidation: modifying workspace `dmuxai.toml` changes the content hash, requiring renewed trust.
- Interactive startup trust prompt in interactive TTY environments for TUI (`dmuxai`) and dashboard (`dmuxai serve`).
- Dedicated CLI command `dmuxai trust` with `--status`, `--json`, `--revoke`, and `--yes` flags.
- Headless execution opt-ins: `--trust-workspace-config` flag on `dmuxai run` and `dmuxai todo run`, and `DMUXAI_TRUST_WORKSPACE_CONFIG=1` environment variable (in-memory only; does not write to the trust store).
- User-level configuration (`~/.config/dmuxai/dmuxai.toml` or `--config PATH`) remains fully unrestricted.

### Out of scope / non-goals

- Altering user-level configuration loading or security boundaries.
- Prompts or interactive blocking in headless non-TTY mode.
- Persistent trust based purely on repository directory path without file content hash validation.
- Network-based trust verification or signing authorities.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Workspace configuration | The `dmuxai.toml` file located in the target workspace root. | User configuration located in `~/.config/dmuxai/dmuxai.toml` or passed via `--config`. |
| Trust store | JSON document at `<state_dir>/trusted-workspaces.json` mapping normalized workspace paths to trusted SHA-256 digests and timestamps. | Conversation database or cache directory. |
| Restricted keys | Configuration keys capable of invoking external processes, reading sensitive environment variables, or sending network requests. | General settings such as UI themes, model selection, or concurrency limits. |
| One-process opt-in | Explicit trust granted for the active process invocation via `--trust-workspace-config` or `DMUXAI_TRUST_WORKSPACE_CONFIG=1`. | Stored persistent trust written to disk. |

```text
                        ┌──────────────────────────────┐
                        │ No workspace dmuxai.toml     │
                        │ status: no_workspace_config  │
                        └──────────────────────────────┘
                                        │
                         workspace dmuxai.toml exists
                                        │
                                        ▼
                        ┌──────────────────────────────┐
                        │ Has restricted keys?         │
                        └──────────────────────────────┘
                                   │          │
                              No   │          │ Yes
                                   ▼          ▼
┌──────────────────────────────┐       ┌──────────────────────────────┐
│ status: not_needed           │       │ Check trust store / opt-in   │
│ All keys loaded              │       └──────────────────────────────┘
└──────────────────────────────┘                      │
                        ┌─────────────────────────────┴─────────────────────────────┐
                        ▼                                                           ▼
         ┌──────────────────────────────┐                            ┌──────────────────────────────┐
         │ Hash matches or Opt-In       │                            │ Hash mismatch & No opt-in    │
         │ status: trusted              │                            │ status: untrusted            │
         │ (Record or OptIn)            │                            │ Restricted keys stripped     │
         └──────────────────────────────┘                            │ Single warning emitted       │
                                                                     └──────────────────────────────┘
```

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2190 | Must | Given arbitrary raw file content bytes | `content_hash` computes the lowercase hexadecimal SHA-256 digest of the raw bytes | Deterministic 64-character hex string | TC-2190 |
| FR-2191 | Must | Given a `RuntimeEnv` with candidate state directories | `TrustStore::store_path` resolves `trusted-workspaces.json` with precedence: `DMUXAI_STATE_DIR` > `XDG_STATE_HOME/dmuxai` > `HOME/.local/state/dmuxai` | Returns `io::ErrorKind::NotFound` if neither directory is set | TC-2191 |
| FR-2192 | Must | Given an operational `TrustStore` and valid workspace path and SHA-256 | `store.trust()` records normalized path, lowercase SHA-256, and UNIX timestamp in seconds; `store.save()` writes atomically via temporary file replacement; `TrustStore::load()` re-reads matching entry | File errors return `io::Error` | TC-2192 |
| FR-2193 | Must | Given a recorded workspace in `TrustStore` | `store.is_trusted(path, sha256)` returns `true` only when normalized path matches and SHA-256 matches exactly | Returns `false` on modified SHA-256 or different path | TC-2193 |
| FR-2194 | Must | Given a recorded workspace entry in `TrustStore` | `store.revoke(path)` removes entry and returns `true`; subsequent call returns `false` | Nonexistent workspace returns `false` | TC-2194 |
| FR-2195 | Must | Given an invalid or malformed JSON file at `trusted-workspaces.json` | `TrustStore::load()` returns an empty store with a `corrupt_warning` string, treating all workspaces as untrusted without crashing | Warning surfaces in `DmuxSettings::config_warnings` | TC-2195, TC-2206 |
| FR-2196 | Must | On Unix platforms, saving `TrustStore` | Written temporary file and destination `trusted-workspaces.json` have file permission mode `0600` | Created directories inherit standard umask | TC-2196 |
| FR-2197 | Must | Given runtime environment, workspace root, and config hash | `workspace_trust::decide` evaluates: stored record match -> `Trusted(Record)`; `RuntimeEnv::trust_workspace_config` set -> `Trusted(OptIn)` without writing store; otherwise `Untrusted` | Opt-in never writes to disk | TC-2197 |
| FR-2198 | Must | Given untrusted workspace config with `[notify]` keys | Strip `notify.command`, `notify.webhook_url`, `notify.webhook_secret_env`, and `notify.secret_header`; preserve safe keys like `events`; emit exactly one warning listing stripped keys without exposing URLs or environment variable names | Warning contains key names only | TC-2198 |
| FR-2199 | Must | Given untrusted workspace config with `[[agents.custom]]` | Strip `agents.custom` entries; record key in untrusted ignored list and emit trust warning | Custom agent definitions are discarded | TC-2199 |
| FR-2200 | Must | Given untrusted workspace config with `[[mcp.servers]]` | Strip `mcp.servers` entries; record key in untrusted ignored list and emit trust warning | MCP servers are discarded | TC-2200 |
| FR-2201 | Must | Given untrusted workspace config with remote git skills, external memory paths, external runner system prompt files, or Jira base URL | Strip git skills sources, skills path outside workspace, `memory.vault` outside workspace, `memory.dir` absolute outside workspace or containing `..`, `todo.jira_base_url`, and `runners.*.system_prompt_file` outside workspace | In-workspace relative paths remain allowed | TC-2201 |
| FR-2202 | Must | Given user-level configuration containing restricted keys | User config loads without restriction; `workspace_trust` status is `NoWorkspaceConfig` when workspace has no `dmuxai.toml`, and no trust warning is generated | User configuration is always trusted | TC-2202 |
| FR-2203 | Must | Given a workspace trusted by record whose `dmuxai.toml` is modified on disk | Re-evaluating trust detects changed SHA-256 and reverts status to `Untrusted` with the new hash, stripping restricted keys | Trust is invalid as soon as content changes | TC-2203, TC-2211 |
| FR-2204 | Must | Given workspace config with restricted keys when `trust_workspace_config` opt-in is active | Status evaluates to `Trusted(OptIn)`; all restricted keys are retained without warnings | Trust store on disk is not updated | TC-2204, TC-2208, TC-2209 |
| FR-2205 | Must | Given workspace `dmuxai.toml` containing only unrestricted keys (e.g., `[ui]`, `[graph]`) | `workspace_trust` status is `NotNeeded`; all keys load without warnings or trust requirement | No trust prompts or warnings generated | TC-2205 |
| FR-2206 | Must | Given a corrupted trust store file during settings load | Status evaluates to `Untrusted`, restricted keys are stripped, and corruption warning is appended to `config_warnings` | Untrusted safe fallback | TC-2206 |
| FR-2207 | Must | Given headless command `dmuxai run` against untrusted workspace config | Executes in untrusted mode without interactive prompt; restricted keys stripped; runs with exit code 0 if primary task succeeds | Subprocesses or webhooks are not executed | TC-2207 |
| FR-2208 | Must | Given headless command executed with `DMUXAI_TRUST_WORKSPACE_CONFIG=1` (or `"true"` / `"yes"`) | One-process opt-in activates; restricted keys are applied during run; trust store is not modified | Unset or `"0"` remains untrusted | TC-2208 |
| FR-2209 | Must | Given `dmuxai run` or `dmuxai todo run` with `--trust-workspace-config` flag | Enables `RuntimeEnv::trust_workspace_config` for the duration of the command without writing to disk | CLI flag overrides stored trust absence | TC-2209 |
| FR-2210 | Must | Given CLI subcommand `dmuxai trust` | Supports `--status` (with optional `--json`), `--yes`, and `--revoke`; prints status and restricted keys; interactive prompt asks `[y/N]` when run in a TTY; exits 0 on success, 1 on refusal/non-TTY, 6 on I/O failure | Headless non-TTY without `--yes` exits 1 | TC-2210 |
| FR-2211 | Must | Given workspace trusted via `dmuxai trust` subsequently edited | `dmuxai trust --status --json` transitions from `trusted` to `untrusted` with updated SHA-256 | Automatic revocation on content change | TC-2211 |
| FR-2212 | Must | Given trusted workspace when `dmuxai trust --revoke` is executed | Workspace entry removed from store; returns exit 0 and status `revoked`; subsequent `--status` reports `untrusted` | Revocation persists in store | TC-2212 |
| FR-2213 | Must | CLI help and usage string output | Lists `dmuxai trust [--status] [--json] [--revoke] [--yes]` and `--trust-workspace-config` on `run` and `todo run` | Exactly one usage entry per command variant | TC-2213 |
| FR-2214 | Must | Given workspace `[notify]` webhook with `webhook_secret_env` | Untrusted run delivers no webhook connection; run with `DMUXAI_TRUST_WORKSPACE_CONFIG=1` connects and sends HTTP POST request; assertion messages avoid exposing secrets | Secret environment variables are shielded | TC-2214 |
| FR-2215 | Must | Given `dmuxai trust --revoke` on a workspace whose config is deleted or edited | Entry is removed from the store, exits 0, and subsequent `--status` reports `untrusted` | Revoke succeeds regardless of config file content | TC-2215 |
| FR-2216 | Must | Given state directory and home directory environment variables unset | Trust store cannot resolve a storage path; CLI operations requiring the store report an error and exit non-zero | Unconfigured state directory fails cleanly | TC-2216 |
| FR-2217 | Must | Given state directory and home directory unset during trust decision | `TrustStore::load` returns store without path, `is_trusted` returns `false`, and `decide` returns `Untrusted` (process opt-in remains functional) | Falls back safely to untrusted | TC-2217 |
| FR-2218 | Must | When creating a new trust state directory on Unix platforms | Directory permissions are set to `0700` | State directory accessible only by owner | TC-2218 |
| FR-2219 | Must | When loading `trusted-workspaces.json` with group- or world-writable mode on Unix | Trust store treats configuration as untrusted with a corruption warning and refuses overwrite on save | Unsafe permissions are ignored | TC-2219 |
| FR-2220 | Must | Given untrusted workspace config with skills source pointing through a symlink to outside the workspace | Skills source is stripped and treated as restricted key | Symlink escape outside workspace rejected | TC-2220 |
| FR-2221 | Must | Given untrusted workspace config with memory vault pointing through a symlink to outside the workspace | Memory vault setting is stripped and treated as restricted key | Symlink escape outside workspace rejected | TC-2221 |
| FR-2222 | Must | Given untrusted workspace with staged skill whose `SKILL.md` is a symlink resolving outside the workspace | Skill resolution fails with missing issue indicating outside workspace | Canonical target path confinement enforced | TC-2222 |
| FR-2223 | Must | Given untrusted workspace with dotpath skill reference resolving outside the workspace | Skill resolution fails with missing issue indicating outside workspace | Relative dotpath resolution confined to workspace | TC-2223 |
| FR-2224 | Must | Given restricted workspace settings during interactive approval prompt or `dmuxai trust` | Prints actual restricted values (argv commands, URLs, headers, agent/MCP commands and env key names only, skill sources, memory vault/dir, Jira URL, prompt files) without ever exposing secret environment variable values | Secret values are shielded while presenting full details | TC-2224 |
| FR-2225 | Must | Child process command construction under child environment policy | `DMUXAI_TRUST_WORKSPACE_CONFIG` and `DMUXAI_CONFIG` are removed in child process environments | Child processes never inherit trust opt-in | TC-2225 |
| FR-2226 | Must | Given a write-capable dashboard (`dmuxai --web`) saves workspace `dmuxai.toml` (skills or agent prompts) and, immediately before the write, the on-disk file hash equals the persistent trust record for that workspace | After a successful write the trust record is updated to the new file hash | The user's own dashboard save keeps the folder trusted | TC-2226, TC-2230 |
| FR-2227 | Must | Given a dashboard save in a workspace with no matching persistent trust record | No trust record is created or changed and no store file is written | Saves never grant trust | TC-2227 |
| FR-2228 | Must | Given trust comes only from the one-process opt-in (`--trust-workspace-config` / `DMUXAI_TRUST_WORKSPACE_CONFIG`) | A dashboard save does not create or upgrade a persistent record | Opt-in stays process-scoped | TC-2228 |
| FR-2229 | Must | Given the on-disk `dmuxai.toml` differs from the trusted hash (edited outside the dashboard) or from the text the save was based on | No refresh happens; after the save the workspace is untrusted | External edits keep invalidating trust | TC-2229, TC-2231 |
| FR-2230 | Must | Given a view-only dashboard (`dmuxai serve`) saves workspace `dmuxai.toml` | No trust refresh happens even when a matching record exists | Refresh limited to authenticated write-capable dashboards | TC-2232 |
| FR-2231 | Must | Given the config write fails | The error is returned and the trust store is not modified | Failed saves never touch trust | TC-2233 |
| FR-2234 | Must | Given any workspace (trusted, untrusted, or with no restricted keys) and a skill whose `SKILL.md` or skill directory lexically lies in the workspace | The canonical skill directory and `SKILL.md` must stay inside the canonical workspace; otherwise the skill is reported missing | Confinement applies regardless of trust state | TC-2234, TC-2235, TC-2236 |
| FR-2235 | Must | Given a path skill source from user config that resolves inside the workspace, or outside it | In-workspace sources are confined to the workspace; other path sources are confined to their own source root; git sources to their checkout | Source roots are enforced for every origin | TC-2237 |
| FR-2236 | Must | When a confined skill is read | `SKILL.md` is opened by its canonical path without following a final symlink and checked as a regular file on the open handle; listed skill files skip symlinks that leave the skill root and never descend into symlinked directories | Read matches the checked path | TC-2238 |
| FR-2237 | Must | Given a referenced skill directory without `SKILL.md` | The issue says the skill has no `SKILL.md`, not that it resolves outside the workspace | Accurate diagnostics | TC-2239 |
| FR-2238 | Must | When trust prompts, `dmuxai trust` output, and configuration warnings (TUI, CLI, dashboard) show configuration-derived text | C0/C1 control characters, DEL, line/paragraph separators, and bidirectional controls are shown as `\u{XX}` escapes; prompts show details built from the same bytes that are hashed and trusted | Displayed text cannot alter the terminal | TC-2240, TC-2243, TC-2244 |
| FR-2239 | Must | Given an existing trust state directory | On load and save it must be a real directory owned by the current user; group/world-writable directories are refused with a warning; other permissive modes are tightened to `0700` | State directory permissions enforced | TC-2241, TC-2242 |
| FR-2240 | Must | Given a memory vault (from user or workspace config) that lexically lies in the workspace | The canonical vault and memory root must stay inside the canonical workspace; otherwise memory is disabled with a warning | Memory root confinement | TC-2245, TC-2246, TC-2247 |
| FR-2241 | Should | When the startup trust prompt cannot save the record, or the trust store has a warning | An error is printed instead of silently continuing; `dmuxai trust` (including `--status`, with a `warning` JSON field) reports the store warning | Failures are visible | Manual review of `src/main.rs` `load_settings_with_trust_prompt` and `src/cli.rs` `trust_workspace` |
| FR-2242 | Must | Given two or more dashboard saves of the workspace `dmuxai.toml` at the same time in one process | Each save reads, edits, writes and refreshes trust while holding a per-workspace lock, so no save loses another save's section | Concurrent saves keep every change | TC-2248, TC-2251 |
| FR-2243 | Must | When the workspace `dmuxai.toml` is written by a dashboard save | The file is written to a temporary file in the same directory, synced and renamed into place; a failed edit or write leaves the file unchanged and no temporary file behind; trust store updates are serialized in the process | No partial files; trust stays current under concurrency | TC-2249, TC-2250 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2190 | FR-2190 | unit | Known byte slice `b"hello world\n"` | Call `content_hash` | Deterministic lowercase hex sha256 string `a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447` | `src/workspace_trust.rs` `tc_2190_content_hash_stable_and_hex` |
| TC-2191 | FR-2191 | unit | `RuntimeEnv` with `dmuxai_state_dir`, `xdg_state_home`, and `home` | Call `TrustStore::store_path` under each combination | Resolves `trusted-workspaces.json` following strict precedence; errors when all absent | `src/workspace_trust.rs` `tc_2191_store_path_resolution_precedence` |
| TC-2192 | FR-2192 | unit | Temporary state directory and workspace path | `store.trust()` with hash, `save()`, then `load()` in new store | Saved workspace entry is present, trusted, and has valid timestamp | `src/workspace_trust.rs` `tc_2192_trust_save_load_round_trip` |
| TC-2193 | FR-2193 | unit | Workspace path trusted with hash A | Query `store.is_trusted()` with hash B or different workspace path | Returns `false` on any hash or path difference | `src/workspace_trust.rs` `tc_2193_is_trusted_false_when_hash_or_path_differs` |
| TC-2194 | FR-2194 | unit | Trusted workspace in store | Call `store.revoke()` | Returns `true` and clears entry; second call returns `false` | `src/workspace_trust.rs` `tc_2194_revoke` |
| TC-2195 | FR-2195 | unit | State directory containing non-JSON string in `trusted-workspaces.json` | `TrustStore::load()` | Generates corrupt warning; store is empty; no crash | `src/workspace_trust.rs` `tc_2195_corrupt_json_handling` |
| TC-2196 | FR-2196 | unit | Unix filesystem environment | `store.trust()` and `store.save()` | Metadata permissions mode of saved `trusted-workspaces.json` is `0600` | `src/workspace_trust.rs` `tc_2196_saved_file_mode_0600` |
| TC-2197 | FR-2197 | unit | Clean state directory | Evaluate `decide()` initially, with `trust_workspace_config = true`, and with store record | Transitions `Untrusted` -> `Trusted(OptIn)` (file not created) -> `Trusted(Record)` | `src/workspace_trust.rs` `tc_2197_decide_logic` |
| TC-2198 | FR-2198 | unit | Untrusted workspace config with `[notify]` command, webhook URL, secret env, and header | `DmuxSettings::load_with_env()` | Notify execution keys are cleared; events kept; single sanitized warning generated without secret values | `src/settings.rs` `tc_2198_untrusted_notify_keys_stripped_and_warnings_sanitized` |
| TC-2199 | FR-2199 | unit | Untrusted workspace config with `[[agents.custom]]` | `DmuxSettings::load_with_env()` | `custom_agents` vector does not contain workspace custom agent; trust status is `Untrusted` | `src/settings.rs` `tc_2199_untrusted_custom_agents_stripped` |
| TC-2200 | FR-2200 | unit | Untrusted workspace config with `[[mcp.servers]]` | `DmuxSettings::load_with_env()` | `mcp_servers` vector does not contain workspace MCP server; trust status is `Untrusted` | `src/settings.rs` `tc_2200_untrusted_mcp_servers_stripped` |
| TC-2201 | FR-2201 | unit | Untrusted workspace config with git skill source, outside vault, and Jira URL | `DmuxSettings::load_with_env()` | Git skill source omitted, memory settings cleared, Jira base URL cleared | `src/settings.rs` `tc_2201_untrusted_skills_git_vault_outside_and_jira_stripped` |
| TC-2202 | FR-2202 | unit | User-level configuration with notify, custom agent, and MCP server | `DmuxSettings::load_with_user_config_and_env()` with no workspace config | User settings load intact; status `NoWorkspaceConfig`; no trust warnings emitted | `src/settings.rs` `tc_2202_user_config_path_loads_unchanged_without_warning` |
| TC-2203 | FR-2203 | unit | Workspace trusted by hash in store | Load settings, modify workspace `dmuxai.toml` on disk, re-load settings | Status transitions from `Trusted(Record)` to `Untrusted` with the new hash | `src/settings.rs` `tc_2203_trust_record_applied_modify_file_reverts_to_untrusted` |
| TC-2204 | FR-2204 | unit | Untrusted workspace config with restricted notify command | Load settings with `RuntimeEnv::trust_workspace_config = true` | Status is `Trusted(OptIn)`; notify command is retained intact | `src/settings.rs` `tc_2204_opt_in_yields_trusted_opt_in` |
| TC-2205 | FR-2205 | unit | Workspace `dmuxai.toml` with only `[ui]` and `[graph]` settings | `DmuxSettings::load_with_env()` | Status is `NotNeeded`; no untrusted warnings emitted | `src/settings.rs` `tc_2205_harmless_workspace_config_is_not_needed_without_warning` |
| TC-2206 | FR-2206 | unit | Workspace with restricted notify and corrupted `trusted-workspaces.json` on disk | `DmuxSettings::load_with_env()` | Status is `Untrusted`; corrupt store warning surfaced in `config_warnings` | `src/settings.rs` `tc_2206_corrupt_store_yields_untrusted_and_warning` |
| TC-2207 | FR-2207 | integration | Workspace config with notify command touching marker file | Run `dmuxai run hello --agent user-fake --json` | Exit 0; marker file is not created; config warning emitted on stderr | `tests/workspace_trust.rs` `tc_2207_untrusted_workspace_config` |
| TC-2208 | FR-2204, FR-2208 | integration | Workspace config with notify command touching marker file | Run with env `DMUXAI_TRUST_WORKSPACE_CONFIG=1` | Exit 0; marker file is created; status command reports `untrusted` (store untouched) | `tests/workspace_trust.rs` `tc_2208_trust_via_env_var` |
| TC-2209 | FR-2204, FR-2209 | integration | Workspace config with notify command touching marker file | Run with CLI flag `--trust-workspace-config` | Exit 0; marker file is created; status command reports `untrusted` | `tests/workspace_trust.rs` `tc_2209_trust_via_cli_flag` |
| TC-2210 | FR-2210 | integration | Untrusted workspace config | Execute `trust --status --json`, `trust --yes`, and repeat status | Status transitions from `untrusted` to `trusted`; subsequent run creates marker | `tests/workspace_trust.rs` `tc_2210_trust_cli_command_lifecycle` |
| TC-2211 | FR-2203, FR-2211 | integration | Workspace config trusted with `trust --yes` | Append comment to `dmuxai.toml` and run `dmuxai run` | Run succeeds without executing notify command; `trust --status --json` reports `untrusted` | `tests/workspace_trust.rs` `tc_2211_modified_config_invalidates_trust` |
| TC-2212 | FR-2194, FR-2212 | integration | Workspace config trusted with `trust --yes` | Run `dmuxai trust --revoke` | Exit 0; status reports `revoked`; subsequent `--status --json` reports `untrusted` | `tests/workspace_trust.rs` `tc_2212_trust_revoke` |
| TC-2213 | FR-2213 | unit | Main CLI `USAGE` text constant | Inspect command lists for `todo run` and `trust` | `dmuxai todo run` includes `--trust-workspace-config`; `dmuxai trust` listed with flags | `src/main.rs` `tc_2213_usage_lists_todo_run_and_trust_once` |
| TC-2214 | FR-2214 | integration | Workspace `[notify]` with webhook URL, secret env, and listening TCP socket | Run without trust, then run with `DMUXAI_TRUST_WORKSPACE_CONFIG=1` | Untrusted run makes no socket connection; trusted run connects and issues HTTP POST | `tests/workspace_trust.rs` `tc_2214_webhook_trust_enforcement` |
| TC-2215 | FR-2215 | integration | Workspace config trusted then config deleted | Run `dmuxai trust --revoke` | Exit 0; status reports `revoked`; trust store removes record | `tests/workspace_trust.rs` `tc_2215_trust_revoke_after_delete_or_edit_config` |
| TC-2216 | FR-2216 | integration | CLI invoked with all state and home env vars stripped | Run `dmuxai trust --yes` or `dmuxai trust --revoke` | Exit non-zero; error message reports trust store path cannot be resolved | `tests/workspace_trust.rs` `tc_2216_trust_store_path_unset_errors_cli` |
| TC-2217 | FR-2217 | unit | `RuntimeEnv` with all state and home directories unset | Evaluate `TrustStore::load` and `decide` | Store path is None, `is_trusted` is false, `decide` is Untrusted | `src/workspace_trust.rs` `tc_2217_unset_state_and_home_returns_untrusted_and_no_store_path` |
| TC-2218 | FR-2218 | unit | Unix filesystem environment | `TrustStore::save` when creating state directory | Directory mode is 0700 | `src/workspace_trust.rs` `tc_2218_state_dir_created_with_0700` |
| TC-2219 | FR-2219 | unit | State directory with group-writable `trusted-workspaces.json` | `TrustStore::load` and `store.save` | Corrupt warning emitted; untrusted; save returns error | `src/workspace_trust.rs` `tc_2219_group_writable_trust_store_ignored_with_warning` |
| TC-2220 | FR-2220 | unit | Untrusted workspace with skill source symlink pointing outside | `DmuxSettings::load_with_env` | Skill source stripped and ignored | `src/settings.rs` `tc_2220_untrusted_skills_source_symlink_dir_outside_stripped` |
| TC-2221 | FR-2221 | unit | Untrusted workspace with memory vault symlink pointing outside | `DmuxSettings::load_with_env` | Memory setting stripped | `src/settings.rs` `tc_2221_untrusted_memory_symlink_dir_outside_stripped` |
| TC-2222 | FR-2222 | unit | Untrusted workspace with skill having `SKILL.md` symlinked outside | `crate::stage_skills::resolve` with `confine_to` | Returns `IssueKind::Missing` resolving outside workspace | `src/stage_skills.rs` `tc_2222_untrusted_skill_md_symlink_outside_rejected` |
| TC-2223 | FR-2223 | unit | Untrusted workspace with dotpath skill symlinked outside | `crate::stage_skills::resolve` with `confine_to` | Returns `IssueKind::Missing` resolving outside workspace | `src/stage_skills.rs` `tc_2223_untrusted_dotpath_symlink_outside_rejected` |
| TC-2224 | FR-2224 | unit | Untrusted workspace with commands, webhook, secret env, and custom agents | `restricted_workspace_details` with secret env var set in process | Returns details with command strings and env key names; never contains secret value | `src/settings.rs` `tc_2224_restricted_workspace_details_hides_secret_values` |
| TC-2225 | FR-2225 | unit | Process command with `DMUXAI_TRUST_WORKSPACE_CONFIG` and `DMUXAI_CONFIG` set | `apply_child_env_policy` | Command environment removes both variables (values are None) | `src/process/mod.rs` `tc_2225_child_env_policy_removes_trust_workspace_config_and_config` |
| TC-2226 | FR-2226 | unit | Workspace config trusted by record | `write_workspace_config_refreshing_trust` with `allow_refresh = true` and matching `based_on` | Returns `Refreshed`; new hash trusted, old hash not; file updated | `src/workspace_trust.rs` `tc_2226_trusted_save_refreshes_record` |
| TC-2227 | FR-2227 | unit | Workspace config with no trust record | Same helper | Returns `NotEligible`; file written; store file absent | `src/workspace_trust.rs` `tc_2227_untrusted_save_stays_untrusted` |
| TC-2228 | FR-2228 | unit | No record, `trust_workspace_config = true` | Same helper | Returns `NotEligible`; store file absent; new hash untrusted without the flag | `src/workspace_trust.rs` `tc_2228_opt_in_flag_does_not_persist_trust` |
| TC-2229 | FR-2229 | unit | Record for old hash; file edited on disk | Same helper with edited or stale `based_on`, and with `allow_refresh = false` | Returns `NotEligible` in every case; new hash untrusted | `src/workspace_trust.rs` `tc_2229_external_edit_blocks_refresh` |
| TC-2230 | FR-2226 | unit | `--web` router (`web_attached = true`), trusted workspace | `POST /api/agent-prompts` with valid token | 200; trust record matches the saved file hash | `src/web/server.rs` `tc_2230_dashboard_save_refreshes_trust` |
| TC-2231 | FR-2229 | unit | `--web` router, trusted workspace, `dmuxai.toml` then edited externally | `POST /api/agent-prompts` | 200; workspace is untrusted for the saved file hash | `src/web/server.rs` `tc_2231_external_edit_then_dashboard_save_stays_untrusted` |
| TC-2232 | FR-2230 | unit | View-only router (`web_attached = false`), trusted workspace | `POST /api/skills` | No trust refresh; workspace untrusted for the saved file hash | `src/web/server.rs` `tc_2232_view_only_save_does_not_refresh_trust` |
| TC-2233 | FR-2231 | unit | `dmuxai.toml` path is a directory, workspace trusted | Same helper | Returns `Err`; trust store file unchanged | `src/workspace_trust.rs` `tc_2233_write_failure_leaves_trust_untouched` |
| TC-2234 | FR-2234 | unit | No confinement override; `.agents/skills/a/SKILL.md` symlinks outside | `resolve` | Missing, resolves outside the workspace | `src/stage_skills.rs` `tc_2234_skill_md_symlink_outside_rejected_without_restricted_keys` |
| TC-2235 | FR-2234 | unit | `.agents/skills/b` directory symlink to outside | `resolve` | Missing | `src/stage_skills.rs` `tc_2235_agents_skill_dir_symlink_outside_rejected` |
| TC-2236 | FR-2234 | unit | `./linked` directory symlink to outside, no restricted keys | `resolve` | Missing | `src/stage_skills.rs` `tc_2236_dotpath_dir_symlink_outside_rejected_without_restricted_keys` |
| TC-2237 | FR-2235 | unit | User-config relative path source pointing at an in-workspace symlink to outside | `resolve` | Missing | `src/stage_skills.rs` `tc_2237_user_relative_source_to_in_workspace_symlink_rejected` |
| TC-2238 | FR-2236 | unit | Real in-workspace skill with a regular file and an outward file symlink | `resolve` | Ok; files list has the regular file only | `src/stage_skills.rs` `tc_2238_in_workspace_skill_still_resolves` |
| TC-2239 | FR-2237 | unit | `./nothing_here` without `SKILL.md` | `resolve` | Missing, has no SKILL.md | `src/stage_skills.rs` `tc_2239_missing_skill_md_reports_missing_not_escape` |
| TC-2240 | FR-2238 | unit | String with ESC, CR, NUL, C1 and bidi controls | `escape_control` | No control chars remain; escapes present | `src/workspace_trust.rs` `tc_2240_escape_control_chars` |
| TC-2241 | FR-2239 | unit | State dir with mode 0755 | `TrustStore::save` and `load` | Mode becomes 0700 | `src/workspace_trust.rs` `tc_2241_state_dir_0755_tightened_to_0700` |
| TC-2242 | FR-2239 | unit | State dir with mode 0775 | `TrustStore::load` and `save` | Warning; save refused; no store file | `src/workspace_trust.rs` `tc_2242_group_writable_state_dir_refused` |
| TC-2243 | FR-2238 | unit | Notify command value containing control characters | `restricted_workspace_details` | Keys and values contain no control chars | `src/settings.rs` `tc_2243_restricted_details_escape_control_chars` |
| TC-2244 | FR-2238 | unit | Unparseable workspace config containing a control byte | `DmuxSettings::load_with_user_config_and_env` | Warnings contain no control chars | `src/settings.rs` `tc_2244_config_warnings_escape_control_chars` |
| TC-2245 | FR-2240 | unit | User vault at an in-workspace symlink to outside | Settings load | Memory disabled with warning | `src/settings.rs` `tc_2245_user_memory_vault_symlink_in_workspace_disabled` |
| TC-2246 | FR-2240 | unit | User vault at a real in-workspace dir | Settings load | Memory enabled | `src/settings.rs` `tc_2246_workspace_memory_vault_inside_ok` |
| TC-2247 | FR-2240 | unit | Real in-workspace vault whose `dir` is a symlink to outside | Settings load | Memory disabled | `src/settings.rs` `tc_2247_relative_memory_root_symlink_escape_disabled` |
| TC-2248 | FR-2242 | unit | Workspace config file | 16 threads update different sections at once | All 16 sections present | `src/workspace_trust.rs` `tc_2248_concurrent_updates_keep_all_sections` |
| TC-2249 | FR-2243 | unit | Workspace config file | Several updates, then an edit that fails | No `*.tmp` files; last content kept; failed edit leaves the file unchanged | `src/workspace_trust.rs` `tc_2249_atomic_write_leaves_no_temp_files` |
| TC-2250 | FR-2243 | unit | Trusted workspace | 8 concurrent updates with refresh allowed | Store trusts the final file hash | `src/workspace_trust.rs` `tc_2250_concurrent_refresh_keeps_trust` |
| TC-2251 | FR-2242 | unit | `--web` router, trusted workspace | `POST /api/skills` and `POST /api/agent-prompts` at the same time, 10 rounds | Both changes present after each round; still trusted | `src/web/server.rs` `tc_2251_concurrent_dashboard_saves_keep_both_sections` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| State store | `<state_dir>/trusted-workspaces.json` maps workspace paths to `{ sha256, trusted_at }` | Deleting file reverts all workspaces to untrusted | TC-2192, TC-2196 |
| Configuration | Untrusted workspace `dmuxai.toml` has restricted keys stripped with one summarized warning | Safe defaults; user config unaffected | TC-2198..TC-2202 |
| CLI commands | Subcommand `dmuxai trust` and flag `--trust-workspace-config` on `run` and `todo run` | Additive CLI surface; scripts opt in via environment variable or flag | TC-2208..2210, TC-2213 |
| Environment | `DMUXAI_TRUST_WORKSPACE_CONFIG=1` enables one-process trust | Process-scoped; does not mutate disk store | TC-2208, TC-2214 |
| Dashboard save | A write-capable dashboard save refreshes an existing trust record only when the on-disk file still matches it | Never creates trust; view-only and opt-in never refresh | TC-2226..TC-2233 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | SHA-256 digest and trust store persistence | `src/workspace_trust.rs` | FR-2190..FR-2197 | TC-2190..TC-2197 |
| 2 | Restricted key inspection and stripping | `src/settings.rs` | FR-2198..FR-2206 | TC-2198..TC-2206 |
| 3 | CLI trust subcommand and runtime flags | `src/cli.rs`, `src/runtime_env.rs`, `src/main.rs` | FR-2207..FR-2213 | TC-2207..TC-2213 |
| 4 | Webhook and subprocess trust integration | `tests/workspace_trust.rs` | FR-2214 | TC-2214 |
| 5 | Dashboard save trust refresh | `src/workspace_trust.rs`, `src/web/server.rs` | FR-2226..FR-2231 | TC-2226..TC-2233 |
| 5b | Locked, atomic dashboard config saves | `src/workspace_trust.rs`, `src/web/server.rs` | FR-2242, FR-2243 | TC-2248..TC-2251 |
| 6 | Always-on skill and memory confinement, display escaping, state dir checks | `src/stage_skills.rs`, `src/settings.rs`, `src/workspace_trust.rs`, `src/cli.rs`, `src/main.rs` | FR-2234..FR-2241 | TC-2234..TC-2247 |
| 7 | Specification and documentation registration | `docs/specs/SPEC-workspace-config-trust.md`, `SECURITY.md`, `docs/API.md` | All | `python3 scripts/check-spec.py --all` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-2190..FR-2243) map to passing automated test evidence (TC-2190..TC-2251).
- [x] Spec gate checks pass: `python3 scripts/check-spec.py --all` and `python3 scripts/check-tc-links.py`.
