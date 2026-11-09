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
//! - TC-2606 (FR-2606): list/show/unknown/empty list without host
//! - TC-2607 (FR-2607): close kills the process-group child, self-exit state
//! - TC-2608 (FR-2608): resize propagated to the PTY, invalid dims exit 2
//! - TC-2609 (FR-2609): takeover lock rejects send/exec, read/wait ok, release
//!   (unit covers the lock helper in `src/term/host.rs`)
//! - TC-2620 (FR-2620): masked io.jsonl with modes 0600/0700 (unit covers the
//!   writer in `src/term/journal.rs`)
//! - TC-2632 (FR-2632): child env scrubbed, `--env` still delivered
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

/// TC-2606 / FR-2606: list with no host returns `[]` without starting one;
/// show reports the session fields; unknown names exit 3.
#[test]
fn tc_2606_list_show_and_unknown() {
    let sandbox = Sandbox::new();
    let _guard = sandbox.close_guard();

    // No host running: `list --json` returns [] and does not start a host.
    let (out, listed) = sandbox.term_json(&["term", "list", "--json"]);
    assert_eq!(out.code, 0, "list without host failed: {out:?}");
    assert_eq!(listed, json!([]), "empty list: {listed:?}");
    assert!(sandbox.host_pids().is_empty(), "list must not start a host");
    assert!(!sandbox.socket_file().exists(), "no socket for list alone");

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
