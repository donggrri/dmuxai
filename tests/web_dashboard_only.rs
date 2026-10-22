//! TC-1600..TC-1602 (SPEC-web-dashboard-only): `dmuxai --web` runs the full
//! app headless — the dashboard URL reaches stdout and `web.json` is published
//! even with piped stdin/stdout and no `DMUXAI_HEADLESS_DASHBOARD` (FR-1600);
//! SIGINT exits 0 and removes `web.json` (FR-1601); plain `dmuxai` still needs
//! a TTY and `--help` describes `--web` as no-TUI (FR-1602, FR-1603).

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Kills and reaps the child on every exit path, including panics.
struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Eye-catching timeout used by the test helper below (TC-1600).
const URL_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(unix)]
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Spawns `dmuxai --web -C <workspace> --port 0` with all stdio piped or null
/// (never a TTY). `DMUXAI_HEADLESS_DASHBOARD` is unset so the run proves
/// FR-1600: `--web` is headless with or without the old env var.
fn spawn_web_dashboard(
    workspace: &Path,
    state: &Path,
) -> (ChildGuard, ChildStdout, std::path::PathBuf) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    cmd.args(["--web", "-C"])
        .arg(workspace)
        .args(["--port", "0"])
        .env("XDG_STATE_HOME", state)
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WORKSPACE")
        .env_remove("DMUXAI_STATE_DIR")
        .env_remove("DMUXAI_HEADLESS_DASHBOARD")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn dmuxai --web");
    let stdout = child.stdout.take().expect("stdout is piped");

    // Drain stderr in the background so warnings never fill the pipe and
    // block the headless process (TC-1600 failure mode).
    if let Some(mut stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut discarded = Vec::new();
            let _ = stderr.read_to_end(&mut discarded);
        });
    }

    let guard = ChildGuard(Some(child));
    let discovery_path = workspace.join(".dmuxai").join("web.json");
    (guard, stdout, discovery_path)
}

/// Reads piped stdout until a line starts with the dashboard URL prefix
/// (FR-1600), or panics after [`URL_TIMEOUT`].
fn wait_for_dashboard_url(stdout: ChildStdout) -> String {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line.trim_start().starts_with("dmuxai dashboard: http") {
                let _ = tx.send(line.trim_end().to_owned());
                return;
            }
            line.clear();
        }
        let _ = tx.send(String::from("<stdout closed before the URL line>"));
    });
    let deadline = Instant::now() + URL_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(line) if line.starts_with("dmuxai dashboard: http") => return line,
            Ok(line) if line.is_empty() => continue,
            Ok(line) => panic!("unexpected stdout line before the URL: {line:?}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("timed out after {URL_TIMEOUT:?} waiting for `dmuxai dashboard:`")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("stdout reader thread ended before `dmuxai dashboard:`")
            }
        }
    }
}

fn wait_for_file(path: &Path, timeout: Duration, what: &str) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        assert!(Instant::now() < deadline, "no {what} within {timeout:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

// TC-1600 (FR-1600): `dmuxai --web --port 0` runs headless without a TTY and
// prints the dashboard URL to stdout.
#[test]
fn tc_1600_web_runs_without_tty() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let workspace = tmp.path().join("ws");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::create_dir_all(&state).expect("state dir");

    let (guard, stdout, discovery_path) = spawn_web_dashboard(&workspace, &state);
    let _guard = guard;

    let url = wait_for_dashboard_url(stdout);
    assert!(
        url.starts_with("dmuxai dashboard: http://"),
        "expected an http dashboard URL, got {url:?}"
    );
    wait_for_file(&discovery_path, Duration::from_secs(20), "web.json");
}

// TC-1601 (FR-1601): SIGINT ends the headless process with status 0 and the
// discovery file is removed. Unix only — SIGINT semantics.
#[cfg(unix)]
#[test]
fn tc_1601_web_exits_on_sigint() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let workspace = tmp.path().join("ws");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::create_dir_all(&state).expect("state dir");

    let (guard, stdout, discovery_path) = spawn_web_dashboard(&workspace, &state);
    let mut guard = guard;
    wait_for_dashboard_url(stdout);
    wait_for_file(&discovery_path, Duration::from_secs(20), "web.json");

    let mut child = guard.0.take().expect("child alive");
    let pid = child.id();

    // Deliver SIGINT via kill(1) by pid.
    let kill = Command::new("sh")
        .args(["-c", &format!("kill -INT {pid}")])
        .status()
        .expect("run kill -INT");
    assert!(kill.success(), "kill -INT {pid} failed");

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    let remaining = deadline.saturating_duration_since(Instant::now());
    let status = match rx.recv_timeout(remaining) {
        Ok(status) => status,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("SIGINT shutdown did not finish within {SHUTDOWN_TIMEOUT:?}")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("SIGINT shutdown watcher thread ended unexpectedly")
        }
    };
    let status = status.expect("wait on SIGINT-shutdown child");
    assert!(status.success(), "exit code after SIGINT: {status:?}");

    assert!(
        !discovery_path.exists(),
        "web.json should be removed after shutdown"
    );
}

// TC-1602 (FR-1602, FR-1603): plain `dmuxai` without a TTY still exits with
// the TTY error, and `--help` describes `--web` as no-TUI.
#[test]
fn tc_1602_plain_dmuxai_needs_tty_and_help_mentions_no_tui() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");

    let base = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
        cmd.env("XDG_STATE_HOME", &state)
            .env_remove("DMUXAI_WEB_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.current_dir(&workspace);
        for arg in args {
            cmd.arg(arg);
        }
        cmd
    };

    let out = base(&["-C", "."]).output().expect("run plain dmuxai");
    assert!(
        !out.status.success(),
        "plain dmuxai must fail without a TTY"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("interactive terminal"),
        "expected the TTY error on stderr, got: {stderr:?}"
    );

    let out = base(&["--help"]).output().expect("run dmuxai --help");
    assert!(out.status.success(), "--help must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Run the local dashboard with agents (no TUI)"),
        "--help must describe --web as no-TUI, got: {stdout:?}"
    );
}
