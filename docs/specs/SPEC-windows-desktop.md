# SPEC: Windows desktop launcher (DmuxAI inside WSL)

Status: In progress — steps 1–2 implemented (launcher core, config, `dmuxai-launcher` bin); step 3 packaging (zip/MSI, Start-menu shortcut) and live W-rows pending
Owner: DmuxAI
Created: 2026-10-17
Baseline: `bcdb0eb`
Related: MISSION.md M6 ("Windows ships as a launcher that runs DmuxAI inside WSL"),
[TEST-PLAN.md §2.11 and §4 release gate](../TEST-PLAN.md),
[ANALYSIS.md §2.5 and §3](../ANALYSIS.md),
[SPEC-clipboard-image-paste](SPEC-clipboard-image-paste.md)

## 1. Decision summary

Windows users get a desktop-app experience: a Start-menu entry (and a `dmuxai.exe`
launcher) that opens Windows Terminal running the **Linux** DmuxAI binary inside
WSL with a Linux canonical workspace path. The launcher performs first-run checks
with exact fix-it commands, converts Windows paths to WSL paths, warns about
`/mnt/c` performance, and relays the Windows clipboard image bridge. Packaging and
update stay per-user and require no Rust, Cargo, or Git on Windows. This follows
the mission platform decision (M6) and resolves the Cursor Implement block that
the native Windows build cannot lift (ANALYSIS §2.1, §3).

## 2. Scope

### In scope

- Start-menu shortcut / `dmuxai.exe` launcher on Windows that starts Windows
  Terminal with `wsl.exe -d <distro> --cd <linux path> --exec sh -lc "exec dmuxai"` (login shell so ~/.cargo/bin is on PATH; fixed string, no user text)
- Distro and workspace selection, remembered between runs
- First-run checks: WSL installed, distro present, `dmuxai` present inside the
  distro, and agy/cursor-agent/opencode presence — each with a clear fix-it message
- Windows path → WSL path conversion and the `/mnt/<drive>` performance warning
- Clipboard image paste from Windows (PowerShell `Get-Clipboard` bridge) into the
  WSL-running TUI
- Packaging (zip, optional MSI) and per-user update

### Out of scope / non-goals

- Changing the provider adapters, SQLite schema, graph engine, or any Linux TUI
  behavior (the WSL side runs the unchanged Linux `dmuxai`)
- Running a native Windows Rust build of DmuxAI differently than today; the
  native `dmuxai.exe` remains available and unchanged
- Unsupandboxed Cursor on Windows (native path guard INT-002/NFR-060 unaffected)
- tmux pane orchestration and skills UI (separate specs; ANALYSIS §3 medium-term)
- Reading or modifying `\\wsl$` UNC paths; the Linux path is always canonical

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | Proposed TC-* tests marked *proposed* have no code yet; they must exist before this spec becomes Implemented. | DmuxAI | Spec approval |
| OQ-002 | Where the WSL-side package/auth state (e.g. `cursor-agent login`) is checked: presence only in the launcher, login state is left to the CLIs' own errors. Confirm before Must tests are written. | DmuxAI | Spec approval |
| OQ-003 | MSI vs zip for the first release: zip is the Must, MSI is a Should; confirm MSI tooling (e.g. WiX) availability in CI before FR-397 evidence. | DmuxAI | Before FR-397 test execution |

No implementation starts while a Must requirement depends on an unanswered
question.

## 3. Terms and state model

- **Launcher**: the Windows-side executable and Start-menu entry that prepares and
  starts the WSL session. It never runs the providers itself.
- **Fix-it message**: an error screen naming the one exact command that resolves
  the detected problem (install command, login command, or reinstall link).
- **Canonical workspace**: the Linux absolute path (`/home/...` or `/mnt/...`)
  that WSL-side `dmuxai` uses as `current_dir`; it is the SQLite workspace key.

