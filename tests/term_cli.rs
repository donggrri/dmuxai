//! Integration tests for persistent named terminal sessions
//! (SPEC-terminal-sessions, section 5).
//!
//! Each test spawns the compiled `dmuxai` binary with isolated `HOME`/`XDG_*`
//! dirs, `DMUXAI_TERM_IDLE_SECS=5`, and `/bin/sh` as the shell. Sessions are
//! closed at the end of every test and no external tools beyond `/bin/sh` are
//! required.
//!
//! Requirement coverage:
//! - TC-2600 (FR-2600): open/duplicate/bad name, session survives the CLI
//! - TC-2601 (FR-2601): socket 0600 in 0700 dir, host alive (unit covers the
//!   long-path fallback in `src/term/client.rs`)
//! - TC-2602 (FR-2602): send text, keys, oversize, unknown key
//! - TC-2604 (FR-2604): wait match, wait timeout (unit covers never-match regex
//!   semantics in `src/term/wait.rs`)
//! - TC-2605 (FR-2605): exec true/false/printf, marker removed
//! - TC-2606 (FR-2606): list/show/unknown/empty list without host; closed and
//!   exited sessions are history hidden from the default list, shown by
//!   `list --all` and still reported by `show`; both list forms return []
//!   without a host and never start one
//! - TC-2607 (FR-2607): close kills the process-group child, self-exit state;
//!   close --all / close NAME without a listening host spawn no host
//! - TC-2608 (FR-2608): resize propagated to the PTY, invalid dims exit 2
//! - TC-2609 (FR-2609): takeover lock rejects send/exec, read/wait ok, release
//!   (unit covers the lock helper in `src/term/host.rs`)
//! - TC-2610 (FR-2610): native serial open/send/read/wait over an in-process
//!   PTY pair (no socat); TC-2611 (FR-2611): serial error kinds survive the
//!   host, and a 0000 slave (root skip) reports the dialout hint, not chmod 666
//! - TC-2612 (FR-2612): `open --wait-for` returns on the fake board banner
//! - TC-2613 (FR-2613): closing the PTY master marks the session lost; reopen
//!   against a restored slave returns open
//! - TC-2614 (FR-2614): WSL2 usbipd attach runs once with the explicit busid;
//!   without a busid usbipd.exe is never invoked and the error is
//!   `usbipd_busid_missing`; `--no-usbipd-attach` skips attach even with a
//!   busid
//! - TC-2615 (FR-2615): wait without --since does not match older buffer text
//! - TC-2622 (FR-2604): `wait --match` matches a prompt emitted in several
//!   writes and wrapped in ANSI escapes/CR; `read --raw` still shows ESC
//! - TC-2616 (FR-2616): exec reply text is exactly the command output
//! - TC-2617 (FR-2617): `term --help`, `term open --help` and `--help`/`-h`
//!   for every other term subcommand exit 0 with subcommand-local help, not
//!   the root usage
//! - TC-2619 (FR-2604): `wait --idle` without `--since` succeeds on a quiet
//!   session well before the timeout; a bare `--idle 500` is 500 ms
//! - TC-2618 (FR-2610): optional socat PTY pair (`#[ignore]`, skips if absent)
//!   plus a picocom `--kind cmd` reader on the same pair when picocom exists
//! - TC-2630 (FR-2630): default-deny send and exec each need one-shot
//!   approval; trusted allow bypasses it; approve is refused while
//!   DMUXAI_RUN_ID is set
//! - TC-2620 (FR-2620): masked io.jsonl with modes 0600/0700 (unit covers the
//!   writer in `src/term/journal.rs`)
//! - TC-2632 (FR-2632): child env scrubbed, `--env` still delivered
//! - TC-2623 (#317): the host process cwd is not the workspace and the host
//!   exits within the idle grace plus slack after the last session closes
//! - TC-2690 (NFR-2690): non-term commands never start a host
//! - TC-2691 (NFR-2691): warm-host send+read p50 under the loose bound
//! - TC-2692 (NFR-2692): exec overhead within the loose x3 bound
//! - TC-2693 (NFR-2693): a read from a second client is served while the host
//!   is still blocked in another client's wait
//! - TC-2695 (INT-2695): `--json` errors are exactly one JSON object

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use dmuxai::term::proto::{Request, RequestOp, Response, read_response, write_request};

const IDLE_SECS: &str = "5";

struct Sandbox {
    _dir: tempfile::TempDir,
    workspace: PathBuf,
    data_home: PathBuf,
    state_home: PathBuf,
    runtime_dir: PathBuf,
    home: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let workspace = root.join("ws");
        let data_home = root.join("data");
        let state_home = root.join("state");
        let runtime_dir = root.join("runtime");
        let home = root.join("home");
        for dir in [&workspace, &data_home, &state_home, &runtime_dir, &home] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Self {
            _dir: dir,
            workspace,
            data_home,
            state_home,
            runtime_dir,
            home,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
        cmd.arg("-C")
            .arg(&self.workspace)
            .args(args)
            .current_dir(&self.workspace)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_STATE_HOME", &self.state_home)
            .env("XDG_RUNTIME_DIR", &self.runtime_dir)
            .env("HOME", &self.home)
            .env("DMUXAI_TRUST_WORKSPACE_CONFIG", "1")
            .env("DMUXAI_TERM_IDLE_SECS", IDLE_SECS)
            .env("SHELL", "/bin/sh")
            .env("DMUXAI_WORKSPACE", &self.workspace)
            .env_remove("DMUXAI_STATE_DIR")
            .env_remove("DMUXAI_WEB_TOKEN");
        cmd
    }

    fn run(&self, args: &[&str]) -> Out {
        let output = self
            .command(args)
            .stdin(Stdio::null())
            .output()
            .expect("execute dmuxai");
        Out::new(output)
    }

    /// Runs a term subcommand and returns the captured output with its JSON.
    fn term_json(&self, args: &[&str]) -> (Out, Value) {
        let out = self.run(args);
        let json = out.json("stdout");
        (out, json)
    }

    /// Opens a shell session named `name` using the deterministic shell.
    fn open_shell(&self, name: &str) -> Value {
        let (out, json) = self.term_json(&["term", "open", name, "--json"]);
        assert_eq!(out.code, 0, "term open failed: {out:?}");
        json
    }

    fn repo_key(&self) -> String {
        dmuxai::state_path::repo_key(&self.workspace).expect("repo_key")
    }

    /// Per-workspace socket directory: `$XDG_STATE_HOME/dmuxai/repos/<key>/term`.
    fn socket_dir(&self) -> PathBuf {
        self.state_home
            .join("dmuxai")
            .join("repos")
            .join(self.repo_key())
            .join("term")
    }

    fn socket_file(&self) -> PathBuf {
        self.socket_dir().join("host.sock")
    }

    /// Session journal roots: `$XDG_DATA_HOME/dmuxai/repos/<key>/terms`.
    fn terms_dir(&self) -> PathBuf {
        self.data_home
            .join("dmuxai")
            .join("repos")
            .join(self.repo_key())
            .join("terms")
    }

    /// Pids of running `dmuxai … -C <workspace> … term __host` processes.
    fn host_pids(&self) -> Vec<u32> {
        procs_with_args(&[
            "-C",
            self.workspace.to_str().expect("utf8 workspace"),
            "term",
            "__host",
        ])
    }

    /// Closes every live session in the workspace; a no-op when no host runs.
    fn close_all(&self) {
        if self.host_pids().is_empty() {
            return;
        }
        let out = self.run(&["term", "close", "--all", "--json"]);
        if out.code != 0 {
            eprintln!("close_all failed for {}: {out:?}", self.workspace.display());
        }
    }

    fn link_serial(&self, name: &str, board: &PtyBoard) -> PathBuf {
        let link = self.workspace.join(name);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(board.path(), &link).expect("link serial board");
        link
    }

    fn close_guard(&self) -> CloseGuard<'_> {
        CloseGuard { sandbox: self }
    }

    fn host_request_on(&self, mut stream: UnixStream, op: RequestOp) -> Response {
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        write_request(&mut stream, &Request::new(op)).expect("write request");
        read_response(&mut stream).expect("read response")
    }
}

struct CloseGuard<'a> {
    sandbox: &'a Sandbox,
}

impl Drop for CloseGuard<'_> {
    fn drop(&mut self) {
        self.sandbox.close_all();
    }
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn new(output: Output) -> Self {
        Self {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Parses `stdout` as exactly one JSON value (INT-2695 contract).
    fn json(&self, what: &str) -> Value {
        serde_json::from_str(self.stdout.trim()).unwrap_or_else(|error| {
            panic!("{what} is not one JSON object: {error}\n{}", self.stdout)
        })
    }
}

impl std::fmt::Debug for Out {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Out")
            .field("code", &self.code)
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .finish()
    }
}

