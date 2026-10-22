# SPEC: TUI stabilization, CLI headless ergonomics, and input reliability

Status: Draft  
Owner: DmuxAI  
Created: 2026-10-17  
Baseline: `73e3f4d`  
Related: dogfood #1, #4, #5, #6; TEST-PLAN §1.1, §2.6, §2.7, §2.8  

## 1. Decision summary

DmuxAI currently fails immediately with an opaque `Os { code: 6 }` error when stdout/stdin is not a TTY or when invoked with `--help` or `--version`, cuts off status bar keys on narrow terminals, blocks for ~20 seconds whenever F2 is pressed on AGY due to uncached subprocess discovery, and swallows the Enter key when submitted right after a pasted text burst. This specification establishes four stabilization capabilities: (1) CLI argument preprocessing for `--help`/`--version` (exit 0) and graceful non-TTY startup detection with human-readable guidance; (2) adaptive status line layout that drops low-priority slash command hints first, then abbreviates key bindings before truncating essential status indicators; (3) multi-tier caching (in-process memory + on-disk JSON with 24-hour TTL) for the AGY model list so F2 opens in under 200 ms with background refresh; and (4) bracketed paste mode support and input burst handling so that pressing Enter right after pasting immediately submits the prompt. All other outstanding gaps and warnings from TEST-PLAN and dogfood logs are cataloged with explicit boundaries.

## 2. Scope

### In scope

- Pre-initialization CLI argument inspection in `src/main.rs` for `--help` / `-h` and `--version` / `-V`, outputting to stdout and exiting with code 0 without touching TTY state or databases.
- Interactive terminal (TTY) verification before raw mode entry; reporting a clean error to stderr and exiting non-zero when stdout or stdin is not a TTY.
- Bracketed paste enablement (`EnableBracketedPaste`) and cleanup (`DisableBracketedPaste`) in `TerminalSession`.
- Input burst and paste event handling in `src/app.rs` ensuring Enter immediately submits buffered text without swallowing keys or injecting literal newlines.
- Dynamic status line formatting in `src/app.rs` respecting `area.width` with prioritized degradation: dropping slash hints, abbreviating key bindings, and protecting essential agent/model/activity information.
- In-memory and on-disk TTL model catalog caching for AGY in `src/agent/agy.rs` and `src/app.rs`, enabling instant F2 modal presentation and asynchronous background refresh.

### Out of scope / non-goals

The following table explicitly accounts for every other item, gap, and warning identified in [docs/TEST-PLAN.md](../TEST-PLAN.md) and [docs/dogfood/README.md](../dogfood/README.md):