| Launch state | Meaning | Exit / next state |
| --- | --- | --- |
| checking | first-run checks are being run | → blocked on first failure with a fix-it message, or → launching |
| launching | `wt.exe`/`wsl.exe` child started | terminal opens in the distro at the canonical path |
| launched | TUI session handed to WSL `dmuxai` | terminal state |
| blocked | check failed | user runs the fix-it command and starts the launcher again |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-390 | Must | User starts DmuxAI from the Start-menu shortcut or by running `dmuxai.exe` on Windows. | Windows Terminal opens running `wsl.exe -d <distro> --cd <linux path> --exec sh -lc "exec dmuxai"` (login shell so ~/.cargo/bin is on PATH; fixed string, no user text); the TUI appears with the converted workspace. | If Windows Terminal is not installed, the launcher starts `wsl.exe` directly in the default console and shows a notice naming the `wt.exe` install command, without blocking. | TC-390, TC-391 |
| FR-391 | Must | The distro is chosen on first run (or via re-selection). | With one WSL distro it is used without prompting; with several, an explicit picker lists `wsl --list` names; the choice is remembered and reused silently on later launches. | An empty distro list behaves like the missing-distro check (FR-393) instead of launching. | TC-392, TC-393 |
| FR-392 | Must | A Windows workspace path is given (launcher argument, last-used, or picked folder). | The launcher converts it to a Linux path inside the target distro (`wslpath -u`) before launch and starts `dmuxai` with that path as its working directory; the Linux path is the persisted workspace key. | If the path does not exist inside the distro (drive not mounted, folder deleted), the launcher shows a fix-it message and does not start the TUI; `\\wsl$`/`\\wsl.localhost` forms are never emitted back to the user. | TC-394 |
| FR-393 | Must | First run, or a previous launch was blocked. | Checks run in order — WSL installed, distro present, `dmuxai` present inside the distro — and each failure stops launch with a fix-it message naming the exact remediation command. Missing agent CLIs (agy, cursor-agent, opencode) are reported as one non-blocking list with per-CLI fix-it commands; launch continues. | The blocked list is comprehensive for one pass: after fixing one item, the next missing item is reported on the next attempt, never all-at-once-then-success ambiguity. | TC-395, TC-396, TC-397 |
| FR-394 | Must | The converted workspace path starts with `/mnt/`. | The launcher shows a performance/ownership warning that names the slow path and suggests an ext4 clone (for example `git clone` under `~/projects/`), and requires an explicit continue decision. | Without confirmation the launcher stops before `wt.exe` is started; an explicit remembered "proceed on `/mnt/`" choice suppresses repeats. | TC-398 |
| FR-395 | Must | The user presses Ctrl+V (or Alt+V where the terminal uses it) with a PNG image in the Windows clipboard during a WSL TUI session. | The image is attached (`[Image #N]` chips), saved under `.dmuxai/attachments/`, and paths are forwarded to the provider exactly as on Linux (existing clipboard fallback chain including the Windows `powershell.exe` bridge). | If no image decodes, the paste is a no-op notice; drag/drop or file paths keep working; interactive pass-through behavior stays governed by the image spec (M5). | TC-399 |
| FR-396 | Must | The user installs from the published release zip. | Per-user install places the launcher + uninstaller, creates a Start-menu shortcut for the current user, and requires no Rust, Cargo, or Git on the Windows machine. | Uninstall removes the shortcut and installed files; a second install overwrites cleanly instead of duplicating entries. | TC-400 |
| FR-397 | Should | The user installs from the MSI. | Same per-user scope and shortcut result as FR-396; upgrading over an existing install replaces the launcher in place. | If MSI packaging is not wired in CI, the release ships zip only and the MSI is documented as not yet available. | TC-401 |
| FR-398 | Must | The user runs the installer/launcher again after a release. | The previous launcher version is replaced; if the distro-side `dmuxai` version is older than the launcher's minimum floor, a fix-it message names the exact update command instead of starting a known-incompatible session. | Version detection failure (no `--version` support yet on older builds) falls back to a "cannot verify version" notice and still launches, rather than guessing compatibility. | TC-402 |
| NFR-390 | Must | Any launcher execution of Windows-side commands. | All child processes are launched as fixed argv arrays (`wsl.exe`, `wslpath`, `wt.exe` with separate arguments); workspace text or distro names are never embedded into `cmd.exe`/PowerShell command strings. | Paths with spaces, quotes, `&`, or parentheses are passed through byte-for-byte as one argument each. | TC-403 |
| UX-390 | Should | Any fix-it message or check list is shown. | Messages show object (what is missing), reason, and one copyable command line, fitting one screen without scrolling. | Long CLI name lists wrap per item instead of truncating silently. | TC-404 |