/// True while `/proc/<pid>` exists.
fn pid_exists(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Polls `check` until it returns true, up to `limit`.
fn poll_until(limit: Duration, mut check: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    loop {
        if check() {
            return true;
        }
        if started.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Pids of processes whose exact argument list contains every needle.
fn procs_with_args(needles: &[&str]) -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return pids;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let args: Vec<String> = raw
            .split(|byte| *byte == 0)
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect();
        if needles
            .iter()
            .all(|needle| args.iter().any(|arg| arg == needle))
        {
            pids.push(pid);
        }
    }
    pids
}

/// Extracts a captured pid from an exec reply such as `CHILD_12345`.
fn child_pid_from(text: &str, prefix: &str) -> u32 {
    let pattern = format!("{prefix}(\\d+)");
    let re = regex::Regex::new(&pattern).unwrap();
    let caps = re
        .captures(text)
        .unwrap_or_else(|| panic!("no {prefix}<pid> in exec output: {text:?}"));
    caps[1].parse().unwrap()
}

/// An in-process PTY pair used as a virtual serial board. The slave path is
/// passed to `--serial`; the master side prints a banner and answers PING with
/// PONG. No socat or external serial tool is involved.
struct PtyBoard {
    path: PathBuf,
    master: i32,
    slave: i32,
    stop: Arc<AtomicBool>,
    emit_tx: std::sync::mpsc::Sender<Vec<u8>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn drain_emits(rx: &std::sync::mpsc::Receiver<Vec<u8>>, master: i32) {
    while let Ok(data) = rx.try_recv() {
        board_write(master, &data);
    }
}

impl PtyBoard {
    fn start() -> Self {
        let mut master = -1;
        let mut slave = -1;
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(
            result,
            0,
            "openpty failed: {}",
            std::io::Error::last_os_error()
        );
        unsafe {
            libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let path = std::fs::read_link(format!("/proc/self/fd/{slave}"))
            .expect("resolve the PTY slave path");
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(slave, &mut termios) == 0 {
                libc::cfmakeraw(&mut termios);
                let _ = libc::tcsetattr(slave, libc::TCSANOW, &termios);
            }
        }

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let (emit_tx, emit_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let thread = std::thread::spawn(move || {
            board_write(master, b"BOOT READY\r\n");
            let mut buffer = [0u8; 1024];
            while !thread_stop.load(Ordering::Relaxed) {
                drain_emits(&emit_rx, master);
                let mut pollfd = libc::pollfd {
                    fd: master,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut pollfd, 1, 50) };
                if ready <= 0 {
                    continue;
                }
                let read = unsafe {
                    libc::read(
                        master,
                        buffer.as_mut_ptr().cast::<libc::c_void>(),
                        buffer.len(),
                    )
                };
                if read <= 0 {
                    break;
                }
                let data = &buffer[..read as usize];
                if data.windows(4).any(|window| window == b"PING") {
                    board_write(master, b"PONG\r\n");
                }
            }
        });

        Self {
            path,
            master,
            slave,
            stop,
            emit_tx,
            thread: Some(thread),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Sends raw bytes to the fake board from the test side; the board thread
    /// writes them onto the PTY master within roughly one poll tick.
    fn emit(&self, data: &[u8]) {
        self.emit_tx
            .send(data.to_vec())
            .expect("board thread alive");
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if self.master >= 0 {
            unsafe {
                libc::close(self.master);
            }
            self.master = -1;
        }
        if self.slave >= 0 {
            unsafe {
                libc::close(self.slave);
            }
            self.slave = -1;
        }
    }
}

impl Drop for PtyBoard {
    fn drop(&mut self) {
        self.close();
    }
}

fn fake_usbipd(dir: &Path, script: &str) -> PathBuf {
    let path = dir.join("usbipd.exe");
    std::fs::write(&path, script).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn approval_id_from(json: &Value) -> String {
    assert_eq!(json["ok"].as_bool(), Some(false), "blocked send: {json:?}");
    assert_eq!(
        json["exit_code"].as_i64(),
        Some(7),
        "blocked send: {json:?}"
    );
    assert_eq!(
        json["error_kind"], "approval_required",
        "blocked send: {json:?}"
    );
    assert_eq!(json["session"], "a", "blocked send: {json:?}");
    let matched = json["matched"].as_str().unwrap_or_default();
    assert!(!matched.is_empty(), "blocked send has matched: {json:?}");
    let id = json["approval_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no approval_id: {json:?}"))
        .to_owned();
    assert_eq!(id.len(), 16, "approval id: {id}");
    assert!(
        id.chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
        "approval id must be lowercase hex: {id}"
    );
    id
}

fn board_write(fd: i32, data: &[u8]) {
    let mut offset = 0;
    while offset < data.len() {
        let written = unsafe {
            libc::write(
                fd,
                data[offset..].as_ptr().cast::<libc::c_void>(),
                data.len() - offset,
            )
        };
        if written <= 0 {
            break;
        }
        offset += written as usize;
    }
}

/// Kills and reaps a spawned CLI child on drop so a failing assert never
/// leaves the process behind.
struct ReapOnDrop(Option<Child>);

impl Drop for ReapOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// TC-2600 / FR-2600: open ok with pid, duplicate exit 4, bad name exit 2, and
/// the session keeps running after the CLI exits.
#[test]
fn tc_2600_open_duplicate_bad_name_and_session_survives_cli_exit() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    let (out, opened) = sandbox.term_json(&["term", "open", "a", "--json"]);
    assert_eq!(out.code, 0, "open failed: {out:?}");
    assert_eq!(opened["name"], "a");
    assert_eq!(opened["kind"], "shell");
    assert_eq!(opened["state"], "open");
    let pid = opened["pid"].as_u64().expect("pid present") as u32;
    assert!(pid_exists(pid), "session pid {pid} alive after open");

    // Duplicate name in the workspace: exit 4 (conflict).
    let (dup, json) = sandbox.term_json(&["term", "open", "a", "--json"]);
    assert_eq!(dup.code, 4, "duplicate open must exit 4: {dup:?}");
    assert_eq!(json["ok"], false, "duplicate reply: {json:?}");
    assert_eq!(json["error_kind"], "conflict");

    // Invalid name: exit 2 (usage).
    let (bad, json) = sandbox.term_json(&["term", "open", "Bad_Name", "--json"]);
    assert_eq!(bad.code, 2, "bad name must exit 2: {bad:?}");
    assert_eq!(json["error_kind"], "usage");

    // The session still runs in a host process after the CLI exits.
    std::thread::sleep(Duration::from_millis(300));
    assert!(pid_exists(pid), "session pid {pid} alive after CLI exited");
    let (out, shown) = sandbox.term_json(&["term", "show", "a", "--json"]);
    assert_eq!(out.code, 0, "show after CLI exit failed: {out:?}");
    assert_eq!(shown["state"], "open");
    assert_eq!(shown["pid"], json!(pid));
}

/// TC-2601 / FR-2601: first `term open` starts a detached host listening on a
/// per-workspace unix socket with mode 0600 in a 0700 directory; the host
/// stays alive past the CLI that started it. The long-path fallback itself is
/// unit-tested in `src/term/client.rs`.
#[test]
fn tc_2601_socket_modes_and_host_alive() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    assert!(
        sandbox.host_pids().is_empty(),
        "no host before the first term command"
    );

    let (out, opened) = sandbox.term_json(&["term", "open", "a", "--json"]);
    assert_eq!(out.code, 0, "open failed: {out:?}");
    assert_eq!(opened["state"], "open", "open: {opened:?}");

    let socket = sandbox.socket_file();
    assert!(socket.exists(), "socket at {}", socket.display());
    let dir_mode = std::fs::metadata(sandbox.socket_dir())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700, "socket dir mode 0700");
    let socket_mode = std::fs::metadata(&socket).unwrap().permissions().mode();
    assert_eq!(socket_mode & 0o777, 0o600, "socket file mode 0600");

    // A follow-up term command in a different process talks to the same host.
    let (out, shown) = sandbox.term_json(&["term", "show", "a", "--json"]);
    assert_eq!(out.code, 0, "show failed: {out:?}");
    assert_eq!(shown["state"], "open", "showed: {shown:?}");

    let pids = sandbox.host_pids();
    assert_eq!(pids.len(), 1, "one detached host process: {pids:?}");
    assert!(pid_exists(pids[0]), "host pid {} alive", pids[0]);
}

/// TC-2602 / FR-2602: send text reaches the shell, send --keys works, oversize
/// input exits 2, and an unknown key name exits 2 naming the key.
#[test]
fn tc_2602_send_echo_keys_and_limits() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    // Text path: `echo hi` shows up in stripped output.
    let (out, read) = sandbox.term_json(&["term", "read", "a", "--json"]);
    assert_eq!(out.code, 0, "read failed: {out:?}");
    let since = read["seq_to"].as_u64().expect("seq_to");
    let (out, sent) = sandbox.term_json(&["term", "send", "a", "echo hi", "--json"]);
    assert_eq!(out.code, 0, "send failed: {out:?}");
    assert!(sent["seq"].is_u64(), "send reply carries seq: {sent:?}");
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        r"\bhi\b",
        "--since",
        &since.to_string(),
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "wait for output failed: {out:?}");
    assert_eq!(waited["reason"], "match", "wait: {waited:?}");
    assert!(waited["text"].as_str().unwrap().contains("hi"));

    // Named keys: C-c maps into the control byte table, unknown name exits 2.
    let (out, _) = sandbox.term_json(&["term", "send", "a", "--keys", "C-c", "--json"]);
    assert_eq!(out.code, 0, "send --keys C-c failed: {out:?}");
    let (out, json) = sandbox.term_json(&["term", "send", "a", "--keys", "Nope", "--json"]);
    assert_eq!(out.code, 2, "unknown key must exit 2: {out:?}");
    assert!(
        json["error"].as_str().unwrap_or_default().contains("Nope"),
        "unknown key names the key: {json:?}"
    );

    // More than 64 KiB per call exits 2 (65537 bytes with --no-newline).
    let oversize = "x".repeat(65537);
    let (out, json) =
        sandbox.term_json(&["term", "send", "a", "--no-newline", &oversize, "--json"]);
    assert_eq!(out.code, 2, "oversize send must exit 2: {out:?}");
    assert_eq!(json["error_kind"], "usage", "oversize: {json:?}");
}

