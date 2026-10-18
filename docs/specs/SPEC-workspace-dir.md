# SPEC: workspace-dir

Status: Implemented (v0.2.2)
Created: 2026-10-18
Related: issue #56; Dongjin 2026-10-18 09:42 KST ("let the user set the project working directory"); `workspace::resolve_workspace_root` (`DMUXAI_WORKSPACE` or cwd); SPEC-state-per-repo (per-repo DB); SPEC-multi-instance-workspace

## 1. Decision summary

The workspace (project directory) is fixed at start: `DMUXAI_WORKSPACE` if set, otherwise the current directory. Runners, the conversation DB, documents, to-dos and events all key off it. You cannot pick it from the CLI with a flag, from config, or from the dashboard.

Decision:

1. **CLI and config.** A global `-C DIR` / `--workspace-dir DIR` flag, and `[workspace] dir` in the **user** config. Precedence: flag, then `DMUXAI_WORKSPACE`, then user config, then the current directory.
2. **Dashboard.** A workspace control in the header shows the current path. It opens a dialog with a path input, server-side validation, a directory-only browser and recent paths. Confirming a switch **re-executes the process** in the new workspace (Unix `exec`, same port, bind and token), so every component re-opens cleanly on the new directory. The page reconnects by itself.
3. **Runners** always get the active workspace as their working directory and `{workspace}` value. This is checked by a test.

Re-exec is simpler and safer than re-wiring the live App, which holds about 40 workspace-bound handles (DB, event sink, docs, to-dos, MCP, warm cursor process). On Windows the switch endpoint answers 501 (Windows compatibility is postponed); the CLI and config parts work there too.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-870 | Must | `dmuxai [-C DIR \| --workspace-dir DIR] [subcommand…]` sets the workspace for every mode (TUI, `--web`, `serve`, `run`, `todo`, `status`, `events`, `mcp`, `init --workspace`). Precedence: flag, then `DMUXAI_WORKSPACE`, then user-config `[workspace] dir` (`~` expanded), then the current directory. A missing, non-directory or rejected path (FR-873) exits with code 2 and a one-line error | TC-870, TC-871 |
| FR-871 | Must | The header shows the active workspace path: abbreviated with `~`, ellipsized in the middle, full path in `title`. It is a button that opens the workspace dialog. The snapshot already carries `workspace`; `serve` shows it the same way | TC-872, TC-874 |
| FR-872 | Must | The dialog has a path input and a recent-paths list (at most 10, most recent first, stored in the state dir `recent-workspaces.json`; entries that no longer pass validation are hidden). It also has a directory browser fed by `GET /api/workspace/dirs?path=`, which lists **child directory names only** (no files, dotdirs hidden unless `hidden=1`, at most 500, sorted). Browsing is limited to the browse roots: `$HOME` and the optional user-config list `[workspace] browse_roots`. Each child is canonicalized before listing and dropped if it leaves the roots or is denied (FR-873). A path typed into the input may lie outside the roots; it only has to pass validation `POST /api/workspace/validate {path}` returns the canonical path or a reason. Both need the dashboard token and are refused in view-only mode, except `validate` | TC-873, TC-874 |
| FR-873 | Must | Validation (CLI, config and dashboard alike): the path must exist and be a directory after canonicalization (symlinks resolved, `..` removed). It must be readable and writable by the user (the existing write probe), and must not be `/`, the DmuxAI state dir or a path inside it, the DmuxAI user config dir, or a credential directory: `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.config/gh`, `~/.docker`, `~/.kube`, `~/.password-store`, `~/.local/share/keyrings`, and the runner credential dirs `~/.cursor`, `~/.codex`, `~/.gemini`, `~/.local/share/opencode`. A path inside any of these is denied too. Paths with NUL or over 4096 bytes are rejected. The directory browser applies the same deny list, so denied directories are not listed | TC-870, TC-873 |
| FR-874 | Must | `POST /api/workspace/switch {path}` validates the path. It answers 409 while any turn or graph run is in flight, and 403 in view-only mode. Otherwise it sets a switch-pending flag **atomically with the busy check** (under the same App lock); from then on new turns, graph runs and to-do runs are refused with 409 "workspace switch in progress". It records the path in the recent list, answers 202 and then, on Unix, re-executes the current binary with the original arguments minus any `-C DIR`, `--workspace-dir DIR` or `--workspace-dir=DIR`, plus `-C <canonical path>`, and the same port, bind and `DMUXAI_WEB_TOKEN`, after a clean shutdown (warm processes reaped, session heartbeat closed). Before tearing anything down it runs a preflight (`current_exe()` resolves and is executable, the target still validates). If the preflight fails, nothing has been shut down: it clears the switch-pending flag and publishes the error as a dashboard notice. If `exec` itself fails after the clean shutdown (it returns only on error), the process prints the error and the exact restart command (`dmuxai … -C <path>`) to stderr and exits with code 1. It never keeps serving a half-shut-down workspace. On non-Unix it answers 501. The dashboard shows "switching…", reconnects and reloads the snapshot | TC-875, TC-876 |
| FR-875 | Must | Every runner turn (built-in and custom agents, graph nodes, headless `run`) uses the active workspace as its process working directory and as `{workspace}`. No code path may fall back to the process cwd once the workspace is resolved | TC-877 |
| NFR-870 | Must | The workspace API returns directory names only, never file names or file contents, and only inside the browse roots. It never follows a path or symlink into the deny list, and never logs or returns the dashboard token. Errors name the reason (`not a directory`, `not writable`, `denied: credential directory`) without listing directory contents | TC-873 |

