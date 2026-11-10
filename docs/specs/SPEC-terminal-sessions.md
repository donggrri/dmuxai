# SPEC-terminal-sessions: Persistent named terminal sessions (`dmuxai term`)

Status: in progress (PR1a on main 0bfc67b; PR1b merged as b2e8b6b; follow-up #315/#316/#317)
Owner: DmuxAI Team
Created: 2026-11-09
Baseline: main b2e8b6b (PR1b). IDs 2600–2699 are reserved for terminal sessions. PR1a shipped FR/TC-2600–2609, 2620, 2621, 2632, NFR/TC-2690–2693, INT/TC-2695. PR1b adds FR/TC-2610–2616, 2617, 2618, 2619, 2630. Follow-up after #307 adds TC-2622 and TC-2623 and tightens FR-2601, FR-2604, and FR-2606.
Related: #301 (overhead budget), #228 (invisible long-running command), #230 (tree kill), #194 (Display errors), #308 (usbipd auto-attach), #309 (term help), #310 (no-usbipd-attach with busid), #311 (term subcommand help), #312 (wait --idle), #313 (unitless --idle), #314 (close --all starts host), #315 (wait --match chunk/ANSI), #316 (list hides closed), #317 (host idle vs cwd filesystem), SPEC-run-journal, SPEC-process-supervisor, SPEC-agent-api

## 1. Decision summary

Agents and humans need long-lived terminals (a shell, an `ssh` session, later a serial console) that survive a single CLI call, so a test node can start a command, come back for its output, and be watched by a human. dmuxai adds named sessions owned by a per-workspace **session host** and driven by `dmuxai term …` subcommands with `--json`. Completion is detected by an output regex, an idle period, or (for POSIX shells) a nonce exit marker. Every input and output event is written, masked, to a local journal. PR1a shipped Linux PTY sessions. PR1b adds native serial consoles (including WSL usbipd attach), a dangerous-command block-and-approve guard with a per-workspace allowlist, a local bench script, and two leftover tightenings: `exec` returns only the command output, and `wait` without `--since` does not search old output.

## 2. Scope

### In scope

- PR1a: `dmuxai term open/send/read/wait/exec/list/show/resize/close/reopen`, session host, Linux PTY `shell`/`cmd`, journal, takeover lock.
- Native serial: `term open <name> --serial DEV --baud N` via `serial2` (no extra tool). `--kind cmd -- picocom …` remains supported as a PTY command.
- Serial errors with their own `error_kind` and no retry loop (missing device, permission, busy).
- When the device is missing on WSL2, dmuxai runs `usbipd.exe attach --wsl --busid <id>` itself only if an explicit busid is given (`--usbipd-busid` or trusted `[terminal.serial] usbipd_busid`). It never auto-selects a busid from `usbipd.exe list`. Without an explicit busid it does not invoke `usbipd.exe` and exits with a hint to run `usbipd list` and pass `--usbipd-busid`. `--no-usbipd-attach` (or trusted `[terminal.serial] usbipd_attach = false`) skips attach even when a busid is present. Failure prints a clear hint and does not loop.
- WSL1: `--serial COM3` is accepted and mapped to `/dev/ttyS3` (`COMn` → `/dev/ttySn`).
- Unplug (EIO/ENXIO) moves the session to `lost`; `term reopen <name>` retries the same settings.
- `term open … --wait-for REGEX --timeout DUR` opens and waits in one call.
- Dangerous-command guard on `send`/`exec` text: exit 7 with `approval_id`; `term approve <id>` (refused when `DMUXAI_RUN_ID` is set) lets that one input through. Per-workspace `[terminal.policy] allow` / extra `deny` apply only when the workspace is trusted.
- `scripts/bench-term.sh` (local, not CI) records release-build send/read and exec-vs-direct numbers.
- Leftovers: `exec` text has no echoed command, marker, or trailing prompt; `wait` without `--since` starts from the current seq.

### Out of scope / non-goals

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
| Kind | `shell` (interactive POSIX shell, exit markers allowed), `cmd` (any argv, no marker), or `serial` (native device via serial2). | Runner kinds. |
| Seq | A per-session monotonic counter; each output chunk covers a seq range so readers neither miss nor repeat data. | Journal line numbers. |
| Marker | A per-call nonce line `__DMUXAI_<nonce>_<exit>__` appended by `exec` to learn the exit code; it is removed from returned output. | Prompt regexes. |
| Takeover | A per-session flag meaning a human holds the session; non-human input is rejected while it is set. | Plan approval. |
| Approval | A one-shot id created when the dangerous-command guard blocks an input. `term approve <id>` releases that one input. | Graph plan approval. |

States: `starting → open → exited | closed | lost`; `open ↔ takeover` (flag, input lock only). `lost` is used when a serial device disappears (EIO/ENXIO). `exited` carries `exit_code` or `signal`. `exited` and `closed` are terminal. `reopen` on a `lost` (or still-specified) session starts a new backend with the same spec and name.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2600 | Must | When `term open <name> [--kind shell\|cmd] [--cwd DIR] [--env K=V]... [--cols N --rows N] [-- <argv>]` runs | A PTY child starts under the session host, `open` returns `{name, kind, state:"open", pid}`, and the session keeps running after the CLI exits. | A name not matching `[a-z0-9][a-z0-9-]{0,31}` exits 2; a name already open in the workspace exits 4; `--kind cmd` without argv exits 2. | TC-2600 |
| FR-2601 | Must | When a `term` subcommand that needs a session host runs and no host is listening | The host starts detached, listens on a per-workspace Unix socket in a 0700 directory with the socket at 0600, rejects peers with another uid, removes a stale socket, and exits after its last live (non-terminal) session ends plus an idle grace (default 600 s, `DMUXAI_TERM_IDLE_SECS` overrides). Idle expiry must not depend on the workspace or process-cwd filesystem (ext4, 9p, or a `/mnt/c` drvfs path). After binding the socket the host must not keep the workspace as its process cwd (chdir to the socket directory or another state path). The event loop must decide idle expiry without waiting on workspace directory I/O (journal flush is skipped once no live sessions remain, or is ordered after the idle check). `term list` and `term close` do not start a host (FR-2606, FR-2607). | If the socket path would exceed the platform limit, a short path under `$XDG_RUNTIME_DIR` or `/tmp/dmuxai-<uid>/` keyed by the repo key is used. A host that cannot start exits 6 with a Display message. | TC-2601, TC-2623 |
| FR-2602 | Must | When `term send <name> <text> [--no-newline]` or `term send <name> --keys K[,K...]` runs | The text plus `\n` (unless `--no-newline`) or the named keys (`C-a`..`C-z`, `Enter`, `Tab`, `Esc`, `Up`, `Down`, `Left`, `Right`, `Backspace`, `Space`, `F1`..`F12`) are written to the PTY, and the reply carries the seq at the time of the write. | More than 64 KiB per call exits 2; an unknown key name exits 2 naming it; a session that is not open exits 3 (unknown) or 4 (exited/closed). | TC-2602 |
| FR-2603 | Must | When `term read <name> [--since SEQ] [--tail N] [--raw]` runs | Output after `SEQ` (or the last N lines) is returned with `seq_from`, `seq_to`, and `dropped_bytes`; ANSI/control sequences are stripped unless `--raw`. | The buffer holds 1 MiB per session; data evicted before `SEQ` is reported in `dropped_bytes`, never silently skipped. | TC-2603 |
| FR-2604 | Must | When `term wait <name> (--match REGEX \| --idle DUR) --timeout DUR [--since SEQ]` runs | It returns as soon as stripped output after `SEQ` matches, or output has been quiet for `--idle`, with `reason: "match"` or `"idle"`, and `reason: "exited"` if the child exits first. `--match` searches the accumulated new text since `SEQ`, not each output chunk in isolation, so a prompt split across reads still matches. Matching uses the same control-sequence strip as `term read` without `--raw` (CSI including private-mode bytes such as `?`, charset escapes, and OSC); raw bytes stay available via `term read --raw`. When `--since` is omitted, the wait starts from the current seq (output already in the buffer is not searched for `--match`). `--idle` succeeds when no new output arrives for that duration; it does not require new bytes after the wait starts (the idle clock starts at wait start, or resets from the last output if more data arrives). `--since` applies only to `--match` search. A bare number for `--idle` is milliseconds (`--idle 500` is 500 ms); suffixes `ms`/`s`/`m`/`h` are accepted. A bare number for `--timeout` is seconds (`8` and `8s` are 8 seconds; `500ms` is 500 ms). | On timeout it exits 1 with `reason: "timeout"` and the output tail; an invalid regex or missing `--timeout` exits 2. | TC-2604, TC-2615, TC-2619, TC-2622 |
| FR-2605 | Must | When `term exec <name> <cmd> [--timeout DUR]` runs on a `shell` session | The command is sent followed by a nonce exit marker; the reply has the command's own output only: no echoed command line, no marker, and no trailing shell prompt, plus `exit_code`, `reason: "marker"`, and `elapsed_ms`. | On a `cmd` session it sends and waits for idle and returns `exit_code: null` with `reason`; on timeout it exits 1 with `reason: "timeout"`. | TC-2605, TC-2616 |
| FR-2606 | Must | When `term list` or `term show <name>` runs | Each session reports name, kind, state, pid, created_ms, last_output_ms, cols, rows, takeover, exit_code/signal, and seq. Closed and exited sessions are kept as history (`show` still works; closing an already-closed session stays a no-op). `term list` hides `closed` and `exited` sessions; `term list --all` includes them. | An unknown name in `show` exits 3. With no host running, `list` (with or without `--all`) returns `[]` without starting one. | TC-2606 |
| FR-2607 | Must | When `term close <name> [--force]` or `term close --all` runs, or the child exits on its own | Close sends SIGHUP, then after a 2 s grace (0 with `--force`) kills the whole process group tree; state becomes `closed`. A child that exits by itself moves the session to `exited` with its code or signal. | Closing an unknown name exits 3; closing an already closed session is a no-op success. With no host running, `close --all` (including `--force`) succeeds with zero sessions closed and does not start a host; `close NAME` exits 3 without starting a host. | TC-2607 |
| FR-2608 | Must | When `term resize <name> --cols N --rows N` runs | The PTY window size changes and the child sees it (`stty size`). | Values outside 1–1000 exit 2. | TC-2608 |
| FR-2609 | Must | While a session's takeover flag is set (host request `takeover`/`release`, dashboard in PR3) | Non-human `send`, `--keys`, and `exec` are rejected with exit 4, `error_kind: "conflict"`, and the message `session '<name>' is in human takeover; ask the user to release it`; `read`, `wait`, `show` keep working. | Release clears the flag; input works again immediately. | TC-2609 |
| FR-2620 | Must | For every input, key, resize, state change, and output chunk | A JSONL line `{seq, t_ms, dir, by, run_id, data, bytes}` is appended to `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/terms/<16hex>/io.jsonl` next to a `manifest.json` (schema_version 1); data is ANSI-stripped and passed through `mask_sensitive_tokens` before it is written; dirs 0700, files 0600, symlinks refused. | Output is coalesced into chunks of at most 50 ms / 8 KiB. A journal write error is reported in `show` as `journal_error` and does not kill the session. | TC-2620 |
| FR-2621 | Must | When `send --secret` is used, or the latest output matches `[Pp]assword:\|passphrase\|PIN:` | The journal records that input as `[REDACTED input]` and no reply echoes the typed text. | — | TC-2621 |
| FR-2632 | Must | When a session child starts | Its environment is scrubbed like runner children (`DMUXAI_WEB_TOKEN` and other dashboard secrets removed); `--env K=V` is added after scrubbing. | — | TC-2632 |
| FR-2610 | Must | When `term open <name> --serial DEV --baud N [--parity none\|even\|odd] [--data 8] [--stop 1] [--flow none\|rtscts]` runs | The host opens the device with serial2 (no external tool), the session kind is `serial`, and `send`/`read`/`wait` work as for a PTY session. `--kind cmd -- picocom -b N DEV` (or minicom) remains a supported alternative. | A missing `--baud` with `--serial` exits 2. `--serial` together with a shell argv exits 2. | TC-2610 |
| FR-2611 | Must | When opening a serial device fails | The CLI exits with a Display message and a distinct `error_kind`, and does not retry in a loop. Missing device: `serial device <path> not found. On WSL2 attach the USB device from Windows: usbipd list; usbipd attach --wsl --busid <id>. On WSL1, COMn is /dev/ttySn.` Permission denied: `permission denied on <path>. Add yourself to the dialout group: sudo usermod -aG dialout $USER, then log in again (WSL: wsl --shutdown).` Busy: `device is in use by pid N (<comm>)` when that process can be found. | dmuxai never suggests `chmod 666` or sudo to open the device. | TC-2611 |
| FR-2612 | Must | When `term open … --wait-for REGEX --timeout DUR` runs, or `term wait --match` is used on a serial session | Open succeeds and then waits for the regex on new output (boot banner, prompt, and similar strings) with the same timeout rules as `term wait`. | A missing `--timeout` with `--wait-for` exits 2. | TC-2612 |
| FR-2613 | Must | When a serial reader sees EIO or ENXIO (device unplugged) | The session state becomes `lost`. `term reopen <name>` retries with the same serial settings and, on success, returns state `open`. | `reopen` of an unknown name exits 3; `reopen` of a non-lost PTY session starts a new child with the same spec (already required in PR1a). | TC-2613 |
| FR-2614 | Must | When `--serial` points at a missing path and the environment looks like WSL2 (`/proc/version` contains `microsoft` or `WSL`) | If and only if an explicit busid is present (`--usbipd-busid` or trusted `[terminal.serial] usbipd_busid`) and attach is not disabled (`--no-usbipd-attach` or trusted `[terminal.serial] usbipd_attach = false`), dmuxai runs `usbipd.exe attach --wsl --busid <id>` once (Windows interop from WSL), waits a short time for the device node, then opens it. Without an explicit busid, dmuxai does not invoke `usbipd.exe` at all (no `list`, no `attach`) and exits immediately with `error_kind` `usbipd_busid_missing`, a hint to run `usbipd list` and pass `--usbipd-busid <id>`, and the FR-2611 manual hint. | dmuxai never auto-selects a busid from `usbipd.exe list`. If `usbipd.exe` is not found, the device is not bound (admin `usbipd bind` needed), attach fails, or the node is still missing, the command exits with `error_kind` naming the cause plus the FR-2611 manual hint. There is no retry loop. `--no-usbipd-attach` skips attach even when a busid is present and reports the missing-device error. | TC-2614 |
| FR-2615 | Must | When `term wait --match` is invoked without `--since` after earlier output that would match | The wait does not succeed against that earlier output; only data arriving after the wait starts can match. Callers that just sent input and want to match the output it causes should pass `--since` from that send's reply seq (otherwise a prompt that arrives before wait starts is ignored). | — | TC-2615 |
| FR-2616 | Must | When `term exec` runs a simple command such as `printf x` on a shell session | The returned `text` is exactly the command output (`x`), with no echoed command line, no nonce marker, and no trailing prompt. | — | TC-2616 |
| FR-2617 | Must | When `dmuxai term --help` or `dmuxai term <subcommand> --help` (or `-h`) runs | The CLI prints help for that topic: `term --help` lists term operations (open, send, read, wait, exec, approve, list, show, resize, close, reopen); `term open --help` documents `--serial`, `--baud`, `--usbipd-busid`, and `--no-usbipd-attach`; `term wait --help` documents `--match`, `--idle`, `--timeout`, and `--since`; every other term subcommand prints that subcommand's own help. | The printed text is not the root `dmuxai` usage, and a subcommand's help is not only the parent `term` list. `dmuxai --help` and `dmuxai run --help` are unchanged. | TC-2617 |
| FR-2630 | Must | When `term send` or `term exec` text from a CLI or agent caller matches a default deny pattern (or an extra trusted `[terminal.policy] deny` regex), and does not match a trusted `[terminal.policy] allow` regex | The write is not sent. The command exits 7 with `error_kind` `approval_required` and JSON `{approval_id, matched, session}`. `term approve <id>` (refused when `DMUXAI_RUN_ID` is set) then allows that one input through. Approvals expire after `[terminal.policy] approve_timeout` (default 10m). Default deny patterns include `rm -rf`/`rm -fr` on `/`, `~`, `*`, or `..`; `dd` with `of=`; `mkfs`/`wipefs`/`fdisk`/`parted`/`sgdisk`; writes to `/dev/sd*`, `nvme`, `mmcblk`, `mtd`; `flash_erase`/`flashcp`/`nandwrite`/`mtd erase`; U-Boot `nand erase`/`sf erase`/`mmc erase`/`erase `/`env default -a`/`saveenv`; `fastboot` flash/erase/format; `reboot`/`poweroff`/`shutdown`/`halt`/`init 0|6`/`systemctl reboot|poweroff`; `chmod -R 777 /`; and a fork-bomb pattern. Workspace `[terminal]` keys apply only when the workspace is trusted. | The guard is a seatbelt, not a sandbox: a script already running inside the session is not inspected. `--keys` cannot be used to send free text. | TC-2630 |
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
| TC-2604 | FR-2604 | unit + integration | open shell | wait `--match done` after sending a command that prints `done`; wait with a regex that never matches | reason `match`; never-matching exits 1 with `timeout` | `src/term/wait.rs`, `tests/term_cli.rs` |
| TC-2605 | FR-2605 | unit + integration | open shell | exec `true`, exec `false`, exec `printf x` | exit codes 0, 1, 0; output does not contain the marker | `src/term/wait.rs`, `tests/term_cli.rs` |
| TC-2606 | FR-2606 | integration | two sessions; separately after close | list, show one, show unknown, list with no host; after close, default list hides the closed session and `list --all` includes `state=closed` | fields present; unknown exit 3; empty list starts no host; default list after close does not include the closed/exited name | `tests/term_cli.rs` |
| TC-2607 | FR-2607 | integration | shell running `sleep 100 &` child; separately, no host running | close; separately `exit 3` inside a shell; `close --all` with no host | process group gone, state closed; self-exit gives state exited, exit_code 3; no-host `close --all` exits 0 with an empty closed list and starts no host | `tests/term_cli.rs` |
| TC-2608 | FR-2608 | integration | open shell | resize 100x30, exec `stty size` | output `30 100`; resize 0 exits 2 | `tests/term_cli.rs` |
| TC-2609 | FR-2609 | unit + integration | open shell with takeover set via host request | send, exec, read; release; send | send/exec exit 4 with the message; read ok; after release send ok | `src/term/host.rs`, `tests/term_cli.rs` |
| TC-2620 | FR-2620 | unit + integration | open shell | send `echo sk-test-AAAAAAAAAAAAAAAAAAAAAAAA` | io.jsonl has in/out lines, token masked, modes 0600/0700 | `src/term/journal.rs`, `tests/term_cli.rs` |
| TC-2621 | FR-2621 | unit | journal writer | `--secret` input; input after `Password:` output | journal line data is `[REDACTED input]` | `src/term/journal.rs` |
| TC-2632 | FR-2632 | integration | `DMUXAI_WEB_TOKEN` set in CLI env | open shell, exec `echo ${DMUXAI_WEB_TOKEN:-unset}` | output `unset` | `tests/term_cli.rs` |
| TC-2610 | FR-2610 | integration | in-process PTY pair: master runs a fake board, slave path is passed as `--serial` | `term open board --serial <slave> --baud 115200` then send/read | kind `serial`, output from the fake board is readable; no socat or extra tool | `tests/term_cli.rs`, `src/term/serial.rs` |
| TC-2611 | FR-2611 | integration | isolated dirs | open `--serial` a nonexistent path (not on WSL, or with usbipd disabled); open a 0000-mode PTY slave (skip if root) | missing device: exit not 0, `error_kind` set, hint text names the path; permission: hint mentions the dialout group | `tests/term_cli.rs` |
| TC-2612 | FR-2612 | integration | fake board prints a banner then a prompt | `term open … --serial <slave> --baud 115200 --wait-for` the banner `--timeout 5s` | open returns after the banner, `reason` is `match` | `tests/term_cli.rs` |
| TC-2613 | FR-2613 | integration | open a fake-board serial session | close the master (EIO); `show`; `reopen` after restoring a pair or against the same slave if still present | state becomes `lost`; reopen with a live slave returns `open` | `tests/term_cli.rs` |
| TC-2614 | FR-2614 | unit + integration | PATH has a fake `usbipd.exe` that records argv and a missing device path; `/proc/version` treated as WSL2 via a test hook or by running the helper directly | (1) open `--serial` missing path `--usbipd-busid 1-2`; (2) the same open without `--usbipd-busid`; (3) open a shell first (warm host), then open `--serial` missing path `--usbipd-busid 9-99 --no-usbipd-attach` | (1) the fake binary is invoked with `attach --wsl --busid 1-2` and is not used to pick an id via `list`; when the fake fails, the error names the cause and includes the manual hint; no busy loop. (2) the fake binary is not invoked at all (no `list`, no `attach`); `error_kind` is `usbipd_busid_missing`; the message mentions `usbipd list` and `--usbipd-busid`. (3) the fake binary is not invoked; `error_kind` is the missing-device kind (`serial_not_found`), not `usbipd_attach_failed` | `src/term/serial.rs`, `tests/term_cli.rs` |
| TC-2615 | FR-2615, FR-2604 | integration | open shell, send `echo already_there` and wait until it appears | `term wait --match already_there --timeout 1s` without `--since` | exit 1, `reason: timeout` (old output is not a match) | `tests/term_cli.rs` |
| TC-2616 | FR-2616, FR-2605 | integration | open shell | `term exec 'printf x' --timeout 5s` | `text` equals `x` (no echo, no marker, no prompt) | `tests/term_cli.rs` |
| TC-2617 | FR-2617 | integration | isolated dirs | `term --help`, `term open --help`, and `--help`/`-h` for every other term subcommand (send, read, wait, exec, approve, list, show, resize, close, reopen) | exit 0; term help names open/send/approve; open help names `--serial` and `--usbipd-busid`; wait help names `--match` and `--idle`; each subcommand help is not the root usage and (except `term --help` itself) is not only the parent term list | `tests/term_cli.rs`, `src/main.rs` |
| TC-2618 | FR-2610 | integration (ignored) | `socat` on PATH | optional PTY pair via socat, then `--kind cmd -- picocom` if present | send/read works; skipped when tools are missing (`#[ignore]`, not required in the gate) | `tests/term_cli.rs` |
| TC-2619 | FR-2604 | unit + integration | open shell, send a command, `wait --match` succeeds | `term wait NAME --idle 1s --timeout 8s --json` without `--since`; also `--idle 500` (unitless) `--timeout 8s` | exit 0, `reason: idle`, finishes well before the 8s timeout; does not require new output after the wait starts; unitless `--idle 500` is 500 ms (not 500 s) | `src/term/wait.rs`, `src/cli.rs`, `tests/term_cli.rs` |
| TC-2622 | FR-2604 | unit + integration | ring buffer and/or a PTY/serial fake | wait `--match '[#\\$] '` while a prompt arrives split across two pushes and wrapped as `#` + CSI reset (including a private-mode sequence such as `\\x1b[?2004h`) + space, and with CR | reason `match`; raw `read --raw` still contains the ESC bytes; FR-2615 still holds (the same pattern already in the buffer before wait starts does not match without `--since`) | `src/term/wait.rs`, `src/term/buffer.rs`, `tests/term_cli.rs` |
| TC-2623 | FR-2601 | integration | temp workspace, short `DMUXAI_TERM_IDLE_SECS` | open a session, close it, then wait for the host to exit; inspect the live host's cwd | the host process cwd is not the workspace path; after close the host exits within the idle grace plus a small slack, without needing workspace filesystem events | `src/term/host.rs`, `tests/term_cli.rs` |
| TC-2630 | FR-2630 | unit + integration | open shell | `term send`/`exec` a default-denied command such as `reboot`; `term approve` the id; send again; `term approve` with `DMUXAI_RUN_ID` set; trusted workspace `allow = ["^reboot$"]` | first call exits 7 with `approval_id` and does not run; after approve the same text is sent; approve inside a run exits 5 or 7 as forbidden; allowlisted text is sent without approve | `src/term/policy.rs`, `tests/term_cli.rs` |
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
| Security / permissions | Socket 0600 in 0700 dir + peer uid check; child env scrubbed; journal masked, local only. The takeover lock rejects non-human input. The dangerous-command guard (FR-2630) blocks matching `send`/`exec` text until `term approve`. It is a seatbelt, not a sandbox. | Must not weaken runner env scrubbing or journal masking. | TC-2601, TC-2609, TC-2620, TC-2630, TC-2632 |
| Platform | Linux (incl. WSL2) via `portable-pty` and `serial2`. WSL2 may invoke `usbipd.exe` once on a missing serial device. | Windows native serial is PR4. `term` on Windows still exits 2. | TC-2610, TC-2611, TC-2614 |

## 7. Implementation plan

PR1a is on main (`0bfc67b`). PR1b order:

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Spec | `docs/specs/SPEC-terminal-sessions.md` | new rows | check-spec |
| 2 | Serial backend + usbipd helper + virtual board tests | `src/term/serial.rs`, `src/term/{mod,backend,host,client}.rs`, Cargo.toml (`serial2`) | FR-2610–2614 | TC-2610–2614 |
| 3 | Policy + `term approve` | `src/term/policy.rs`, settings `[terminal]`, `src/cli.rs` | FR-2630 | TC-2630 |
| 4 | Leftover wait/exec tightening | `src/term/wait.rs`, exec path | FR-2615, FR-2616 | TC-2615, TC-2616 |
| 5 | CLI flags, tests, bench script, lab checklist | `src/cli.rs`, `tests/term_cli.rs`, `scripts/bench-term.sh`, `docs/term-serial-lab-checklist.md` | NFR-2691, NFR-2692 | `cargo test --test term_cli`; script is local only |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] `scripts/gate.sh --browser` passes; `cargo test --test 'web_*' -- --ignored` passes or failures are known flakes.
- [ ] musl static release build still links with `portable-pty`.
- [ ] Perf numbers (NFR-2690–2692) recorded in the PR description.