/// TC-2604 / FR-2604: wait returns `reason: "match"` when the regex appears in
/// output after SEQ, and exits 1 with `reason: "timeout"` when it never does.
#[test]
fn tc_2604_wait_match_and_timeout() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    let (_, read) = sandbox.term_json(&["term", "read", "a", "--json"]);
    let since = read["seq_to"].as_u64().expect("seq_to");
    let (out, _) = sandbox.term_json(&[
        "term",
        "send",
        "a",
        "--no-newline",
        "sleep 0.2; echo done_$((1+1))\n",
        "--json",
    ]);
    assert_eq!(out.code, 0, "send failed: {out:?}");
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        r"\bdone_2\b",
        "--since",
        &since.to_string(),
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "wait match failed: {out:?}");
    assert_eq!(waited["reason"], "match", "wait: {waited:?}");
    assert!(
        waited["text"]
            .as_str()
            .unwrap_or_default()
            .contains("done_2"),
        "wait text: {waited:?}"
    );

    // A regex that never matches exits 1 with reason timeout and a tail.
    let (_, read) = sandbox.term_json(&["term", "read", "a", "--json"]);
    let since = read["seq_to"].as_u64().expect("seq_to");
    let (out, timed) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        "zz-never-match-zz",
        "--since",
        &since.to_string(),
        "--timeout",
        "2s",
        "--json",
    ]);
    assert_eq!(out.code, 1, "never-matching wait must exit 1: {out:?}");
    assert_eq!(timed["reason"], "timeout", "wait: {timed:?}");
    assert!(
        timed["text"].is_string(),
        "timeout reply carries output tail"
    );

    // Invalid regex exits 2 on the client before reaching the host.
    let out = sandbox.run(&["term", "wait", "a", "--match", "([", "--timeout", "1s"]);
    assert_eq!(out.code, 2, "invalid regex must exit 2: {out:?}");
}

/// TC-2605 / FR-2605: exec returns the child's exit code via the nonce marker
/// and the reply never contains the marker text.
#[test]
fn tc_2605_exec_exit_codes_and_marker_stripped() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    let (out, result) = sandbox.term_json(&["term", "exec", "a", "true", "--json"]);
    assert_eq!(out.code, 0, "exec true failed: {out:?}");
    assert_eq!(result["exit_code"], 0, "exec true: {result:?}");
    assert_eq!(result["reason"], "marker", "exec true: {result:?}");
    assert!(result["elapsed_ms"].is_u64(), "exec true: {result:?}");

    let (out, result) = sandbox.term_json(&["term", "exec", "a", "false", "--json"]);
    assert_eq!(out.code, 0, "exec false failed: {out:?}");
    assert_eq!(result["exit_code"], 1, "exec false: {result:?}");

    let (out, result) = sandbox.term_json(&["term", "exec", "a", "printf x", "--json"]);
    assert_eq!(out.code, 0, "exec printf failed: {out:?}");
    assert_eq!(result["exit_code"], 0, "exec printf: {result:?}");
    let text = result["text"].as_str().unwrap_or_default();
    assert!(text.contains('x'), "exec output: {result:?}");
    assert!(!text.contains("__DMUXAI_"), "marker leaked: {result:?}");
}

/// TC-2615 / FR-2615: a `term wait --match` without `--since` starts at the
/// current sequence, so output already in the buffer does not satisfy it; an
/// explicit `--since 0` still matches older output.
#[test]
fn tc_2615_wait_without_since_ignores_older_output() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    let (out, _) = sandbox.term_json(&["term", "send", "a", "echo already_there", "--json"]);
    assert_eq!(out.code, 0, "send failed: {out:?}");
    let appeared = poll_until(Duration::from_secs(5), || {
        let (out, read) = sandbox.term_json(&["term", "read", "a", "--json"]);
        out.code == 0
            && read["text"]
                .as_str()
                .unwrap_or_default()
                .contains("already_there")
    });
    assert!(appeared, "`echo already_there` never reached the buffer");

    let (out, timed) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        "already_there",
        "--timeout",
        "1s",
        "--json",
    ]);
    assert_eq!(out.code, 1, "old output must not match: {out:?}");
    assert_eq!(timed["reason"], "timeout", "wait: {timed:?}");

    let (out, matched) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        "already_there",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "explicit --since 0 must still match: {out:?}");
    assert_eq!(matched["reason"], "match", "wait: {matched:?}");
}

/// TC-2622 / FR-2604: `wait --match '[#$] '` succeeds when the PTY fake board
/// emits its prompt split across separate writes and wrapped in ANSI escapes
/// and CR — the matcher strips the wrapping. `read --raw` still shows the
/// raw ESC bytes.
#[test]
fn tc_2622_wait_match_prompt_across_writes_with_ansi_and_cr() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let board = PtyBoard::start();
    let serial = board.path().to_str().expect("utf8 slave path").to_owned();

    let (out, opened) = sandbox.term_json(&[
        "term", "open", "board", "--serial", &serial, "--baud", "115200", "--json",
    ]);
    assert_eq!(out.code, 0, "serial open failed: {out:?}");
    assert_eq!(opened["state"], "open", "open: {opened:?}");

    // Consume the startup banner first so the later wait starts fresh.
    let (out, _) = sandbox.term_json(&[
        "term",
        "wait",
        "board",
        "--match",
        "BOOT READY",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "banner wait failed: {out:?}");

    // The board prints its prompt in several writes, wrapped in a title OSC,
    // CSI color sequences and CRs, so no single write contains "[#$] ".
    board.emit(b"\x1b]0;board\x07");
    board.emit(b"\r");
    board.emit(b"\x1b[1;32m");
    board.emit(b"boar");
    board.emit(b"d0\x1b[0m");
    board.emit(b"\r");
    board.emit(b"boar");
    board.emit(b"d0# ");

    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "board",
        "--match",
        r"[#$] ",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "wait for wrapped prompt failed: {out:?}");
    assert_eq!(waited["reason"], "match", "wait: {waited:?}");
    let text = waited["text"].as_str().unwrap_or_default().to_owned();
    assert!(text.contains("d0# "), "match text: {text:?}");
    assert!(!text.contains("\x1b"), "match text is stripped: {text:?}");

    // TC-2622: `read --raw` keeps the escape bytes.
    let (out, read) = sandbox.term_json(&["term", "read", "board", "--raw", "--json"]);
    assert_eq!(out.code, 0, "raw read failed: {out:?}");
    let raw = read["text"].as_str().unwrap_or_default().to_owned();
    assert!(raw.contains("\x1b"), "raw read keeps ESC: {raw:?}");
    assert!(raw.contains("d0"), "raw read has the prompt: {raw:?}");
}

/// TC-2616 / FR-2616: the exec reply `text` is exactly the command output —
/// no echoed command line, no nonce marker and no trailing prompt.
#[test]
fn tc_2616_exec_text_is_exactly_command_output() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    let (out, result) = sandbox.term_json(&["term", "exec", "a", "printf x", "--json"]);
    assert_eq!(out.code, 0, "exec printf failed: {out:?}");
    assert_eq!(result["reason"], "marker", "exec printf: {result:?}");
    assert_eq!(result["text"], json!("x"), "exec printf: {result:?}");

    let (out, result) = sandbox.term_json(&["term", "exec", "a", "echo hi", "--json"]);
    assert_eq!(out.code, 0, "exec echo failed: {out:?}");
    assert_eq!(result["text"], json!("hi"), "exec echo: {result:?}");

    let (out, result) = sandbox.term_json(&["term", "exec", "a", "true", "--json"]);
    assert_eq!(out.code, 0, "exec true failed: {out:?}");
    assert_eq!(result["text"], json!(""), "exec true: {result:?}");
    assert!(
        !result["text"].as_str().unwrap().contains("__DMUXAI_"),
        "marker leaked: {result:?}"
    );
}

/// TC-2630 / FR-2630: a default-denied send and a default-denied exec are not
/// written/executed, each exits 7 with a structured approval id (approval_id,
/// matched, session), and `term approve` releases exactly one identical input.
/// `term approve` is refused while a run id is present.
#[test]
fn tc_2630_dangerous_send_needs_one_shot_approval() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    let (out, blocked) = sandbox.term_json(&["term", "send", "a", "echo shutdown", "--json"]);
    assert_eq!(out.code, 7, "blocked send: {out:?}");
    let id = approval_id_from(&blocked);

    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        "shutdown",
        "--since",
        "0",
        "--timeout",
        "1s",
        "--json",
    ]);
    assert_eq!(out.code, 1, "blocked text must not run: {out:?}");
    assert_eq!(waited["reason"], "timeout", "blocked wait: {waited:?}");

    let (out, approved) = sandbox.term_json(&["term", "approve", &id, "--json"]);
    assert_eq!(out.code, 0, "approve failed: {out:?}");
    assert_eq!(approved["approved"], id.as_str(), "approve: {approved:?}");

    let (out, _) = sandbox.term_json(&["term", "send", "a", "echo shutdown", "--json"]);
    assert_eq!(out.code, 0, "approved send failed: {out:?}");
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        "shutdown",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "approved text must run: {out:?}");
    assert_eq!(waited["reason"], "match", "approved wait: {waited:?}");

    let (out, blocked_again) = sandbox.term_json(&["term", "send", "a", "echo shutdown", "--json"]);
    assert_eq!(out.code, 7, "approval is one-shot: {out:?}");
    let id_again = approval_id_from(&blocked_again);
    assert_ne!(id, id_again, "a fresh approval id is issued");

    // TC-2630 covers `exec` the same way: a denied command exits 7 with the
    // structured approval JSON, the text never reaches the shell, and the
    // approval releases exactly one identical exec.
    let (out, blocked_exec) = sandbox.term_json(&["term", "exec", "a", "echo halt", "--json"]);
    assert_eq!(out.code, 7, "blocked exec: {out:?}");
    let id_exec = approval_id_from(&blocked_exec);
    let matched = blocked_exec["matched"].as_str().unwrap_or_default();
    assert!(
        matched.contains("halt"),
        "exec refusal names the matched pattern: {blocked_exec:?}"
    );

    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        r"\bhalt\b",
        "--since",
        "0",
        "--timeout",
        "1s",
        "--json",
    ]);
    assert_eq!(out.code, 1, "blocked exec text must not run: {out:?}");
    assert_eq!(waited["reason"], "timeout", "blocked exec wait: {waited:?}");

    let (out, approved) = sandbox.term_json(&["term", "approve", &id_exec, "--json"]);
    assert_eq!(out.code, 0, "approve exec failed: {out:?}");
    assert_eq!(
        approved["approved"],
        id_exec.as_str(),
        "approve: {approved:?}"
    );

    let (out, result) = sandbox.term_json(&["term", "exec", "a", "echo halt", "--json"]);
    assert_eq!(out.code, 0, "approved exec failed: {out:?}");
    assert_eq!(result["exit_code"], 0, "allowed exec: {result:?}");
    assert_eq!(result["reason"], "marker", "allowed exec: {result:?}");
    assert_eq!(result["text"], json!("halt"), "allowed exec: {result:?}");

    let (out, blocked_exec_again) =
        sandbox.term_json(&["term", "exec", "a", "echo halt", "--json"]);
    assert_eq!(out.code, 7, "exec approval is one-shot: {out:?}");
    let id_exec_again = approval_id_from(&blocked_exec_again);
    assert_ne!(id_exec, id_exec_again, "a fresh approval id is issued");

    let output = sandbox
        .command(&["term", "approve", &id_again, "--json"])
        .env("DMUXAI_RUN_ID", "run-1")
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");
    let out = Out::new(output);
    let json = out.json("stdout");
    assert_eq!(
        out.code, 5,
        "approve inside a run must be forbidden: {out:?}"
    );
    assert_eq!(json["error_kind"], "forbidden", "run guard: {json:?}");
}