### Design notes

- `dmuxai init --workspace` keeps its boolean meaning (write to the workspace config). The new global flag is spelled `-C` / `--workspace-dir` to avoid the clash.
- Recent paths live in the state dir, not in a workspace, so they are not committed with a project.
- The TUI header and status line already show the workspace name. The TUI does not get a switch command in v0.2.2; you restart with `-C`. A TUI picker is listed under "Later".

### Implementation decisions

- Because `dmuxai serve` is view-only and FR-872/FR-874 refuse `/api/workspace/dirs` and `/api/workspace/switch` with 403 in view-only mode, `TC-874` and `TC-876` launch `dmuxai --web` under a pseudo-terminal (`pty.fork()`) so both directory browsing and Unix `exec` re-execution run against the interactive web server while view-only 403 enforcement remains covered by `TC-873` and `TC-875`.
- Deny-list checks (`check_workspace_denied`) run before the write probe in `validate_workspace_dir` so `/`, the state/config dirs, and read-only credential directories report their `denied: ...` reason and never have a probe file created inside them.
- The switch handler runs its post-validation work in a spawned task. If the App does not answer within 10s, or drops the reply, it sends `CancelSwitchWorkspace` and answers 504. A dropped client connection can therefore never leave switch-pending set (`workspace_switch_timeout_sends_cancel`).
- A failing preflight answers 500 with the notice text, and also publishes it as a dashboard notice. It used to answer 202 although nothing was switched.
- Recent paths are filtered on every `GET /api/workspace` with `validate_workspace_dir_no_probe`: the same checks except the write probe, so a read-only request never creates files. A recent directory that has since become read-only still fails with `not writable` when you switch to it (`load_recent_workspaces_does_not_write_probes`).
- `POST /api/workspace/validate` stays available in view-only mode, and works for paths outside the browse roots, as FR-872 specifies. It needs the dashboard token, returns only ok or a reason (never directory contents), and uses the same write probe as the CLI.
- The header path button shows a `▾` caret, so you can tell it opens the dialog.

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-870 | FR-870, FR-873 | unit | temp dirs, env and user config | Precedence order is honoured; missing and non-directory paths, `/`, the state dir, the config dir and `~/.ssh` are rejected with the documented reason; symlinks and `..` canonicalize | `src/workspace.rs` `workspace_dir_precedence_and_validation` |
| TC-871 | FR-870 | integration (CLI) | built binary, two temp dirs | `dmuxai -C <a> status --json` reports `<a>` while run from `<b>`; `-C /nonexistent` exits 2 with one line on stderr | `tests/cli.rs` `workspace_dir_flag_sets_workspace` |
| TC-872 | FR-871 | unit (asset) | index.html and JS | Header workspace button with `title` and `aria-haspopup="dialog"`; dialog markup with path input, recent list and browser; middle-ellipsis helper | `src/web/assets.rs` `workspace_dir_assets` |
| TC-873 | FR-872, FR-873, NFR-870 | integration | axum router, temp tree with files, dotdirs and a symlink to a denied dir | `dirs` lists child directories only, hides dotdirs, caps at 500, refuses denied paths and paths outside the browse roots, and drops a child symlink that points into a denied dir; `validate` reasons; auth required; view-only refuses `dirs` and `switch`; the token never appears in bodies | `src/web/server.rs` `workspace_api_lists_dirs_and_validates` |
| TC-874 | FR-871, FR-872 | e2e (ignored, headless Chrome) | `dmuxai --web` (pty) | The header shows the path; the dialog lists recent paths and subdirectories; an invalid path shows the reason inline; screenshot captured | `tests/web_workspace_dir.rs` `workspace_dialog_renders_in_headless_chrome` |
| TC-875 | FR-874 | integration | router with a fake busy turn | `switch` answers 409 while busy, 202 when idle (the re-exec hook is a test double), 403 view-only, and 501 on non-Unix (cfg-gated); after a 202 a new prompt gets 409; a failing preflight clears the flag and raises a notice; a failing exec double exits with code 1 and prints the restart command; the re-exec argv has exactly one `-C` (also when the original used `--workspace-dir=DIR`) | `src/web/server.rs` `workspace_switch_guards` |
| TC-876 | FR-874 | e2e (ignored, Unix) | `dmuxai --web` (pty) with a fixed token | After `switch` to a second temp dir, the same URL serves a snapshot whose `workspace` is the new path within 15s | `tests/web_workspace_dir.rs` `workspace_switch_reexecs_serve` |
| TC-877 | FR-875 | unit | custom agent with `command = ["pwd"]`-style probe, graph node runner | The spawned process cwd and `{workspace}` both equal the resolved workspace, not the test process cwd | `src/custom_agent.rs` `runner_cwd_is_workspace` |