## 5. Acceptance scenarios and test design

Unit tests run the launcher logic with a fake `wsl.exe`/`wslpath` probe so no WSL
is needed in CI. Live/WSL checks are manual and recorded in TEST-PLAN §3 (W-rows).

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-390 | FR-390 | unit | stubbed environment with `wt.exe` present; distro `Ubuntu`; workspace `C:\src\p` → `/home/u/p` | build the launch command | argv is exactly `wt.exe new-tab wsl.exe -d Ubuntu --cd /home/u/p --exec sh -lc "exec dmuxai"` (login shell so ~/.cargo/bin is on PATH; fixed string, no user text) (no shell string) | `launcher_builds_windows_terminal_command` (src/launcher/mod.rs) |
| TC-391 | FR-390 | unit | `wt.exe` absent | build the launch command | argv is `wsl.exe -d ... --cd ... --exec sh -lc "exec dmuxai"` (login shell so ~/.cargo/bin is on PATH; fixed string, no user text) and a notice naming the Windows Terminal install source is recorded | `launcher_falls_back_without_windows_terminal` (src/launcher/mod.rs) |
| TC-392 | FR-391, FR-392 | unit | launcher config with remembered distro and last workspace | launch | remembered values are used without prompting; corrupt/unreadable config behaves as "not remembered" | `launcher_remembers_distro_and_workspace` (src/launcher/mod.rs) |
| TC-393 | FR-391 | unit | two distros listed, none remembered | first launch | picker lists both distros; the selected one is persisted and reported | `launcher_prompts_when_distro_ambiguous` (src/launcher/mod.rs) |
| TC-394 | FR-392 | unit | input `C:\src\my project` and a fake `wslpath` | convert | result is `/mnt/c/src/my project` round-tripped through the fake (spaces preserved); a `\\wsl$` input raises the never-reemit error path | `windows_path_maps_to_wsl_path` (src/launcher/mod.rs) |
| TC-395 | FR-393 | unit | `wsl.exe --status` reports not installed | first-run checks | blocked with fix-it naming `wsl --install`, exit code non-zero, TUI not started | `launcher_reports_missing_wsl` (src/launcher/mod.rs) |
| TC-396 | FR-393 | unit | distro present, `dmuxai` probe returns not-found | first-run checks | blocked with fix-it naming the in-distro install command | `launcher_reports_missing_dmuxai_in_distro` (src/launcher/mod.rs) |
| TC-397 | FR-393 | unit | `agy` and `cursor-agent` present, `opencode` missing | first-run checks | one non-blocking notice listing `opencode` with its fix-it command; launch proceeds | `launcher_reports_missing_agent_clis_non_blocking` (src/launcher/mod.rs) |
| TC-398 | FR-394 | unit | converted path `/mnt/c/src/p` | pre-launch gate | performance warning shown; launch denied without confirm; `--allow-mnt` remembered confirm suppresses repeats | `mnt_path_shows_performance_warning` (src/launcher/mod.rs) |
| TC-399 | FR-395 | unit + manual | Windows clipboard contains PNG; session inside WSL | Ctrl+V paste | decode→attach→save chain: reuse existing tests `base64_powershell_output_decodes_to_png`, `clipboard_paste_registers_pending_image_chip`, `clipboard_paste_without_png_is_a_noop`; WSL end-to-end recorded as TEST-PLAN §3 W-row (live-TUI) | `cargo test base64_powershell_output_decodes_to_png`; live checklist W-03 |
| TC-400 | FR-396 | integration (manual on windows-latest) | published zip on a clean Windows user profile | install, then uninstall | shortcut exists and points at the installed launcher; uninstall removes shortcut and files; no build tools required | manual: TEST-PLAN §3 W-row (installation) |
| TC-401 | FR-397 | manual | MSI on a clean profile, then again over an existing install | install, upgrade | per-user shortcut present; upgrade replaces the launcher in place with a single entry | manual: TEST-PLAN §3 W-row (only after OQ-003 resolves) |
| TC-402 | FR-398 | unit | distro `dmuxai --version` output older than the launcher floor | launch | update fix-it message with the exact command; if the version probe fails, the "cannot verify version" notice path is used and launch continues | `launcher_detects_stale_wsl_dmuxai_version` (src/launcher/mod.rs) |
| TC-403 | NFR-390 | unit | workspace text containing `"& ( )` and spaces | build every launcher command | each command is one argv list; the hostile text stays inside a single argument and never reaches `cmd.exe`/PowerShell | `launcher_never_assembles_shell_command_string` (src/launcher/mod.rs) |
| TC-404 | UX-390, FR-393 | manual live | real WSL machine, at least one missing item | first-run checks | every shown message fits one screen with a copyable fix-it command; fixes resolve in one retry per FR-393 | live checklist W-0x rows in TEST-PLAN §3 |