/// TC-2630 / FR-2630: a trusted workspace `[terminal.policy] allow` regex
/// bypasses the default deny list, so the same text is written directly.
#[test]
fn tc_2630_trusted_allow_bypasses_default_deny() {
    let sandbox = Sandbox::new();
    std::fs::write(
        sandbox.workspace.join("dmuxai.toml"),
        "[terminal.policy]\nallow = [\"^echo shutdown$\"]\n",
    )
    .unwrap();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    let (out, sent) = sandbox.term_json(&["term", "send", "a", "echo shutdown", "--json"]);
    assert_eq!(out.code, 0, "allowlisted send failed: {out:?}");
    assert!(sent["seq"].is_u64(), "send reply: {sent:?}");
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        "shutdown",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "allowlisted text must run: {out:?}");
    assert_eq!(waited["reason"], "match", "allowlisted wait: {waited:?}");
}

/// TC-2618 / FR-2610 (ignored): an external socat PTY pair is optional and is
/// skipped cleanly when socat is not installed.
#[test]
#[ignore = "requires socat; skipped when the tool is missing"]
fn tc_2618_socat_serial_pair_optional() {
    if Command::new("socat")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("skipping TC-2618: socat is not installed");
        return;
    }

    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let link = sandbox.workspace.join("socat-serial");
    let board = sandbox.workspace.join("socat-board.sh");
    std::fs::write(
        &board,
        "#!/bin/sh\nprintf 'BOOT READY\\r\\n'\nwhile IFS= read -r line; do\n  case \"$line\" in\n    *PING*) printf 'PONG\\r\\n' ;;\n  esac\ndone\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&board).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&board, permissions).unwrap();

    let socat = Command::new("socat")
        .args([
            "-d",
            "-d",
            &format!("pty,raw,echo=0,link={}", link.display()),
            &format!("exec:{}", board.display()),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn socat");
    let _reap = ReapOnDrop(Some(socat));
    if !poll_until(Duration::from_secs(5), || link.exists()) {
        eprintln!("skipping TC-2618: socat did not create the PTY link");
        return;
    }

    let (out, opened) = sandbox.term_json(&[
        "term",
        "open",
        "board",
        "--serial",
        link.to_str().expect("utf8 link"),
        "--baud",
        "115200",
        "--wait-for",
        "BOOT READY",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "socat serial open failed: {out:?}");
    assert_eq!(opened["reason"], "match", "socat open wait: {opened:?}");
    let (out, _) = sandbox.term_json(&["term", "send", "board", "PING", "--json"]);
    assert_eq!(out.code, 0, "socat serial send failed: {out:?}");
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "board",
        "--match",
        "PONG",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "socat serial reply failed: {out:?}");
    assert_eq!(waited["reason"], "match", "socat serial reply: {waited:?}");

    // TC-2618 extra piece: with picocom installed, a `--kind cmd` session
    // drives the same socat PTY pair. If picocom is missing, only this extra
    // part is skipped; the socat serial path above still ran.
    let picocom = Command::new("picocom")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok();
    if !picocom {
        eprintln!("skipping TC-2618 picocom part: picocom is not installed");
        return;
    }

    let (out, opened) = sandbox.term_json(&[
        "term",
        "open",
        "picocom-board",
        "--kind",
        "cmd",
        "--json",
        "--",
        "picocom",
        "--noinit",
        "--nolock",
        "-b",
        "115200",
        link.to_str().expect("utf8 link"),
    ]);
    assert_eq!(out.code, 0, "picocom open failed: {out:?}");
    assert_eq!(opened["kind"], "cmd", "open: {opened:?}");
    assert_eq!(opened["state"], "open", "open: {opened:?}");
    assert!(opened["pid"].is_u64(), "cmd session has a pid: {opened:?}");

    let (out, sent) = sandbox.term_json(&["term", "send", "picocom-board", "PING", "--json"]);
    assert_eq!(out.code, 0, "picocom send failed: {out:?}");
    assert!(sent["seq"].is_u64(), "send reply: {sent:?}");
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "picocom-board",
        "--match",
        "PONG",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "picocom reply read failed: {out:?}");
    assert_eq!(waited["reason"], "match", "picocom reply: {waited:?}");
    assert!(
        waited["text"].as_str().unwrap_or_default().contains("PONG"),
        "reply text: {waited:?}"
    );
}

/// TC-2606 / FR-2606: list with no host returns `[]` without starting one;
/// show reports the session fields; unknown names exit 3.
#[test]
fn tc_2606_list_show_and_unknown() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    // No host running: `list --json` and `list --all --json` return [] and
    // neither starts a host.
    for args in [
        vec!["term", "list", "--json"],
        vec!["term", "list", "--all", "--json"],
    ] {
        let (out, listed) = sandbox.term_json(&args);
        assert_eq!(out.code, 0, "list without host failed: {out:?}");
        assert_eq!(listed, json!([]), "empty list: {listed:?}");
        assert!(sandbox.host_pids().is_empty(), "list must not start a host");
        assert!(!sandbox.socket_file().exists(), "no socket for list alone");
    }

    sandbox.open_shell("b");
    sandbox.open_shell("c");

    let (out, listed) = sandbox.term_json(&["term", "list", "--json"]);
    assert_eq!(out.code, 0, "list failed: {out:?}");
    let sessions = listed.as_array().expect("list array");
    assert_eq!(sessions.len(), 2, "list: {sessions:?}");
    for entry in sessions {
        for field in [
            "name",
            "kind",
            "state",
            "pid",
            "created_ms",
            "last_output_ms",
            "cols",
            "rows",
            "takeover",
            "seq",
        ] {
            assert!(
                entry.get(field).is_some(),
                "list entry missing {field}: {entry:?}"
            );
        }
    }

    let (out, shown) = sandbox.term_json(&["term", "show", "b", "--json"]);
    assert_eq!(out.code, 0, "show failed: {out:?}");
    for field in [
        "name",
        "kind",
        "state",
        "pid",
        "created_ms",
        "last_output_ms",
        "cols",
        "rows",
        "takeover",
        "exit_code",
        "signal",
        "seq",
    ] {
        // exit_code and signal are legitimately null while the session is open.
        assert!(shown.get(field).is_some(), "show field {field}: {shown:?}");
    }
    assert_eq!(shown["name"], "b");
    assert_eq!(shown["state"], "open", "showed: {shown:?}");

    // TC-2606 / FR-2606: a closed session stays in the host's list as
    // intended history. The default list hides it, `list --all` includes it
    // with state=closed, and `show` still reports closed.
    let (out, _) = sandbox.term_json(&["term", "close", "c", "--json"]);
    assert_eq!(out.code, 0, "close c failed: {out:?}");

    let (out, listed) = sandbox.term_json(&["term", "list", "--json"]);
    assert_eq!(out.code, 0, "list after close failed: {out:?}");
    let names: Vec<&str> = listed
        .as_array()
        .expect("list array")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(
        names.contains(&"b") && !names.contains(&"c"),
        "closed session must be hidden from the default list: {names:?}"
    );

    let (out, listed_all) = sandbox.term_json(&["term", "list", "--all", "--json"]);
    assert_eq!(out.code, 0, "list --all after close failed: {out:?}");
    let entries = listed_all.as_array().expect("list --all array");
    assert_eq!(entries.len(), 2, "list --all includes closed: {entries:?}");
    let closed = entries
        .iter()
        .find(|entry| entry["name"].as_str() == Some("c"))
        .expect("closed entry in --all");
    assert_eq!(closed["state"], "closed", "closed entry: {closed:?}");

    let (out, shown) = sandbox.term_json(&["term", "show", "c", "--json"]);
    assert_eq!(out.code, 0, "show closed failed: {out:?}");
    assert_eq!(
        shown["state"], "closed",
        "show still reports closed: {shown:?}"
    );

    let (out, json) = sandbox.term_json(&["term", "show", "ghost", "--json"]);
    assert_eq!(out.code, 3, "unknown show must exit 3: {out:?}");
    assert_eq!(json["error_kind"], "not_found", "unknown: {json:?}");
}

