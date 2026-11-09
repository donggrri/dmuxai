# SPEC-terminal-sessions: Persistent named terminal sessions (`dmuxai term`), PR1a

Status: in progress (PR1a implemented in tree: Linux PTY shell/cmd sessions, session host, CLI, journal; PR1b–PR4 pending)
Owner: DmuxAI Team
Created: 2026-11-09
Baseline: main 4c59eac (v0.3.1); highest existing FR/TC-2567, NFR-1886. IDs 2600–2699 are reserved for terminal sessions; PR1a defines FR/TC-2600–2609, 2620, 2621, 2632, NFR/TC-2690–2693, INT/TC-2695.
Related: #301 (overhead budget), #228 (invisible long-running command), #230 (tree kill), #194 (Display errors), SPEC-run-journal, SPEC-process-supervisor, SPEC-agent-api

## 1. Decision summary

Agents and humans need long-lived terminals (a shell, an `ssh` session, later a serial console) that survive a single CLI call, so a test node can start a command, come back for its output, and be watched by a human. dmuxai adds named sessions owned by a per-workspace **session host** and driven by `dmuxai term …` subcommands with `--json`. Completion is detected by an output regex, an idle period, or (for POSIX shells) a nonce exit marker. Every input and output event is written, masked, to a local journal. PR1a ships Linux PTY sessions; serial, the dangerous-command guard, agent integration, the dashboard view, and Windows follow in PR1b–PR4.

## 2. Scope

### In scope

- `dmuxai term open/send/read/wait/exec/list/show/resize/close/reopen`, all with `--json`.
- A session host process that autostarts on the first `term` call, outlives the CLI, and exits when idle.
- Linux PTY sessions of kind `shell` (default `$SHELL -l`, else `/bin/sh`) and `cmd` (`-- <argv>`).
- Per-session ring buffer with monotonic sequence numbers; regex, idle, and timeout waits.
- A masked, local, 0600 JSONL journal of all I/O per session.
- A human-takeover lock that rejects non-human input (the dashboard toggle itself is PR3).

### Out of scope / non-goals

- Native serial (`--serial`), WSL `usbipd attach` automation, and serial errors: PR1b.
- Dangerous-command block-and-approve with a per-workspace allowlist, and `term approve`: PR1b.
- `DMUXAI_RUN_ID`/`DMUXAI_RUNNER` runner env, prompt block, `[terminal] allowed_runners` enforcement, `term log`: PR2.
- Dashboard light text view, takeover toggle, approval banner: PR3. No xterm.js.
- Native Windows (ConPTY, COM, named pipes): PR4.
- A sandbox. A session runs a real shell with the user's rights.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2600 | Dangerous commands: block until approved, per-workspace allowlist (decided 2026-11-09, PR1b). | Dongjin | Closed |
| OQ-2601 | Takeover fully locks agent input until release (decided 2026-11-09). | Dongjin | Closed |
| OQ-2602 | Dashboard view is light text, no xterm.js (decided 2026-11-09, PR3). | Dongjin | Closed |
| OQ-2603 | Serial opens the device directly; dmuxai runs `usbipd attach --wsl` itself when the device is missing (decided 2026-11-09, PR1b). | Dongjin | Closed |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Session | A named, long-lived PTY child plus its output buffer and journal, unique per workspace. | A conversation/task session in the SQLite DB. |
| Session host | One background `dmuxai` process per workspace that owns all sessions and serves a Unix socket. | The web dashboard server. |
| Kind | `shell` (interactive POSIX shell, exit markers allowed) or `cmd` (any argv, no marker). `serial` is added in PR1b. | Runner kinds. |
| Seq | A per-session monotonic counter; each output chunk covers a seq range so readers neither miss nor repeat data. | Journal line numbers. |
| Marker | A per-call nonce line `__DMUXAI_<nonce>_<exit>__` appended by `exec` to learn the exit code; it is removed from returned output. | Prompt regexes. |
| Takeover | A per-session flag meaning a human holds the session; non-human input is rejected while it is set. | Plan approval. |