| Gap / Item ID | Source | Description | Disposition and Rationale |
| --- | --- | --- | --- |
| FR-CLI-3 | TEST-PLAN §2.6 | Headless prompt execution (`dmuxai run --agent <a> "<prompt>"`) | Out of scope; non-interactive batch runner will be defined in a future CLI batch execution spec. |
| FR-PERM-1..3 | TEST-PLAN §2.1 | Implement turn failure on permission denial or missing writes | Out of scope; already specified in [SPEC-permission-hardening.md](SPEC-permission-hardening.md) and implemented. |
| FR-THEME-1..3 | TEST-PLAN §2.2 / dogfood #2 | Speaker color distinction and `NO_COLOR` / `DMUXAI_FORCE_COLOR` | Out of scope; already specified in [SPEC-speaker-colors.md](SPEC-speaker-colors.md) and implemented. |
| FR-MODEL-1..4 | TEST-PLAN §2.3 / dogfood #3 | Runner-aware model policies and F2 user model priority | Out of scope; already specified in [SPEC-model-policy.md](SPEC-model-policy.md) and implemented. |
| FR-CUSTOM-1..5 | TEST-PLAN §2.4 | Headless custom agent registration via `[[agents.custom]]` in `dmuxai.toml` | Out of scope; already specified in [SPEC-custom-agent-provider.md](SPEC-custom-agent-provider.md) and implemented. |
| FR-COMPACT-1..3 | TEST-PLAN §2.5 | `/compact` alias and `/summarize <focus>` command | Out of scope; already specified in draft [SPEC-compact-summarize-focus.md](SPEC-compact-summarize-focus.md). |
| FR-SKILL-1..2 | TEST-PLAN §2.9 | Frontmatter parsing, invalid SKILL.md skipping, and node prompt injection | Out of scope; deferred to a future dedicated skill integration spec. |
| FR-GRAPH-8..9, NFR-GRAPH-2 | TEST-PLAN §2.10 | `/graph override`, graph restart persistence/resume, and concurrency/retry caps | Out of scope; belongs to runtime graph orchestrator enhancements. |
| FR-WIN-1..3 | TEST-PLAN §2.11 | Windows launcher, WSL path conversion, and Windows clipboard image bridge | **Deferred (Windows dropped 2026-10-22).** Out of scope; tracked as release gate prerequisites for Windows WSL packaging. |
| NFR-REPORT-1 | TEST-PLAN §2.12 | Enforcing `docs/graph-runs/<topic>.md` report presence in CI | Out of scope; repository CI workflow automation task. |
| UX-QUIT-2 | TEST-PLAN §2.12 | Ctrl+C cancels running turn; idle notice (Esc already non-exiting) | Out of scope; separate keybinding and signal-handling consistency spec. |
| Dogfood #7 | dogfood README #7 | AGY Implement turns silently empty due to auto-denied actions | Out of scope; already resolved in PR #8 via `--dangerously-skip-permissions`. |
| Dogfood #8 | dogfood README #8 | AGY long Implement turn hangs on background compilation command | Out of scope; runner timeout and process supervision topic for provider adapters. |
| Dogfood #9 | dogfood README #9 | SQLite DB schema CHECK constraint limiting agent names to built-ins | Out of scope; already resolved in PR #8 (v2 schema rebuild). |
| Dogfood #10 | dogfood README #10 | Syntax error in `dmuxai.toml` silently dropped without notice | Out of scope; already resolved in PR #8 (startup notice emitted). |
| Dogfood #11 | dogfood README #11 | Raw string test fixture break on `"#` and substitution re-scanning | Out of scope; already resolved in PR #8 code review. |
| TC-045 ⚠️ | TEST-PLAN §1.1 | `SPEC-codex-effort-tool-usage` missing named automated test | Out of scope; maintenance gap in Codex test suite. |
| TC-001..007 ⚠️ | TEST-PLAN §1.1 | `SPEC-automatic-cross-agent-sharing` missing concrete test names | Out of scope; traceability maintenance in cross-agent spec. |
| UX-GRAPH-1 / NFR-GRAPH-1 ⚠️ | TEST-PLAN §1.1 | `SPEC-agent-graph-orchestration` manual-only evidence and missing TC table | Out of scope; graph orchestration spec formatting maintenance. |
| SPEC-image-attachment ⚠️ | TEST-PLAN §1.1 | `SPEC-image-attachment` missing structured TC acceptance table | Out of scope; image attachment spec template migration. |
| Spec ID collisions ⚠️ | TEST-PLAN §1.1 | Overlapping TC numbers across different spec files | Out of scope; addressed across repository by using domain-scoped IDs (such as `FR-380..` and `TC-380..`). |

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-380 | Should disk caching of models apply to all runners or AGY only? | DmuxAI | Implementation |
| *Resolution* | Disk caching architecture is designed generically for any runner whose discovery is an expensive CLI subprocess, but initialized and verified first for AGY where the 20-second delay is observed. | DmuxAI | Accepted |
| OQ-381 | Where should the on-disk model cache file reside? | DmuxAI | Implementation |
| *Resolution* | Under the user state/cache directory (`$XDG_STATE_HOME/dmuxai/model-cache/`), falling back to `.dmuxai/cache/` within the workspace if state home is unavailable. | DmuxAI | Accepted |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| TTY | An interactive terminal character device attached to stdin and stdout. | A piped stream or redirected file descriptor. |
| Bracketed paste | Terminal control sequence mode (`\x1b[?2004h`) where pasted input is bracketed between `\x1b[200~` and `\x1b[201~`. | Normal keystroke streaming. |
| Paste burst | Rapid arrival of key events over a short time window without intervening user typing pauses. | Bracketed paste event or single slow keypress. |
| Model cache TTL | Time-to-live duration (24 hours) after which a cached model catalog is treated as stale. | Invalidation on process restart. |
| Essential status | Core runtime state that must always remain visible: active agent, state badge, activity, model, mode, effort. | Informational hints and key reminders. |