/// TC-2607 / FR-2607: close kills the whole process group tree and the state
/// becomes `closed`; a child that exits by itself moves the session to
/// `exited` with its code.
#[test]
fn tc_2607_close_process_group_and_self_exit() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    // A background `sleep` inside the shell is part of the process group.
    sandbox.open_shell("a");
    let (out, exec) =
        sandbox.term_json(&["term", "exec", "a", "sleep 100 & echo CHILD_$!", "--json"]);
    assert_eq!(out.code, 0, "exec failed: {out:?}");
    let sleep_pid = child_pid_from(exec["text"].as_str().unwrap_or_default(), "CHILD_");
    assert!(pid_exists(sleep_pid), "sleep pid {sleep_pid} started");

    let (out, _) = sandbox.term_json(&["term", "close", "a", "--json"]);
    assert_eq!(out.code, 0, "close failed: {out:?}");
    let gone = poll_until(Duration::from_secs(3), || !pid_exists(sleep_pid));
    assert!(gone, "close killed the process-group child {sleep_pid}");
    let (_, shown) = sandbox.term_json(&["term", "show", "a", "--json"]);
    assert_eq!(shown["state"], "closed", "after close: {shown:?}");

    // Closing an unknown name exits 3; closing again is a no-op success.
    let (out, _) = sandbox.term_json(&["term", "close", "a", "--json"]);
    assert_eq!(out.code, 0, "double close must succeed: {out:?}");
    let (out, _) = sandbox.term_json(&["term", "close", "ghost", "--json"]);
    assert_eq!(out.code, 3, "unknown close must exit 3: {out:?}");

    // A child exiting by itself gives state `exited` with its exit code.
    sandbox.open_shell("b");
    let (out, _) = sandbox.term_json(&["term", "send", "b", "exit 3", "--json"]);
    assert_eq!(out.code, 0, "send exit 3 failed: {out:?}");
    let reached = poll_until(Duration::from_secs(5), || {
        let (out, shown) = sandbox.term_json(&["term", "show", "b", "--json"]);
        out.code == 0
            && shown["state"] == "exited"
            && shown.get("exit_code").and_then(Value::as_i64) == Some(3)
    });
    assert!(reached, "self-exit did not record state exited/exit_code 3");
}

/// TC-2607 / FR-2607: with no host listening, `term close --all --json` (with
/// or without `--force`) exits 0 with an empty `closed` array and never starts
/// a host or creates the per-workspace socket.
#[test]
fn tc_2607_close_all_without_host_spawns_nothing() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    assert!(
        sandbox.host_pids().is_empty(),
        "no host before the first term command"
    );
    for args in [
        vec!["term", "close", "--all", "--json"],
        vec!["term", "close", "--all", "--force", "--json"],
    ] {
        let (out, closed) = sandbox.term_json(&args);
        assert_eq!(out.code, 0, "close --all without a host failed: {out:?}");
        assert_eq!(closed["closed"], json!([]), "closed: {closed:?}");
        assert!(
            sandbox.host_pids().is_empty(),
            "close --all must not start a host: {args:?}"
        );
        assert!(
            !sandbox.socket_file().exists(),
            "close --all must not create a socket: {args:?}"
        );
    }
}

/// TC-2607 / FR-2607: with no host listening, `term close ghost --json` exits 3
/// with `not_found` and, like `close --all`, never spawns a host or socket.
#[test]
fn tc_2607_close_named_without_host_reports_not_found() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    let (out, json) = sandbox.term_json(&["term", "close", "ghost", "--json"]);
    assert_eq!(out.code, 3, "close without a host must exit 3: {out:?}");
    assert_eq!(json["error_kind"], "not_found", "close ghost: {json:?}");
    assert!(
        sandbox.host_pids().is_empty(),
        "close NAME must not start a host"
    );
    assert!(
        !sandbox.socket_file().exists(),
        "close NAME must not create a socket"
    );
}

/// TC-2623 (#317): the host process is not pinned to the workspace and does
/// not outlive its sessions by much: its cwd symlink under procfs is not the
/// sandbox workspace (when procfs exposes it), and after the last session is
/// closed the host process exits within DMUXAI_TERM_IDLE_SECS (5) plus a
/// small slack (about 3s).
#[test]
fn tc_2623_host_cwd_not_workspace_and_exits_after_close() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    let pids = sandbox.host_pids();
    assert_eq!(pids.len(), 1, "one host process: {pids:?}");
    let host_pid = pids[0];

    // Skipped when the procfs symlink cannot be read (anything but "the
    // workspace" is fine; the host keeps its journal cwd instead).
    if let Ok(cwd) = std::fs::read_link(format!("/proc/{host_pid}/cwd")) {
        assert_ne!(
            cwd, sandbox.workspace,
            "host cwd must not be the sandbox workspace: {cwd:?}"
        );
    }

    let (out, _) = sandbox.term_json(&["term", "close", "a", "--json"]);
    assert_eq!(out.code, 0, "close failed: {out:?}");

    // IDLE_SECS is 5 in every test run; allow ~3s of slack.
    let exited = poll_until(
        Duration::from_secs(IDLE_SECS.parse::<u64>().expect("idle secs") + 3),
        || !pid_exists(host_pid),
    );
    assert!(
        exited,
        "host process {host_pid} must exit within idle grace + slack"
    );
}

/// TC-2608 / FR-2608: resize changes the PTY window the child sees
/// (`stty size` prints "<rows> <cols>"); values outside 1-1000 exit 2.
#[test]
fn tc_2608_resize_visible_to_child() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    let (out, _) = sandbox.term_json(&[
        "term", "resize", "a", "--cols", "100", "--rows", "30", "--json",
    ]);
    assert_eq!(out.code, 0, "resize failed: {out:?}");
    let (out, shown) = sandbox.term_json(&["term", "show", "a", "--json"]);
    assert_eq!(out.code, 0, "show failed: {out:?}");
    assert_eq!(shown["cols"], 100, "showed: {shown:?}");
    assert_eq!(shown["rows"], 30, "showed: {shown:?}");

    let (_, exec) = sandbox.term_json(&["term", "exec", "a", "stty size", "--json"]);
    let text = exec["text"].as_str().unwrap_or_default();
    assert!(
        text.lines().any(|line| line.trim() == "30 100"),
        "stty size must report rows 30 cols 100: {exec:?}"
    );

    let out = sandbox.run(&["term", "resize", "a", "--cols", "0", "--rows", "30"]);
    assert_eq!(out.code, 2, "resize 0 must exit 2: {out:?}");
    let out = sandbox.run(&["term", "resize", "a", "--cols", "100", "--rows", "1001"]);
    assert_eq!(out.code, 2, "resize 1001 rows must exit 2: {out:?}");
}

/// TC-2609 / FR-2609: with the takeover flag set through a host request,
/// send/keys/exec are rejected with exit 4 and the takeover message; read and
/// show keep working; release clears the lock immediately.
#[test]
fn tc_2609_takeover_lock_blocks_input_until_release() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    let error_response = sandbox.host_request_on(
        UnixStream::connect(sandbox.socket_file()).expect("connect to term host"),
        RequestOp::Takeover {
            name: "a".to_owned(),
        },
    );
    assert!(error_response.ok, "takeover request: {error_response:?}");

    let takeover_message = "session 'a' is in human takeover; ask the user to release it";

    let (out, json) = sandbox.term_json(&["term", "send", "a", "echo hi", "--json"]);
    assert_eq!(out.code, 4, "send under takeover must exit 4: {out:?}");
    assert_eq!(json["error_kind"], "conflict", "send: {json:?}");
    assert_eq!(json["error"], takeover_message, "send: {json:?}");

    let (out, json) = sandbox.term_json(&["term", "exec", "a", "true", "--json"]);
    assert_eq!(out.code, 4, "exec under takeover must exit 4: {out:?}");
    assert_eq!(json["error_kind"], "conflict", "exec: {json:?}");
    assert_eq!(json["error"], takeover_message, "exec: {json:?}");

    // Reads keep working under the lock.
    let (out, read) = sandbox.term_json(&["term", "read", "a", "--json"]);
    assert_eq!(out.code, 0, "read under takeover failed: {out:?}");
    assert!(read["text"].is_string(), "read under takeover: {read:?}");
    let (out, shown) = sandbox.term_json(&["term", "show", "a", "--json"]);
    assert_eq!(out.code, 0, "show under takeover failed: {out:?}");
    assert_eq!(shown["takeover"], true, "show: {shown:?}");

    let response = sandbox.host_request_on(
        UnixStream::connect(sandbox.socket_file()).expect("connect to term host"),
        RequestOp::Release {
            name: "a".to_owned(),
        },
    );
    assert!(response.ok, "release request: {response:?}");

    // Input works again immediately after release.
    let (out, sent) = sandbox.term_json(&["term", "send", "a", "--no-newline", "x", "--json"]);
    assert_eq!(out.code, 0, "send after release failed: {out:?}");
    assert!(sent["seq"].is_u64(), "send after release: {sent:?}");
    let (_, shown) = sandbox.term_json(&["term", "show", "a", "--json"]);
    assert_eq!(shown["takeover"], false, "show after release: {shown:?}");
}

/// TC-2610 / FR-2610: a native serial session opens the PTY-pair slave path
/// with serial2 (no external tool), reports kind `serial`, and send/read/wait
/// work against the fake board on the master side.
#[test]
fn tc_2610_serial_open_send_read_with_pty_board() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let board = PtyBoard::start();
    let serial = board.path().to_str().expect("utf8 slave path").to_owned();

    let (out, opened) = sandbox.term_json(&[
        "term", "open", "board", "--serial", &serial, "--baud", "115200", "--json",
    ]);
    assert_eq!(out.code, 0, "serial open failed: {out:?}");
    assert_eq!(opened["name"], "board");
    assert_eq!(opened["kind"], "serial", "open: {opened:?}");
    assert_eq!(opened["state"], "open", "open: {opened:?}");
    assert!(opened["pid"].is_null(), "serial sessions have no child pid");

    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "board",
        "--match",
        "BOOT READY",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "wait for banner failed: {out:?}");
    assert_eq!(waited["reason"], "match", "wait: {waited:?}");
    assert!(
        waited["text"]
            .as_str()
            .unwrap_or_default()
            .contains("BOOT READY"),
        "banner text: {waited:?}"
    );

    let (out, _) = sandbox.term_json(&["term", "send", "board", "PING", "--json"]);
    assert_eq!(out.code, 0, "serial send failed: {out:?}");
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "board",
        "--match",
        "PONG",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "wait for reply failed: {out:?}");
    assert_eq!(waited["reason"], "match", "reply: {waited:?}");
}

