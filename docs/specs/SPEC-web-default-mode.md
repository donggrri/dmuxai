# SPEC-web-default-mode

Status: In progress · Series: global web view 2/3 · Builds on SPEC-web-local-access · Request: t754u

## 1. Decision summary
The owner always wants the web dashboard, and wants one dashboard for every project he works in. Plain `dmuxai` therefore starts web mode by default (the headless `--web` host), and the TUI becomes `dmuxai --tui` or `[ui] default_mode = "tui"`. When a `--web` dashboard of the same OS user is already running, `dmuxai` in another project adds that project to the running dashboard (in-process workspace hosting, the same path as `POST /api/workspace/switch`) and prints its URL instead of starting a second server.

## 2. Scope

In scope: default mode resolution, `--tui` and `--new` flags, `[ui] default_mode`, reuse of a running dashboard via the user-level discovery file, `[web] max_workspaces`, messages and exit codes.
Out of scope: the all-projects list (SPEC-web-all-projects), auto-opening a browser, Windows native.

### Assumptions and open questions
| ID | Question | Default chosen in this spec |
|---|---|---|
| OQ-2970 | What does plain `dmuxai` start? | Web mode (headless host, `--web`). `--tui`, `DMUXAI_UI=tui` or `[ui] default_mode = "tui"` gives the TUI. |
| OQ-2971 | Second `dmuxai` in another path while a dashboard runs | Reuse: add the workspace to the running dashboard and exit 0 after printing its URL (`?ws=<id>`). `--new` starts a separate dashboard. |
| OQ-2972 | Workspace limit of one dashboard | `[web] max_workspaces`, default 8 (was a fixed 4), range 1..32. Over the limit: exit 4 with a message to close one in the dashboard. |
| OQ-2973 | Repo custom agents on reuse | Custom agents are a process-wide registry, so the in-process open refuses a workspace whose repo config defines `[[agents.custom]]` (today's behavior). Then `dmuxai` does not reuse: it starts its own dashboard as with `--new` and says why (FR-2978). Workspace trust is handled by the host's existing in-process open path. |

## 3. Terms
- Live web dashboard: the user-level discovery file exists, `mode = "web"`, its `uid` is ours, its pid is alive, and `GET <base_url>/api/ping` (with the secret from the file when `token_required`) answers 200 within 2 s with the `dmuxai.dashboard/v1` schema and its `mode`. `/api/ping` is a small side-effect-free endpoint so the probe does not depend on the snapshot size.
- Web mode: the existing headless `--web` host.

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-2970 | Must | Plain `dmuxai` (no subcommand) runs web mode when the resolved default mode is `web`. Resolution happens after settings load, in this order: `--tui` / `--web` flag, `DMUXAI_UI` (`web`/`tui`), `[ui] default_mode` (normal settings merge: workspace config over user config), built-in `web`. Invalid values are a config error naming the key. |
| FR-2971 | Must | `dmuxai --tui` runs today's TUI regardless of config. `--tui` together with `--web` or `serve` is a usage error (exit 2). Plain `dmuxai` accepts `--port`, `--bind`, `--allow-remote` and `--new`, which apply to web mode and are a usage error with `--tui`. |
| FR-2972 | Must | In web mode without `--new`, before starting a server, `dmuxai` checks for a live web dashboard. If one exists it asks that host to open the current workspace in-process with a new `POST /api/workspace/attach` (same code path as `POST /api/workspace/switch`'s in-process open, but it does not change the active view of open browser tabs), using the token from the discovery file only when required, otherwise tokenless with `X-Dmuxai-Client: 1`. On success it prints `dmuxai dashboard: <base_url>/?ws=<id>` (with the secret only when required, FR-2956) and exits 0 without starting a server. If the workspace is already open there, it just prints its URL. |
| FR-2973 | Must | If the live dashboard reports it is full, `dmuxai` exits 4 with a message. If there is no live web dashboard (missing, stale, `serve`, other uid, unreachable), `dmuxai` starts its own web host as today and becomes the dashboard in the discovery file. |
| FR-2974 | Must | `--new` skips reuse and starts a separate dashboard on `--port` (default 7420) with today's fallback to a free port when busy; it does not replace a live web dashboard in the user-level discovery file. "Open in new window" in the dashboard (`spawn_dashboard`) passes `--new`. |
| FR-2975 | Must | `[web] max_workspaces` (default 8, 1..32) replaces the fixed limit of 4 of the headless host directory. |
| FR-2976 | Must | A non-TTY plain `dmuxai` in web mode runs (web mode has no TUI); in tui mode the existing TTY error stays. |
| FR-2978 | Must | When the in-process open is refused because the workspace defines repo custom agents, `dmuxai` prints one line saying the running dashboard cannot host this workspace and starts its own dashboard as with `--new`. |
| FR-2977 | Should | `dmuxai --help` and README document the default mode, `--tui`, `--new`, `[ui] default_mode`, `[web] max_workspaces`. |
| NFR-2970 | Must | The reuse probe adds at most one local HTTP round trip (2 s timeout) to start-up and nothing when no discovery file exists; benchmark within 10% (#301), numbers in the PR. |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-2970 | FR-2970 | `tc_2970_default_mode_resolution_order` |
| TC-2971 | FR-2971 | `tc_2971_tui_flag_and_conflicts` |
| TC-2972 | FR-2972 | `tc_2972_second_start_opens_workspace_in_live_dashboard` |
| TC-2973 | FR-2973 | `tc_2973_full_view_only_or_stale_dashboard_handling` |
| TC-2974 | FR-2974 | `tc_2974_new_flag_skips_reuse_and_spawn_dashboard_passes_new` |
| TC-2975 | FR-2975 | `tc_2975_max_workspaces_setting_bounds_and_limit` |
| TC-2976 | FR-2976 | `tc_2976_non_tty_plain_run_uses_web_mode` |
| TC-2977 | FR-2977 | `tc_2977_help_documents_default_mode_flags` |
| TC-2978 | NFR-2970 | `tc_2978_no_discovery_file_skips_reuse_probe` |
| TC-2979 | FR-2978 | `tc_2979_repo_custom_agents_fall_back_to_own_dashboard` |

## 6. Data and safety contracts
No schema change. Reuse only targets a dashboard with our uid listening on loopback. Existing tests to update: `parse_cli_empty_args_defaults_to_run`, `tc_520`, `non_tty_start_fails_with_clear_message` (use `--tui`), `tc_1602`, `insert_up_to_max_and_full_error`, `tc_2305`. Rollback: `[ui] default_mode = "tui"`.

## 7. Exit criteria
All TCs pass, gate and web tests pass, benchmark within 10%.