States: `starting → open → exited | closed`; `open ↔ takeover` (flag, input lock only); `lost` is reserved for serial (PR1b). `exited` carries `exit_code` or `signal`. `exited` and `closed` are terminal; `reopen` starts a new child with the same spec and name.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2600 | Must | When `term open <name> [--kind shell\|cmd] [--cwd DIR] [--env K=V]... [--cols N --rows N] [-- <argv>]` runs | A PTY child starts under the session host, `open` returns `{name, kind, state:"open", pid}`, and the session keeps running after the CLI exits. | A name not matching `[a-z0-9][a-z0-9-]{0,31}` exits 2; a name already open in the workspace exits 4; `--kind cmd` without argv exits 2. | TC-2600 |
| FR-2601 | Must | When any `term` subcommand runs and no host is listening | The host starts detached, listens on a per-workspace Unix socket in a 0700 directory with the socket at 0600, rejects peers with another uid, removes a stale socket, and exits after its last session ends plus an idle grace (default 600 s, `DMUXAI_TERM_IDLE_SECS` overrides). | If the socket path would exceed the platform limit, a short path under `$XDG_RUNTIME_DIR` or `/tmp/dmuxai-<uid>/` keyed by the repo key is used. A host that cannot start exits 6 with a Display message. | TC-2601 |
| FR-2602 | Must | When `term send <name> <text> [--no-newline]` or `term send <name> --keys K[,K...]` runs | The text plus `\n` (unless `--no-newline`) or the named keys (`C-a`..`C-z`, `Enter`, `Tab`, `Esc`, `Up`, `Down`, `Left`, `Right`, `Backspace`, `Space`, `F1`..`F12`) are written to the PTY, and the reply carries the seq at the time of the write. | More than 64 KiB per call exits 2; an unknown key name exits 2 naming it; a session that is not open exits 3 (unknown) or 4 (exited/closed). | TC-2602 |
| FR-2603 | Must | When `term read <name> [--since SEQ] [--tail N] [--raw]` runs | Output after `SEQ` (or the last N lines) is returned with `seq_from`, `seq_to`, and `dropped_bytes`; ANSI/control sequences are stripped unless `--raw`. | The buffer holds 1 MiB per session; data evicted before `SEQ` is reported in `dropped_bytes`, never silently skipped. | TC-2603 |
| FR-2604 | Must | When `term wait <name> (--match REGEX \| --idle DUR) --timeout DUR [--since SEQ]` runs | It returns as soon as stripped output after `SEQ` matches, or output has been quiet for `--idle`, with `reason: "match"` or `"idle"`, and `reason: "exited"` if the child exits first. | On timeout it exits 1 with `reason: "timeout"` and the output tail; an invalid regex or missing `--timeout` exits 2. | TC-2604 |
| FR-2605 | Must | When `term exec <name> <cmd> [--timeout DUR]` runs on a `shell` session | The command is sent followed by a nonce exit marker; the reply has the command output without the marker or its echo, `exit_code`, `reason: "marker"`, and `elapsed_ms`. | On a `cmd` session it sends and waits for idle and returns `exit_code: null` with `reason`; on timeout it exits 1 with `reason: "timeout"`. | TC-2605 |
| FR-2606 | Must | When `term list` or `term show <name>` runs | Each session reports name, kind, state, pid, created_ms, last_output_ms, cols, rows, takeover, exit_code/signal, and seq. | An unknown name in `show` exits 3. With no host running, `list` returns `[]` without starting one. | TC-2606 |
| FR-2607 | Must | When `term close <name> [--force]` or `term close --all` runs, or the child exits on its own | Close sends SIGHUP, then after a 2 s grace (0 with `--force`) kills the whole process group tree; state becomes `closed`. A child that exits by itself moves the session to `exited` with its code or signal. | Closing an unknown name exits 3; closing an already closed session is a no-op success. | TC-2607 |
| FR-2608 | Must | When `term resize <name> --cols N --rows N` runs | The PTY window size changes and the child sees it (`stty size`). | Values outside 1–1000 exit 2. | TC-2608 |
| FR-2609 | Must | While a session's takeover flag is set (host request `takeover`/`release`, dashboard in PR3) | Non-human `send`, `--keys`, and `exec` are rejected with exit 4, `error_kind: "conflict"`, and the message `session '<name>' is in human takeover; ask the user to release it`; `read`, `wait`, `show` keep working. | Release clears the flag; input works again immediately. | TC-2609 |
| FR-2620 | Must | For every input, key, resize, state change, and output chunk | A JSONL line `{seq, t_ms, dir, by, run_id, data, bytes}` is appended to `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/terms/<16hex>/io.jsonl` next to a `manifest.json` (schema_version 1); data is ANSI-stripped and passed through `mask_sensitive_tokens` before it is written; dirs 0700, files 0600, symlinks refused. | Output is coalesced into chunks of at most 50 ms / 8 KiB. A journal write error is reported in `show` as `journal_error` and does not kill the session. | TC-2620 |
| FR-2621 | Must | When `send --secret` is used, or the latest output matches `[Pp]assword:\|passphrase\|PIN:` | The journal records that input as `[REDACTED input]` and no reply echoes the typed text. | — | TC-2621 |
| FR-2632 | Must | When a session child starts | Its environment is scrubbed like runner children (`DMUXAI_WEB_TOKEN` and other dashboard secrets removed); `--env K=V` is added after scrubbing. | — | TC-2632 |
| NFR-2690 | Must | When `dmuxai run` (or any non-`term` command) runs with no session | No host is started and no term socket is created or opened. | — | TC-2690 |
| NFR-2691 | Should | Warm host on the box | `term send` + `term read` round trip p50 ≤ 30 ms, p95 ≤ 80 ms; the gate test asserts only a loose bound (p50 ≤ 90 ms). | — | TC-2691 |
| NFR-2692 | Should | `term exec` vs. `bash -c` | Overhead ≤ 10 % for commands ≥ 1 s and ≤ 50 ms for shorter ones (#301); gate test asserts a loose ×3 bound. | — | TC-2692 |
| NFR-2693 | Should | Idle host | No busy polling (idle CPU ≈ 0); one blocking reader thread per session. | — | TC-2693 |
| INT-2695 | Must | Any `term` subcommand with `--json` | stdout is exactly one JSON object; diagnostics go to stderr; errors carry `error_kind` and the exit codes of `agent_api::exit`. | — | TC-2695 |

## 5. Acceptance scenarios and test design

All integration tests spawn the built binary with isolated `HOME`/`XDG_*` dirs and a short `DMUXAI_TERM_IDLE_SECS`, close their sessions, and need no external tools beyond `/bin/sh`.

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2600 | FR-2600 | integration | temp workspace | open `a` (`/bin/sh`), open `a` again, open `Bad_Name` | first ok with pid, duplicate exit 4, bad name exit 2; the session is still alive after the CLI exits | `tests/term_cli.rs` |
| TC-2601 | FR-2601 | integration + unit | temp workspace | first `term open` | socket file mode 0600, dir 0700, host pid alive; long-path fallback chosen by unit test | `tests/term_cli.rs`, `src/term/client.rs` |
| TC-2602 | FR-2602 | unit + integration | open shell | send `echo hi`; send `--keys C-c`; send 65 KiB; send `--keys Nope` | output has `hi`; oversize and unknown key exit 2; key table maps C-c to 0x03 | `src/term/mod.rs`, `tests/term_cli.rs` |
| TC-2603 | FR-2603 | unit | ring buffer with small capacity | push past capacity, read since 0 | seq monotonic, `dropped_bytes` > 0, no duplicate data; ANSI stripped unless raw | `src/term/buffer.rs` |
| TC-2604 | FR-2604 | unit + integration | open shell | wait `--match done` after `sleep 0.2; echo done`; wait with a regex that never matches | reason `match`; never-matching exits 1 with `timeout` | `src/term/wait.rs`, `tests/term_cli.rs` |
| TC-2605 | FR-2605 | unit + integration | open shell | exec `true`, exec `false`, exec `printf x` | exit codes 0, 1, 0; output `x` without marker text | `src/term/wait.rs`, `tests/term_cli.rs` |
| TC-2606 | FR-2606 | integration | two sessions | list, show one, show unknown, list with no host | fields present; unknown exit 3; empty list starts no host | `tests/term_cli.rs` |
| TC-2607 | FR-2607 | integration | shell running `sleep 100 &` child | close; separately `exit 3` inside a shell | process group gone, state closed; self-exit gives state exited, exit_code 3 | `tests/term_cli.rs` |
| TC-2608 | FR-2608 | integration | open shell | resize 100x30, exec `stty size` | output `30 100`; resize 0 exits 2 | `tests/term_cli.rs` |
| TC-2609 | FR-2609 | unit + integration | open shell with takeover set via host request | send, exec, read; release; send | send/exec exit 4 with the message; read ok; after release send ok | `src/term/host.rs`, `tests/term_cli.rs` |
| TC-2620 | FR-2620 | unit + integration | open shell | send `echo sk-test-AAAAAAAAAAAAAAAAAAAAAAAA` | io.jsonl has in/out lines, token masked, modes 0600/0700 | `src/term/journal.rs`, `tests/term_cli.rs` |
| TC-2621 | FR-2621 | unit | journal writer | `--secret` input; input after `Password:` output | journal line data is `[REDACTED input]` | `src/term/journal.rs` |
| TC-2632 | FR-2632 | integration | `DMUXAI_WEB_TOKEN` set in CLI env | open shell, exec `echo ${DMUXAI_WEB_TOKEN:-unset}` | output `unset` | `tests/term_cli.rs` |
| TC-2690 | NFR-2690 | integration | isolated dirs | run `dmuxai status --json` | no term socket dir created, no host process | `tests/term_cli.rs` |
| TC-2691 | NFR-2691 | integration | warm host | 20 send+read round trips | p50 under the loose bound | `tests/term_cli.rs` |
| TC-2692 | NFR-2692 | integration | warm shell | exec `sleep 0.5` vs `bash -c 'sleep 0.5'` | overhead within ×3 bound | `tests/term_cli.rs` |
| TC-2693 | NFR-2693 | unit | buffer + wait | wait on idle session | wait uses a notify/condvar, not a sleep loop (completes promptly after a push) | `src/term/wait.rs` |
| TC-2695 | INT-2695 | integration | any command with `--json` | unknown session `show --json` | stdout parses as JSON with `error_kind: "not_found"`, exit 3 | `tests/term_cli.rs` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | New `terms/` tree beside `runs/` (manifest.json + io.jsonl, rotate at 8 MiB). No SQLite schema change. | Delete `terms/` to roll back; nothing else reads it. | TC-2620 |
| Provider protocol | Host protocol: one length-prefixed (u32 BE) JSON request per connection with `"v":1`; responses are JSON with `ok` or `error_kind`/`message`. | Unknown `v` is rejected with a clear error. | TC-2695 |
| Security / permissions | Socket 0600 in 0700 dir + peer uid check; child env scrubbed; journal masked, local only. The takeover lock rejects non-human input. The dangerous-command guard arrives in PR1b; until then a session is as powerful as the user's shell. | Must not weaken runner env scrubbing or journal masking. | TC-2601, TC-2609, TC-2620, TC-2632 |
| Platform | Linux (incl. WSL2) via `portable-pty`; the release musl static build must still link. | Windows is PR4; `term` on Windows exits 2 with "not supported yet". | gate + musl build |

## 7. Implementation plan

The detailed plan is kept as working notes outside the repo; PR1a order:

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Spec | `docs/specs/SPEC-terminal-sessions.md` | all | check-spec |
| 2 | Buffer, wait, marker, key table | `src/term/{mod,buffer,wait}.rs` | FR-2602–2605, NFR-2693 | TC-2602–2605, TC-2693 |
| 3 | PTY backend, host, protocol, client | `src/term/{backend,host,proto,client}.rs` | FR-2600, 2601, 2607–2609, 2632 | TC-2600, 2601, 2607–2609, 2632 |
| 4 | Journal | `src/term/journal.rs` | FR-2620, 2621 | TC-2620, 2621 |
| 5 | CLI + integration tests | `src/cli.rs`, `src/main.rs`, `tests/term_cli.rs` | FR-2600–2609, INT-2695, NFR-2690–2692 | `cargo test --test term_cli` |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] `scripts/gate.sh --browser` passes; `cargo test --test 'web_*' -- --ignored` passes or failures are known flakes.
- [ ] musl static release build still links with `portable-pty`.
- [ ] Perf numbers (NFR-2690–2692) recorded in the PR description.