/// TC-2611 / FR-2611: serial open failures keep the host's distinct
/// `error_kind` through the client and include the FR-2611 hint.
#[test]
fn tc_2611_serial_missing_device_keeps_error_kind() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let missing = sandbox.workspace.join("missing-serial");
    let output = sandbox
        .command(&[
            "term",
            "open",
            "board",
            "--serial",
            missing.to_str().expect("utf8 path"),
            "--baud",
            "115200",
            "--json",
        ])
        .env("DMUXAI_SERIAL_WSL2", "0")
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");
    let out = Out::new(output);
    let json = out.json("stdout");
    assert_eq!(out.code, 3, "missing serial must exit 3: {out:?}");
    assert_eq!(json["error_kind"], "serial_not_found", "error: {json:?}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains(missing.to_str().unwrap()),
        "hint names the path: {json:?}"
    );
}

/// TC-2611 / FR-2611: a slave whose mode denies the open reports
/// `serial_permission` with the dialout-group manual hint and never suggests
/// `chmod 666`. Skipped as root, who bypasses permission checks, and tested
/// against a real PTY slave so the device path exists.
#[test]
fn tc_2611_serial_permission_denied_names_dialout() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping TC-2611 0000-slave case: running as root");
        return;
    }
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let board = PtyBoard::start();
    let serial = board.path().to_str().expect("utf8 slave path").to_owned();
    std::fs::set_permissions(board.path(), std::fs::Permissions::from_mode(0o000))
        .expect("chmod 0000 the PTY slave");

    let output = sandbox
        .command(&[
            "term", "open", "board", "--serial", &serial, "--baud", "115200", "--json",
        ])
        .env("DMUXAI_SERIAL_WSL2", "0")
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");
    let out = Out::new(output);
    let json = out.json("stdout");
    assert_eq!(out.code, 5, "denied slave open must exit 5: {out:?}");
    assert_eq!(json["error_kind"], "serial_permission", "error: {json:?}");
    let message = json["error"].as_str().unwrap_or_default().to_owned();
    assert!(message.contains("dialout"), "dialout hint: {json:?}");
    assert!(
        message.contains(&serial),
        "hint names the path: {message:?}"
    );
    assert!(
        !message.contains("chmod 666"),
        "must never suggest chmod 666: {message:?}"
    );
}

/// TC-2612 / FR-2612: `open --wait-for REGEX --timeout DUR` returns after the
/// fake board banner and reports `reason: match`; missing --timeout exits 2.
#[test]
fn tc_2612_open_wait_for_banner() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let board = PtyBoard::start();
    let serial = board.path().to_str().expect("utf8 slave path").to_owned();

    let (out, opened) = sandbox.term_json(&[
        "term",
        "open",
        "board",
        "--serial",
        &serial,
        "--baud",
        "115200",
        "--wait-for",
        "BOOT READY",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "open --wait-for failed: {out:?}");
    assert_eq!(opened["reason"], "match", "open wait: {opened:?}");
    assert_eq!(opened["kind"], "serial", "open wait: {opened:?}");
    assert!(
        opened["text"]
            .as_str()
            .unwrap_or_default()
            .contains("BOOT READY"),
        "open wait text: {opened:?}"
    );

    let out = sandbox.run(&[
        "term",
        "open",
        "board2",
        "--serial",
        &serial,
        "--baud",
        "115200",
        "--wait-for",
        "BOOT READY",
    ]);
    assert_eq!(out.code, 2, "--wait-for without --timeout must exit 2");
}

/// TC-2613 / FR-2613: closing the master side makes the serial session `lost`;
/// `reopen` with a live slave path for the same spec returns state `open`.
#[test]
fn tc_2613_serial_lost_then_reopen() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let mut board = PtyBoard::start();
    let link = sandbox.link_serial("board", &board);
    let serial = link.to_str().expect("utf8 link path").to_owned();

    let (out, opened) = sandbox.term_json(&[
        "term", "open", "board", "--serial", &serial, "--baud", "115200", "--json",
    ]);
    assert_eq!(out.code, 0, "serial open failed: {out:?}");
    assert_eq!(opened["state"], "open", "open: {opened:?}");
    let (out, _) = sandbox.term_json(&[
        "term",
        "wait",
        "board",
        "--match",
        "BOOT READY",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "banner wait failed: {out:?}");

    board.close();
    let lost = poll_until(Duration::from_secs(5), || {
        let (out, shown) = sandbox.term_json(&["term", "show", "board", "--json"]);
        out.code == 0 && shown["state"] == "lost"
    });
    assert!(lost, "closing the master must move the session to lost");

    let new_board = PtyBoard::start();
    sandbox.link_serial("board", &new_board);
    let (out, reopened) = sandbox.term_json(&["term", "reopen", "board", "--json"]);
    assert_eq!(out.code, 0, "reopen failed: {out:?}");
    assert_eq!(reopened["state"], "open", "reopen: {reopened:?}");
    assert_eq!(reopened["kind"], "serial", "reopen: {reopened:?}");
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "board",
        "--match",
        "BOOT READY",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "wait after reopen failed: {out:?}");
    assert_eq!(waited["reason"], "match", "wait after reopen: {waited:?}");
}

/// TC-2614 / FR-2614: a WSL2-looking environment runs the fake usbipd.exe
/// attach exactly once with the explicit busid, then reports the attach failure
/// with the cause and the FR-2611 manual hint.
#[test]
fn tc_2614_wsl2_usbipd_attach_once_with_explicit_busid() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let log = sandbox.workspace.join("usbipd.log");
    let fake = fake_usbipd(
        &sandbox.workspace,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\necho 'device is not bound' >&2\nexit 1\n",
            log.display()
        ),
    );
    let missing = sandbox.workspace.join("missing-serial");
    let output = sandbox
        .command(&[
            "term",
            "open",
            "board",
            "--serial",
            missing.to_str().expect("utf8 path"),
            "--baud",
            "115200",
            "--usbipd-busid",
            "1-2",
            "--json",
        ])
        .env("DMUXAI_SERIAL_WSL2", "1")
        .env("DMUXAI_USBIPD_BIN", &fake)
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");
    let out = Out::new(output);
    let json = out.json("stdout");
    assert_eq!(out.code, 6, "attach failure must exit 6: {out:?}");
    assert_eq!(
        json["error_kind"], "usbipd_attach_failed",
        "error: {json:?}"
    );
    let message = json["error"].as_str().unwrap_or_default();
    assert!(message.contains("device is not bound"), "cause: {json:?}");
    assert!(
        message.contains("usbipd attach --wsl --busid"),
        "manual hint: {json:?}"
    );

    let calls = std::fs::read_to_string(&log).expect("usbipd log");
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("attach"))
            .count(),
        1,
        "attach must run once: {calls}"
    );
    assert!(
        calls.lines().any(|line| line == "attach --wsl --busid 1-2"),
        "attach argv: {calls}"
    );
    assert!(
        !calls.lines().any(|line| line == "list"),
        "an explicit busid must not list: {calls}"
    );
}

/// TC-2614 / FR-2614: on fake WSL2 with a missing `--serial` path and no
/// explicit busid, dmuxai never invokes usbipd.exe (no `list`, no `attach`)
/// and exits with `error_kind` `usbipd_busid_missing` plus a message that names
/// `usbipd list` and `--usbipd-busid`.
#[test]
fn tc_2614_without_busid_usbipd_is_never_called() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let log = sandbox.workspace.join("usbipd.log");
    let fake = fake_usbipd(
        &sandbox.workspace,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\necho 'device is not bound' >&2\nexit 1\n",
            log.display()
        ),
    );
    let missing = sandbox.workspace.join("missing-serial");
    let output = sandbox
        .command(&[
            "term",
            "open",
            "board",
            "--serial",
            missing.to_str().expect("utf8 path"),
            "--baud",
            "115200",
            "--json",
        ])
        .env("DMUXAI_SERIAL_WSL2", "1")
        .env("DMUXAI_USBIPD_BIN", &fake)
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");
    let out = Out::new(output);
    let json = out.json("stdout");
    assert_eq!(
        out.code, 6,
        "a missing busid must exit 6 (internal): {out:?}"
    );
    assert_eq!(
        json["error_kind"], "usbipd_busid_missing",
        "error: {json:?}"
    );
    let message = json["error"].as_str().unwrap_or_default();
    assert!(message.contains("usbipd list"), "hint: {json:?}");
    assert!(message.contains("--usbipd-busid"), "hint: {json:?}");
    assert!(
        !log.exists(),
        "usbipd.exe must not be invoked without an explicit busid"
    );
}