Required coverage prompts: happy path (TC-390, TC-392, TC-394, TC-400),
boundary (TC-391 fallback, TC-393 empty/ambiguous distro list, TC-398 confirm
gate, TC-403 primitive characters), failure (TC-395, TC-396, TC-397, TC-402),
state transition (blocked → fixed → launched across repeated first-run passes,
TC-404), isolation (launcher config damage cannot corrupt the DmuxAI SQLite
database or another workspace's keys — config is read-only input to the launch;
verified in TC-392's corrupted-config branch).

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | New launcher-side config file only (`%LOCALAPPDATA%\DmuxAI\launcher.json`: distro, last workspace, `/mnt/` confirm). No DmuxAI SQLite schema change; the WSL-side DB keeps the Linux workspace key as today. | Deleting the config resets first-run state; WSL-side data rollback is unchanged (SQLite) | TC-392 |
| Provider protocol | No change — WSL `dmuxai` speaks the existing Codex/Cursor/AGY protocols. | Rollback = stop using the launcher; the native Windows exe remains | TC-390 |
| Security / permissions | Fixed argv for every Windows-side child process; per-user install; no elevated permissions; `\\wsl$` paths never reintroduced; unsandboxed Cursor rules in baseline INT-002/NFR-060 continue to hold inside WSL sandboxes. | Must not weaken sandbox or shell-avoidance guarantees | TC-403 |
| Platform | Windows launcher component only; Linux flow untouched. Native Windows `dmuxai.exe` and the launcher can coexist, but their conversation histories stay separate (Windows-key vs Linux-key workspaces, ANALYSIS §2.5) — the launcher notice states this once. | Fall back to the native Windows exe at any time | TC-404 |

## 7. Implementation plan

No plan or tasks file exists yet; create `../plans/PLAN-windows-desktop.md` and
`../tasks/TASKS-windows-desktop.md` before implementation starts (scheduled
after the tui-stabilize merge). Ordered skeleton for that plan (requirement
IDs only, no new behavior):

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Launcher core library + fake-probe unit tests (state model, argv builder, checks) | new `launcher/` crate (or `src/bin/dmuxai-launcher.rs`) | FR-390..394, FR-398, NFR-390 | TC-390..398, TC-402, TC-403 |
| 2 | Launcher config persistence | launcher config module | FR-391, FR-392, FR-394 | TC-392, TC-393, TC-398 |
| 3 | Release packaging (zip, MSI behind OQ-003) | CI release workflow additions | FR-396, FR-397 | TC-400, TC-401 |
| 4 | W-rows in TEST-PLAN §3 checklist and first live run | `docs/TEST-PLAN.md` | FR-395, UX-390 | TC-399, TC-404 |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence (TC-390..TC-403 must include at least the *proposed* automations once they exist).
- [ ] The diff was reviewed against scope and non-goals (no Linux-side behavior change; no provider protocol change).
- [ ] Required Rust gates pass: `cargo fmt --check`, `cargo test --all-targets`, `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] Manual W-rows (TC-400, TC-404; TC-401 if MSI ships) are recorded with machine, Windows build, distro, and dates; skipped is never reported as passing.
- [ ] `../reviews/REVIEW-windows-desktop.md` records commands, results, residual risk (e.g. `wslpath` output variance across distros), and rollback notes.
