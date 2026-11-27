# SPEC-web-all-projects

Status: In progress · Series: global web view 3/3 · Builds on SPEC-web-default-mode · Request: t754u

## 1. Decision summary
The `--web` dashboard already lists other workspaces' conversations read-only from the shared state directory (FR-1225, `list_state_dir_conversations`). Runs are not covered: run journals live per repo key under the data directory and only the current workspace's runs are shown, so headless runs started in other paths are invisible. This adds an "All projects" view that lists every project of this OS user with its conversations and runs, adds a per-repo `workspace.json` so run-only repos get a path, and lets the owner open a project in the running dashboard to continue.

## 2. Scope

In scope: `workspace.json` index, workspace path in new run manifests, global project/run APIs (list, detail, live stream), All projects UI, open action, masking, repo-key validation, serve read-only.
Out of scope: editing another repo's conversation without opening it, other OS users' data, transcript search.

### Assumptions and open questions
| ID | Question | Default chosen in this spec |
|---|---|---|
| OQ-2980 | How are repos discovered? | Every `<state base>/repos/<key>/` plus every runs root `<data base>/repos/<key>/runs` (the root used by `run_journal::repo_runs_dir`). The path comes from `workspace.json`, then the newest run manifest's `workspace_root`, then the conversations database; otherwise the project is shown as `unknown path (<key>)`. |
| OQ-2981 | Available in view-only `serve`? | Yes, listing and detail; the open action needs `--web`. |
| OQ-2982 | Missing project folders | Listed with `missing: true`; detail readable; open disabled. |

## 3. Terms
- Repo key: `state_path::repo_key_for_canonical` output, `<sanitized>-<16 hex>`.
- Valid repo key: a request key is accepted only if it is not empty, not `.` or `..`, has no `/`, `\` or NUL, and equals the name of a directory found by the OQ-2980 enumeration (lookup, not pattern); new helper `lookup_repo_key`.
- State base / data base: `state_path::state_base()` and the base used by `run_journal::repo_runs_dir` (honors `XDG_DATA_HOME`; tests add `DMUXAI_DATA_DIR` for isolation).

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-2980 | Must | When a workspace is opened (TUI, web host, in-process open/switch) or a headless run starts, `<state base>/repos/<key>/workspace.json` is written atomically with `canonical_path`, `repo_key`, `last_seen_ms` (directory 0700, file 0600). New run manifests also record `workspace_root`. A write failure is a warning, never an error. |
| FR-2981 | Must | `GET /api/global/projects` lists every repo found per OQ-2980: `repo_key`, `canonical_path` (or null), `missing`, `conversation_count`, `run_count`, `active_run_count`, `last_activity_ms`, `open_ws` (workspace id when this dashboard serves it). Sorted by `last_activity_ms` desc. Databases are opened read-only and never migrated or written. |
| FR-2982 | Must | `GET /api/global/projects/{repo_key}/conversations` lists that repo's conversations (id, title, updated_at_ms, turn_count, spoken agents) reusing the FR-1225 reader; `.../conversations/{id}` returns the transcript read-only; `.../runs` and `.../runs/{run_id}` return the run list and detail in the `/api/runs` format; `.../runs/{run_id}/stream` streams an active run like the existing run stream. Unknown or invalid repo keys or ids return 404. |
| FR-2983 | Must | Responses mask secrets: transcript and log text through `mask_sensitive_tokens`, JSON through the run-journal masking, and the dashboard token through `redact_token_in_value`; the dashboard token never appears. |
| FR-2984 | Must | The dashboard has an "All projects" entry showing projects with counts and an active-run marker; selecting a project shows its conversations and runs; selecting one shows a read-only transcript or run detail (live for active runs) with a "read-only" badge. |
| FR-2985 | Must | In a `--web` host, "Open project" opens that `canonical_path` in-process (the SPEC-web-default-mode attach/switch path) and then loads the selected conversation (`load_conversation` command for the new `ws`). In view-only `serve` and for missing folders the button is disabled with a hint. |
| FR-2986 | Must | Files are located only from the validated repo key under this user's state and data bases; no request path or query value is joined to a filesystem path. |
| NFR-2980 | Must | Listing uses bounded queries (counts and latest rows, no full transcript scan) and is cached for 5 s; other requests have no added cost; benchmark within 10% (#301), numbers in the PR. |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-2980 | FR-2980 | `tc_2980_workspace_json_written_on_open_and_run_start_and_manifest_has_root` |
| TC-2981 | FR-2981 | `tc_2981_global_projects_lists_repos_from_state_and_data_bases` |
| TC-2982 | FR-2982 | `tc_2982_global_conversation_run_detail_and_stream_read_only_bad_keys_404` |
| TC-2983 | FR-2983 | `tc_2983_global_responses_mask_secrets` |
| TC-2984 | FR-2984 | `tc_2984_all_projects_view_renders_projects_conversations_runs` |
| TC-2985 | FR-2985 | `tc_2985_open_project_attaches_and_loads_conversation_disabled_in_serve` |
| TC-2986 | FR-2986 | `tc_2986_valid_repo_key_blocks_traversal` |
| TC-2987 | NFR-2980 | `tc_2987_global_listing_is_cached_and_bounded` |

## 6. Data and safety contracts
New file per repo: `workspace.json`; new optional manifest field `workspace_root` (old manifests stay readable). Databases opened read-only. No cross-user access.

## 7. Exit criteria
All TCs pass, gate and web tests pass, benchmark within 10%.