/// TC-2614 / FR-2614: `--no-usbipd-attach` skips the usbipd.exe attach even
/// when an explicit busid is set and reports the normal missing-device error.
/// The host is already warm (a shell session was opened first), so the flag
/// must travel inside the open request, not through inherited env vars.
#[test]
fn tc_2614_no_attach_flag_skips_usbipd_with_busid() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let log = sandbox.workspace.join("usbipd.log");
    let fake = fake_usbipd(
        &sandbox.workspace,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\necho 'device is not bound' >&2\nexit 1\n",
            log.display()
        ),
    );
    // Warm host: the serial open below is served by this running host, so the
    // CLI's own environment is not enough; the request must carry the flag.
    sandbox.open_shell("warm");
    let missing = sandbox.workspace.join("missing-serial");
    let output = sandbox
        .command(&[
            "term",
            "open",
            "board",
            "--serial",
            missing.to_str().expect("utf8 path"),
            "--baud",
            "115200",
            "--usbipd-busid",
            "9-99",
            "--no-usbipd-attach",
            "--json",
        ])
        .env("DMUXAI_SERIAL_WSL2", "1")
        .env("DMUXAI_USBIPD_BIN", &fake)
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");
    let out = Out::new(output);
    let json = out.json("stdout");
    assert_eq!(
        out.code, 3,
        "a disabled attach must report the missing device: {out:?}"
    );
    assert_eq!(
        json["error_kind"], "serial_not_found",
        "the missing-device path, not usbipd_attach_failed: {json:?}"
    );
    assert!(
        !json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("device is not bound"),
        "the fake usbipd must not have run: {json:?}"
    );
    assert!(
        !log.exists(),
        "the fake usbipd.exe must not be invoked (no list, no attach) when attach is disabled"
    );
}

/// TC-2617 / FR-2617: `term --help`, `term open --help` and every other term
/// subcommand's `--help` / `-h` exit 0 with subcommand-local help; none prints
/// the root usage, and a subcommand's help is not only the parent `term` list.
/// The root `dmuxai --help` is unchanged.
#[test]
fn tc_2617_term_help_and_open_help_are_subcommand_local() {
    let sandbox = Sandbox::new();

    for args in [vec!["term", "--help"], vec!["term", "-h"]] {
        let out = sandbox.run(&args);
        assert_eq!(out.code, 0, "term help must exit 0: {out:?}");
        let text = out.stdout.as_str();
        assert!(
            text.contains("Usage: dmuxai term"),
            "term help usage line: {out:?}"
        );
        assert!(
            text.contains("  open ") && text.contains("  send ") && text.contains("  approve "),
            "term help names open, send and approve: {out:?}"
        );
        assert!(
            !text.contains("dmuxai [--help|--version]") && !text.contains("One terminal"),
            "term help must not print the root usage: {out:?}"
        );
    }

    for args in [vec!["term", "open", "--help"], vec!["term", "open", "-h"]] {
        let out = sandbox.run(&args);
        assert_eq!(out.code, 0, "term open help must exit 0: {out:?}");
        let text = out.stdout.as_str();
        assert!(
            text.contains("Usage: dmuxai term open"),
            "term open help usage line: {out:?}"
        );
        for option in ["--serial", "--baud", "--usbipd-busid", "--no-usbipd-attach"] {
            assert!(
                text.contains(option),
                "term open help documents {option}: {out:?}"
            );
        }
        assert!(
            !text.contains("dmuxai [--help|--version]") && !text.contains("One terminal"),
            "term open help must not print the root usage: {out:?}"
        );
    }

    // Every other term subcommand prints its own help topic, not the root
    // usage and not only the parent term list.
    let topics: [(&str, &[&str]); 9] = [
        ("send", &["--keys", "--no-newline", "--secret"]),
        ("read", &["--since", "--tail", "--raw"]),
        ("wait", &["--match", "--idle", "--timeout", "--since"]),
        ("exec", &["--timeout"]),
        ("approve", &["16 lowercase hex"]),
        ("list", &["--all", "never starts one"]),
        ("resize", &["--cols", "--rows"]),
        ("close", &["--all", "--force"]),
        ("reopen", &["same spec"]),
    ];
    for (subcommand, options) in topics {
        for flag in ["--help", "-h"] {
            let out = sandbox.run(&["term", subcommand, flag]);
            assert_eq!(out.code, 0, "term {subcommand} {flag} must exit 0: {out:?}");
            let text = out.stdout.as_str();
            assert!(
                text.contains(&format!("Usage: dmuxai term {subcommand}")),
                "term {subcommand} {flag} usage line: {out:?}"
            );
            for option in options {
                assert!(
                    text.contains(option),
                    "term {subcommand} help documents {option}: {out:?}"
                );
            }
            assert!(
                !text.contains("dmuxai [--help|--version]") && !text.contains("One terminal"),
                "term {subcommand} help must not print the root usage: {out:?}"
            );
            assert!(
                !text.contains("Subcommands:"),
                "term {subcommand} help must not be only the parent term list: {out:?}"
            );
        }
    }

    // The root usage is unchanged.
    let out = sandbox.run(&["--help"]);
    assert_eq!(out.code, 0, "root help must exit 0: {out:?}");
    assert!(
        out.stdout.contains("dmuxai [--help|--version]"),
        "root help: {out:?}"
    );
    assert!(
        !out.stdout.contains("--no-usbipd-attach"),
        "root help must not document term-open serial flags: {out:?}"
    );
    // `dmuxai run --help` keeps the root usage too.
    let out = sandbox.run(&["run", "--help"]);
    assert_eq!(out.code, 0, "run help must exit 0: {out:?}");
    assert!(
        out.stdout.contains("dmuxai [--help|--version]"),
        "run help must print the root usage: {out:?}"
    );
}

/// TC-2619 / FR-2604: after output has settled, `term wait --idle 1s` without
/// `--since` succeeds with reason "idle" well before the 8s timeout; the idle
/// clock starts at wait start and needs no new output after it begins. A bare
/// `--idle 500` is 500 ms (not 500 s) and also succeeds well before 8s.
#[test]
fn tc_2619_idle_wait_without_since_succeeds_on_quiet_session() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    // Produce output once and wait for it to be visible.
    let (out, _) = sandbox.term_json(&["term", "send", "a", "echo settled", "--json"]);
    assert_eq!(out.code, 0, "send failed: {out:?}");
    let (out, matched) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        "settled",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(out.code, 0, "wait --match failed: {out:?}");
    assert_eq!(matched["reason"], "match", "wait: {matched:?}");

    // No new output after this point: the idle wait must still succeed.
    let started = Instant::now();
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--idle",
        "1s",
        "--timeout",
        "8s",
        "--json",
    ]);
    let elapsed = started.elapsed();
    assert_eq!(out.code, 0, "idle wait failed: {out:?}");
    assert_eq!(waited["reason"], "idle", "wait: {waited:?}");
    assert!(
        elapsed < Duration::from_secs(8),
        "idle wait took {elapsed:?}; it must finish well before the 8s timeout"
    );

    // A fresh successful match, then the bare-number form: `--idle 500` is
    // 500 ms (not 500 s), so the wait still ends with reason idle well before
    // the 8s timeout instead of timing out (TC-2619 / FR-2604).
    let (out, matched) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--match",
        "settled",
        "--since",
        "0",
        "--timeout",
        "5s",
        "--json",
    ]);
    assert_eq!(
        out.code, 0,
        "wait --match before the bare idle wait: {out:?}"
    );
    assert_eq!(matched["reason"], "match", "wait: {matched:?}");

    let started = Instant::now();
    let (out, waited) = sandbox.term_json(&[
        "term",
        "wait",
        "a",
        "--idle",
        "500",
        "--timeout",
        "8s",
        "--json",
    ]);
    let elapsed = started.elapsed();
    assert_eq!(out.code, 0, "bare --idle 500 wait failed: {out:?}");
    assert_eq!(waited["reason"], "idle", "bare --idle 500: {waited:?}");
    assert!(
        elapsed >= Duration::from_millis(500),
        "bare --idle 500 finished after {elapsed:?}; the unitless value must mean 500 ms"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "bare --idle 500 wait took {elapsed:?}; it must finish well before the 8s timeout"
    );
}

/// TC-2620 / FR-2620: every input and output chunk lands in a masked io.jsonl
/// beside a manifest.json with dir 0700 and files 0600.
#[test]
fn tc_2620_journal_masked_jsonl_and_modes() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    let token = "sk-test-AAAAAAAAAAAAAAAAAAAAAAAA";
    sandbox.open_shell("a");

    let (out, from_show) = sandbox.term_json(&["term", "read", "a", "--json"]);
    assert_eq!(out.code, 0, "read failed: {out:?}");
    let _since = from_show["seq_to"].as_u64().expect("seq_to");
    let (out, _) = sandbox.term_json(&["term", "send", "a", &format!("echo {token}"), "--json"]);
    assert_eq!(out.code, 0, "send failed: {out:?}");

    // Output is coalesced (<= 50 ms / 8 KiB) and flushed by the host loop.
    let journal_ready = poll_until(Duration::from_secs(3), || {
        sandbox.terms_dir().join("manifest.json").exists() || {
            let Ok(entries) = std::fs::read_dir(sandbox.terms_dir()) else {
                return false;
            };
            for entry in entries.flatten() {
                let io = entry.path().join("io.jsonl");
                let Ok(content) = std::fs::read_to_string(&io) else {
                    continue;
                };
                if content.lines().any(|line| line.contains("\"dir\":\"out\"")) {
                    return true;
                }
            }
            false
        }
    });
    assert!(journal_ready, "io.jsonl never recorded an out line");

    let entries = std::fs::read_dir(sandbox.terms_dir())
        .expect("terms dir exists")
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let session_dirs: Vec<PathBuf> = entries
        .iter()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_owned();
            (name.len() == 16 && name.chars().all(|c| c.is_ascii_hexdigit())).then_some(path)
        })
        .collect();
    assert_eq!(
        session_dirs.len(),
        1,
        "one 16-hex session dir: {session_dirs:?}"
    );
    let session_dir = &session_dirs[0];

    let dir_mode = std::fs::metadata(sandbox.terms_dir())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700, "terms dir mode 0700");
    let dir_mode = std::fs::metadata(session_dir).unwrap().permissions().mode();
    assert_eq!(dir_mode & 0o777, 0o700, "session dir mode 0700");

    let manifest_path = session_dir.join("manifest.json");
    let manifest_mode = std::fs::metadata(&manifest_path)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(manifest_mode & 0o777, 0o600, "manifest mode 0600");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest["schema_version"], 1, "manifest: {manifest:?}");
    assert_eq!(manifest["name"], "a", "manifest: {manifest:?}");

    let io_path = session_dir.join("io.jsonl");
    let io_mode = std::fs::metadata(&io_path).unwrap().permissions().mode();
    assert_eq!(io_mode & 0o777, 0o600, "io.jsonl mode 0600");

    let content = std::fs::read_to_string(&io_path).expect("io.jsonl");
    let mut saw_in = false;
    let mut saw_out = false;
    for line in content.lines() {
        let line_value: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("journal line {error}: {line}"));
        for field in ["seq", "t_ms", "dir", "by", "run_id", "data", "bytes"] {
            // run_id is legitimately null outside run contexts.
            assert!(
                line_value.get(field).is_some(),
                "journal line missing {field}: {line_value:?}"
            );
        }
        match line_value["dir"].as_str().expect("dir field") {
            "in" => {
                saw_in = true;
                assert!(
                    !line_value["data"].as_str().unwrap().contains(token),
                    "input line leaks the token: {line}"
                );
            }
            "out" => {
                saw_out = true;
                assert!(
                    !line_value["data"].as_str().unwrap().contains(token),
                    "output line leaks the token: {line}"
                );
            }
            _ => {}
        }
    }
    assert!(saw_in, "io.jsonl has an in line:\n{content}");
    assert!(saw_out, "io.jsonl has an out line:\n{content}");
    assert!(
        content.contains("[REDACTED]"),
        "secret token got masked in the journal:\n{content}"
    );
    assert!(!content.contains(token), "raw token leaked:\n{content}");
}