```text
[No TTY detected] ──> Print error to stderr ──> Exit 1
[--help / --version] ──> Print to stdout ──> Exit 0
[Interactive TTY] ──> Initialize terminal & bracketed paste ──> Run TUI App ──> Restore terminal on Exit
```

```text
F2 Pressed (AGY)
  │
  ├──> In-process memory catalog present? ──Yes──> Open Picker Immediately (<200ms)
  │                                                   │
  └──No──> Disk cache valid & not expired? ──Yes──────┤
             │                                        │
             └──No──> Display loading / stale items ──┴──> Spawn Background agy models
                                                                 │
                                                            Update Memory + Disk Cache
```

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-380 | Must | Given command line arguments containing `--help` or `-h`, when `dmuxai` is executed in any environment (TTY or non-TTY piped). | It must print CLI usage and available options to `stdout` and exit with code 0 without initializing terminal raw mode, alternate screen, or database connections. | If writing to stdout fails (e.g. broken pipe), exit with a standard IO error code. | TC-380 |
| FR-381 | Must | Given command line arguments containing `--version` or `-V`, when `dmuxai` is executed in any environment (TTY or non-TTY piped). | It must print the program name and version (e.g. `dmuxai <version>`) to `stdout` and exit with code 0 without initializing terminal raw mode, alternate screen, or database connections. | If stdout is broken, exit with standard IO error code. | TC-381 |
| FR-382 | Must | Given standard input or standard output is not an interactive terminal (TTY) and no help/version flags are passed, when `dmuxai` is launched. | It must exit immediately with exit code 1 and print a clear error message to `stderr` stating that an interactive terminal is required, instead of panicking or emitting `Os { code: 6, ... }` ("No such device or address"). | Error message must suggest running with `--help` for available command options. | TC-382 |
| FR-383 | Must | Given the TUI is rendering the status bar, when the terminal width (`area.width`) is constrained. | The status line must be formatted into a single line that fits entirely within `area.width` without wrapping, horizontal clipping, or losing essential state fields (agent name, agent state, model, mode). | If terminal width is smaller than the minimum essential fields (<40 columns), prioritize agent badge and mode. | TC-383, TC-384 |
| FR-384 | Must | Given the status line needs to reduce length to fit within `area.width`, when formatting elements for display. | Status items must be discarded in strict order of ascending priority: (1) slash command suggestions (`/effort`, `/status`, `/mode`, `/agent`, `/graph`, `/quit exit`) must be dropped first; (2) secondary navigation key hints (`F3 chats`, `F2 model`) dropped next; (3) critical key hints (`Enter send`, `Ctrl+C cancel`, `Esc close`) preserved longest. | Essential runtime status (agent, state, model, effort, mode, activity) must remain visible while any hints are present. | TC-383 |
| FR-385 | Must | Given a very narrow terminal width where dropping slash hints is insufficient, when formatting remaining key hints and long model/activity names. | Key hints must be abbreviated to concise forms (`Enter send` → `Enter`, `Ctrl+C cancel` → `Ctrl+C`, `Esc close` → `Esc`, `F2 model` → `F2`), and long model labels or activity descriptions truncated with an ellipsis (`…`) before essential badges are omitted. | At least the active agent badge and mode indicator must remain rendered on any width ≥ 30 columns. | TC-384 |
| FR-386 | Must | Given an agent whose model list requires subprocess execution (such as AGY) has loaded models during the current process lifetime, when the user presses F2. | The model picker modal must open immediately (<200 ms) populated with the in-process cached models without blocking the UI event loop or re-spawning the discovery subprocess synchronously. | If the in-process cache is marked for refresh, the modal still opens immediately with existing items while refresh proceeds. | TC-385 |
| FR-387 | Must | Given a fresh process launch with an existing valid on-disk model cache file whose age is within the TTL (24 hours), when the user presses F2 on AGY for the first time. | The models must be loaded from disk and displayed in the picker immediately (<200 ms) without waiting for `agy models` CLI execution. | If the disk cache file is corrupt, unreadable, or missing, fall back to background discovery and display the loading indicator. | TC-386 |
| FR-388 | Must | Given the on-disk cache for AGY is expired (>24 hours) or missing, when F2 is pressed or an asynchronous model refresh is initiated. | The model discovery subprocess must execute asynchronously in the background without freezing the UI thread or keyboard input handling; once finished, memory and disk caches must update atomically. If existing cached models exist, they remain browsable during background refresh. | If the subprocess fails or returns non-zero, existing cached models must not be cleared and a non-fatal warning notice must be shown. | TC-387, TC-388 |
| FR-389 | Must | Given a rapid burst of characters in the prompt input (such as piped PTY text or fast paste) followed immediately by an `Enter` key event, when events are processed by the application loop. | The application must immediately trigger prompt submission (`submit_prompt`), dispatching the accumulated prompt text to the active agent without swallowing the `Enter` key or requiring a second keypress. | If the prompt input is empty or pure whitespace, submission is a no-op as normal. | TC-389 |
| FR-490 | Must | Given the TUI terminal session initializes, when configuring terminal device features. | The session must enable bracketed paste mode (`EnableBracketedPaste`) and disable it (`DisableBracketedPaste`) on shutdown/drop. Pasty bursts and newline characters within single-line prompts must be normalized such that an Enter key event triggers submission rather than appending unhandled control characters. | When raw `\n` or `\r` arrives as part of a single-line prompt submission, it must trigger submission rather than corrupting the input buffer. | TC-490, TC-491 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-380 | FR-380 | integration | Subprocess invocation with piped stdout and stdin (non-TTY). | Run `dmuxai --help` and `dmuxai -h`. | Exit code is 0; stdout contains usage summary and command flags; no raw terminal or database files created. | `tests/cli.rs` (`help_flag_prints_usage_without_tty`); `src/main.rs` (`parse_cli_help_flags`) |
| TC-381 | FR-381 | integration | Subprocess invocation with piped stdout and stdin (non-TTY). | Run `dmuxai --version` and `dmuxai -V`. | Exit code is 0; stdout matches `dmuxai <version>`; exits cleanly without terminal escape sequences. | `tests/cli.rs` (`version_flag_prints_version_without_tty`); `src/main.rs` (`parse_cli_version_flags`) |
| TC-382 | FR-382 | integration | Subprocess invocation with piped stdout/stdin (stdin null) and no flags or unknown flag. | Run `dmuxai`, or run `dmuxai --unknown-flag`. | No flags exits code 1 with stderr "dmuxai needs an interactive terminal (TTY); run it in a terminal, or see dmuxai --help" (no Os code 6); unknown flag exits code 2 with error and usage. | `tests/cli.rs` (`non_tty_start_fails_with_clear_message`, `unknown_flag_exits_with_code_2`); `src/main.rs` (`parse_cli_unknown_flags`, `parse_cli_empty_args_defaults_to_run`) |
| TC-383 | FR-383, FR-384 | unit | TestBackend / pure function with widths of 80, 100, 120, and 200 columns. | Render status line with active agent, model, mode, and activity. | At width 120 and 80, slash hints (`/effort`, `/status`, `/mode`, `/agent`, `/graph`) are absent; core fields (agent, state, model, mode) and `Esc`/`Enter` hints remain within width without wrapping. | `src/app.rs` (`fit_status_segments_at_widths_40_80_200`) |
| TC-384 | FR-384, FR-385 | unit | Pure function with constrained terminal width (40 columns). | Format status line with `fit_status_segments`. | Key hints are dropped/abbreviated; model/mode compactly formatted; rendered length ≤ 40 columns; agent and state badge remain visible. | `src/app.rs` (`fit_status_segments_at_widths_40_80_200`) |
| TC-385 | FR-386 | unit | App with AGY model catalog already populated in memory. | Trigger `open_model_picker()` (F2). | Picker modal state is `Some(ModelPicker)`; models match existing list; cached models delivered instantly. | `src/agent/agy.rs` (`runtime_loop` memory cache delivery) |
| TC-386 | FR-387 | unit | Disk cache file containing valid JSON model list and recent timestamp (<24h). | Read disk cache with `read_disk_cache`. | Models loaded directly from disk cache and marked fresh; no subprocess blocking needed. | `src/agent/agy.rs` (`test_disk_cache_roundtrip_and_freshness`) |
| TC-387 | FR-388 | unit | Disk cache file older than 24h (expired TTL). | Read disk cache with `read_disk_cache`. | Models returned with `is_fresh = false` triggering asynchronous background refresh; fresh models written to disk. | `src/agent/agy.rs` (`test_disk_cache_roundtrip_and_freshness`) |
| TC-388 | FR-388 | unit | Disk cache file is missing or corrupt. | Call `read_disk_cache`. | Corrupt or non-existent file returns `None` safely without panic. | `src/agent/agy.rs` (`test_disk_cache_missing_or_corrupt`) |
| TC-389 | FR-389, FR-490 | unit | Prompt input populated by rapid sequence of key events (600 characters) without delay. | Dispatch `KeyCode::Enter` or `KeyCode::Char('\n')` immediately after burst. | `submit_prompt()` executes; input buffer is cleared; prompt is submitted to the active agent; prompt text does not remain in the input bar. | `src/app.rs` (`enter_immediately_after_paste_burst_submits_prompt`, `char_newline_key_submits_prompt`) |
| TC-490 | FR-490 | unit | Terminal session initialization. | Execute `TerminalSession::start()` and drop session guard. | Terminal commands emitted include `EnableBracketedPaste`; drop commands include `DisableBracketedPaste`. | `src/main.rs` (`TerminalSession`) |
| TC-491 | FR-490 | unit | Event loop receiving `Event::Paste(text)` ending in newline or followed by Enter. | Process paste event via `handle_paste`. | Text is accepted and submitted cleanly on Enter or trailing newline without leaving newline characters in input. | `src/app.rs` (`paste_with_trailing_newline_submits_immediately`, `enter_immediately_after_paste_burst_submits_prompt`) |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | On-disk model cache stored as JSON: `{"timestamp": <epoch_secs>, "models": [{"id": "...", "label": "..."}]}` under state directory. | If the cache file cannot be parsed or schema changes, it is cleanly discarded and regenerated from the provider CLI. | TC-386, TC-387 |
| CLI / Protocol | Non-TTY CLI invocation prints plain text usage or version to `stdout` and exits 0. Non-TTY startup prints plain text to `stderr` and exits 1. | Fully backward compatible; does not alter existing interactive terminal experience. | TC-380, TC-381, TC-382 |
| Security / permissions | Non-TTY startup rejection prevents headless background processes from inadvertently acquiring terminal control or hanging. | Prevents silent zombie processes in automated scripts. | TC-382 |
| Platform | Standard `std::io::IsTerminal` checks for stdin/stdout across Linux, WSL included. Bracketed paste sequences supported across ANSI/VT100 terminals. | Falls back gracefully if terminal does not support bracketed paste. | TC-382, TC-490 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add CLI flag argument parser (`--help`, `-h`, `--version`, `-V`) and TTY guard before terminal initialization. | `src/main.rs`, `src/lib.rs` | FR-380, FR-381, FR-382 | TC-380, TC-381, TC-382 |
| 2 | Enable and disable bracketed paste mode in terminal session RAII guard. | `src/main.rs` | FR-490 | TC-490 |
| 3 | Implement responsive status line layout dropping low-priority slash command hints and abbreviating key hints. | `src/app.rs` | FR-383, FR-384, FR-385 | TC-383, TC-384 |
| 4 | Add persistent disk caching and in-process caching for AGY model discovery with 24-hour TTL and non-blocking background refresh. | `src/agent/agy.rs`, `src/app.rs`, `src/settings.rs` | FR-386, FR-387, FR-388 | TC-385, TC-386, TC-387, TC-388 |
| 5 | Fix paste burst handling and Enter swallowing in event loop, ensuring immediate submission. | `src/app.rs` | FR-389, FR-490 | TC-389, TC-491 |

## 8. Review and test exit criteria

- [ ] All Must requirements (FR-380 through FR-389, FR-490) map to passing test scenarios (TC-380 through TC-389, TC-490 through TC-491).
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] Dogfood bugs #1, #4, #5, and #6 are verified fixed and closed in `docs/dogfood/README.md`.
- [ ] `bash scripts/check-spec.sh docs/specs/SPEC-tui-stabilize.md` passes with zero errors.