/// TC-2632 / FR-2632: the child environment is scrubbed like runner children
/// (DMUXAI_WEB_TOKEN removed) while `--env K=V` entries are added afterwards.
#[test]
fn tc_2632_child_env_scrub_and_explicit_env() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    let (out, opened) =
        sandbox.term_json(&["term", "open", "a", "--env", "DMUXAI_CUSTOM=f5", "--json"]);
    assert_eq!(out.code, 0, "open failed: {out:?}");
    assert_eq!(opened["state"], "open", "opened: {opened:?}");

    // Ambient DMUXAI_WEB_TOKEN never reaches the child.
    let (_, exec) = sandbox.term_json(&[
        "term",
        "exec",
        "a",
        "echo ${DMUXAI_WEB_TOKEN:-unset}",
        "--json",
    ]);
    let text = exec["text"].as_str().unwrap_or_default();
    assert!(text.contains("unset"), "web token scrubbed: {exec:?}");
    assert!(
        !text.contains("ambient-web-token-value"),
        "token leaked: {exec:?}"
    );

    // `--env K=V` is added after scrubbing.
    let (_, exec) = sandbox.term_json(&[
        "term",
        "exec",
        "a",
        "echo ${DMUXAI_CUSTOM:-missing}",
        "--json",
    ]);
    let text = exec["text"].as_str().unwrap_or_default();
    assert!(text.contains("f5"), "explicit env delivered: {exec:?}");
}

/// TC-2690 / NFR-2690: a non-term command (`dmuxai status`) starts no host and
/// creates no term socket or socket directory.
#[test]
fn tc_2690_non_term_command_never_starts_host() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    let (out, json) = sandbox.term_json(&["status", "--json"]);
    assert_eq!(out.code, 0, "status failed: {out:?}");
    assert!(json.is_object(), "status json: {json:?}");

    assert!(
        sandbox.host_pids().is_empty(),
        "status must not start a host"
    );
    assert!(
        !sandbox.socket_file().exists(),
        "status must not create a term socket"
    );
    let (out, listed) = sandbox.term_json(&["term", "list", "--json"]);
    assert_eq!(out.code, 0, "list failed: {out:?}");
    assert_eq!(listed, json!([]), "list: {listed:?}");
    assert!(sandbox.host_pids().is_empty(), "list must not start a host");
}

/// TC-2691 / NFR-2691: on a warm host, send+read round trips are fast; the gate
/// asserts the loose p50 <= 90 ms bound from the spec.
#[test]
fn tc_2691_send_read_p50_within_loose_bound() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    // Warm-up round trips (binary page cache, socket path resolution).
    for _ in 0..2 {
        let (out, _) = sandbox.term_json(&["term", "send", "a", "--no-newline", ">", "--json"]);
        assert_eq!(out.code, 0);
        let (out, _) = sandbox.term_json(&["term", "read", "a", "--since", "0", "--json"]);
        assert_eq!(out.code, 0);
    }

    let mut round_trips = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let (out, _) = sandbox.term_json(&["term", "send", "a", "--no-newline", ".", "--json"]);
        assert_eq!(out.code, 0);
        let (out, _) = sandbox.term_json(&["term", "read", "a", "--since", "0", "--json"]);
        assert_eq!(out.code, 0);
        round_trips.push(started.elapsed().as_millis() as u64);
    }
    round_trips.sort_unstable();
    let p50 = round_trips[10];
    eprintln!("TC-2691 send+read round trips (ms): sorted={round_trips:?} p50={p50}");
    assert!(p50 <= 90, "p50 {p50} ms exceeds the loose 90 ms bound");
}

/// TC-2692 / NFR-2692: exec vs. `bash -c` overhead stays within the loose x3
/// bound for a ~1 s class of commands.
#[test]
fn tc_2692_exec_overhead_within_loose_bound() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    // Warm the shell and host before measuring.
    let (out, warm) = sandbox.term_json(&["term", "exec", "a", "true", "--json"]);
    assert_eq!(out.code, 0, "warmup exec failed: {warm:?}");

    let base_started = Instant::now();
    let base_status = Command::new("bash")
        .arg("-c")
        .arg("sleep 0.5")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run bash -c");
    let base_ms = base_started.elapsed().as_millis() as u64;
    assert!(base_status.success(), "bash -c must succeed");

    let term_started = Instant::now();
    let (out, exec) = sandbox.term_json(&["term", "exec", "a", "sleep 0.5", "--json"]);
    assert_eq!(out.code, 0, "exec sleep failed: {exec:?}");
    assert_eq!(exec["exit_code"], 0, "exec sleep: {exec:?}");
    let term_ms = term_started.elapsed().as_millis() as u64;

    eprintln!("TC-2692 overhead: bash -c={base_ms}ms term exec={term_ms}ms");
    assert!(
        term_ms <= base_ms.saturating_mul(3).max(base_ms + 200),
        "term exec {term_ms} ms vs bash -c {base_ms} ms exceeds the x3 bound"
    );
}

/// TC-2693 / NFR-2693: while one client connection is parked in `term wait`,
/// a second client's `term read` is served immediately instead of queueing
/// behind the wait — the host serves each accepted connection on its own
/// thread. Mirrors the in-process unit test `tc_2693_wait_does_not_block_
/// other_clients` (a read during a 2 s wait returns in under 300 ms).
#[test]
fn tc_2693_parallel_read_during_wait() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();
    sandbox.open_shell("a");

    // A never-matching regex with a 2 s timeout parks one connection in the
    // host wait path for the whole window.
    let waiter = sandbox
        .command(&[
            "term",
            "wait",
            "a",
            "--match",
            "zz-never-match-zz",
            "--timeout",
            "2s",
            "--json",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn background wait client");
    let mut waiter = ReapOnDrop(Some(waiter));

    // Give the wait client time to connect and enter the blocking wait.
    std::thread::sleep(Duration::from_millis(300));

    // A second client process reads while the first is still waiting.
    let started = Instant::now();
    let (out, read) = sandbox.term_json(&["term", "read", "a", "--json"]);
    let elapsed = started.elapsed();
    assert_eq!(out.code, 0, "read during wait failed: {out:?}");
    assert!(read["text"].is_string(), "read during wait: {read:?}");
    assert!(
        elapsed < Duration::from_millis(300),
        "read during a 2 s wait took {elapsed:?}; the host blocked it behind the wait"
    );

    // The read truly overlapped the wait: the wait client is still running.
    assert!(
        waiter.0.as_mut().unwrap().try_wait().unwrap().is_none(),
        "wait client finished before the read; the read did not overlap it"
    );

    // The wait ends on its own timeout: exit 1 with reason "timeout".
    let waited = waiter
        .0
        .take()
        .unwrap()
        .wait_with_output()
        .expect("reap wait client");
    assert_eq!(
        waited.status.code(),
        Some(1),
        "wait must exit 1 on timeout: {}",
        String::from_utf8_lossy(&waited.stdout)
    );
    let waited_json: Value = serde_json::from_slice(&waited.stdout).expect("wait json");
    assert_eq!(waited_json["reason"], "timeout", "wait: {waited_json:?}");
}

/// TC-2695 / INT-2695: `term <op> --json` always prints exactly one JSON object
/// on stdout; errors carry `error_kind` and agent_api exit codes.
#[test]
fn tc_2695_json_error_shape() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    // Unknown session, no host: the CLI starts one (connect), rejects on the
    // host response path, and emits exactly one JSON error object.
    let (out, json) = sandbox.term_json(&["term", "show", "ghost", "--json"]);
    assert_eq!(out.code, 3, "unknown show must exit 3: {out:?}");
    assert!(json.is_object(), "error json: {json:?}");
    assert_eq!(json["error_kind"], "not_found", "error json: {json:?}");

    // The non-json human path prints a diagnostic on stderr instead.
    let out = sandbox.run(&["term", "show", "ghost"]);
    assert_eq!(out.code, 3);
    assert!(
        out.stderr.contains("not found"),
        "human diagnostic on stderr: {out:?}"
    );
}
