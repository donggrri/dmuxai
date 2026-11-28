//! Shared headless-Chrome helpers for the dashboard feature coverage tests
//! (SPEC-dashboard-feature-coverage §3, NFR-3750, TC-3764).
//!
//! Every test that uses [`setup_env`] gets its own temp workspace, state dir,
//! `XDG_CONFIG_HOME` and `HOME`, fake agent CLIs, no token and a port-0 server
//! (so the real user config, state and ports 7420/7472 are never touched).
//! [`run_probe`] enforces the 55 s Playwright deadline (< 60 s, NFR-3750) and
//! [`ChildGuard`] kills the server when it drops (no leftover processes).

#![allow(dead_code)]

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

/// Hard deadline for one Playwright probe (NFR-3750: each browser test < 60 s).
pub const PROBE_DEADLINE: Duration = Duration::from_secs(55);

static WEB_E2E_LOCK: Mutex<()> = Mutex::new(());

fn web_e2e_serial() -> std::sync::MutexGuard<'static, ()> {
    WEB_E2E_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Kills and reaps the dashboard process on drop; also serializes e2e tests.
pub struct ChildGuard {
    child: Option<Child>,
    _serial: std::sync::MutexGuard<'static, ()>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Temp workspace/state/config/home plus fake agent CLIs.
pub struct Env {
    pub root: tempfile::TempDir,
    pub workspace: PathBuf,
    pub state: PathBuf,
    pub config: PathBuf,
    pub home: PathBuf,
    pub fake_bin: PathBuf,
}

pub fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    let mut perms = fs::metadata(path).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

/// The shared temp-env helper (TC-3764 checks that new browser tests call it).
pub fn setup_env() -> Env {
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("ws");
    let state = root.path().join("state");
    let config = root.path().join("config");
    let home = root.path().join("home");
    let fake_bin = root.path().join("fake-bin");
    for dir in [&workspace, &state, &config, &home, &fake_bin] {
        fs::create_dir_all(dir).expect("temp dir");
    }

    // Fake runner CLIs for the Runners panel / status probes: never real agents.
    write_executable(
        &fake_bin.join("fake-cursor.sh"),
        "#!/bin/sh\ncase \"$1\" in\n  --version) echo test-cursor; exit 0 ;;\n  status) echo '{\"isAuthenticated\":false}'; exit 0 ;;\nesac\nexit 0\n",
    );
    write_executable(
        &fake_bin.join("fake-agy.sh"),
        "#!/bin/sh\ncase \"$1\" in\n  --version) echo test-agy; exit 0 ;;\nesac\nexit 1\n",
    );
    // Fake chat agent: `reply to <prompt>` at once; a prompt containing `slow`
    // streams `part1`, waits FAKE_SLOW_SECS (default 4), then `part2-final`.
    write_executable(
        &fake_bin.join("fake-chat.sh"),
        "#!/bin/sh\ncase \"$1\" in\n  *slow*)\n    printf 'part1 for %s\\n' \"$1\"\n    sleep \"${FAKE_SLOW_SECS:-4}\"\n    printf 'part2-final for %s\\n' \"$1\"\n    ;;\n  *) printf 'reply to %s\\n' \"$1\" ;;\nesac\n",
    );

    Env {
        root,
        workspace,
        state,
        config,
        home,
        fake_bin,
    }
}

impl Env {
    pub fn user_config_path(&self) -> PathBuf {
        self.config.join("dmuxai").join("dmuxai.toml")
    }

    /// Writes the user config (never the workspace config: no trust prompt needed).
    pub fn write_user_config(&self, body: &str) {
        let path = self.user_config_path();
        fs::create_dir_all(path.parent().expect("config parent")).expect("config dir");
        fs::write(&path, body).expect("write user config");
    }

    /// Registers the fake custom chat agent `fake` (see `fake-chat.sh`), graph runs off.
    pub fn write_chat_agents_config(&self, extra: &str) {
        self.write_chat_agents_config_with("[graph]\nenabled = false\n\n", extra);
    }

    /// Same agents, graph runs left at their defaults (turns get run rows).
    pub fn write_chat_agents_config_with_graph(&self, extra: &str) {
        self.write_chat_agents_config_with("", extra);
    }

    fn write_chat_agents_config_with(&self, graph: &str, extra: &str) {
        let chat = self.fake_bin.join("fake-chat.sh");
        self.write_user_config(&format!(
            "{}{graph}[routing]\ndefault_agent = \"fake\"\n\n[[agents.custom]]\nname = \"fake\"\nlabel = \"demo-agent\"\ncommand = [\"{}\", \"{{prompt}}\"]\nprompt_via = \"arg\"\n\n{extra}",
            dmuxai::settings::WORKFLOW_SPEC_FIRST_OFF_TOML,
            chat.display(),
        ));
    }

    /// Runs `dmuxai <args>` in the temp workspace with the isolated env.
    pub fn dmuxai(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
        cmd.arg("-C").arg(&self.workspace).args(args);
        cmd.stdin(Stdio::null());
        apply_web_env(&mut cmd, self, &[]);
        cmd.output().expect("run dmuxai")
    }
}

pub fn find_chrome() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DMUXAI_CHROME") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    for candidate in [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in ["google-chrome", "google-chrome-stable", "chromium"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// `Some((chrome, python))`, or prints `SKIP` (the test then passes).
pub fn prepare() -> Option<(PathBuf, String)> {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome found");
        return None;
    };
    let python = std::env::var_os("DMUXAI_E2E_PYTHON")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "python3".to_owned());
    let probe = Command::new(&python)
        .args(["-c", "import playwright"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.map(|s| s.success()).unwrap_or(false) {
        println!("SKIP: python playwright not installed");
        return None;
    }
    Some((chrome, python))
}

pub fn apply_web_env(cmd: &mut Command, env: &Env, extra: &[(&str, &str)]) {
    cmd.env("DMUXAI_STATE_DIR", &env.state)
        .env("XDG_STATE_HOME", &env.state)
        .env("XDG_CONFIG_HOME", &env.config)
        .env("HOME", &env.home)
        .env("DMUXAI_CURSOR_BIN", env.fake_bin.join("fake-cursor.sh"))
        .env("DMUXAI_AGY_BIN", env.fake_bin.join("fake-agy.sh"))
        .env_remove("DMUXAI_CONFIG")
        .env_remove("DMUXAI_WORKSPACE")
        .env_remove("DMUXAI_TRUST_WORKSPACE_CONFIG")
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WEB_REQUIRE_TOKEN")
        .env_remove("DMUXAI_WEB_TOKEN_REQUIRED");
    for (k, v) in extra {
        cmd.env(k, v);
    }
}

fn read_dashboard_url(stdout: std::process::ChildStdout) -> String {
    let (tx, rx) = mpsc::channel();
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
        let _ = tx.send(String::new());
    });
    let line = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("dashboard URL");
    line.strip_prefix("dmuxai dashboard: ")
        .unwrap_or(&line)
        .trim_end_matches('/')
        .to_string()
}

/// `dmuxai --web --port 0 -C <workspace>` (writable dashboard, no token).
pub fn start_web(env: &Env, extra_env: &[(&str, &str)]) -> (ChildGuard, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    cmd.args(["--web", "-C"])
        .arg(&env.workspace)
        .args(["--port", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    apply_web_env(&mut cmd, env, extra_env);
    let mut child = cmd.spawn().expect("spawn dmuxai --web");
    let stdout = child.stdout.take().expect("stdout");
    (
        ChildGuard {
            child: Some(child),
            _serial: web_e2e_serial(),
        },
        read_dashboard_url(stdout),
    )
}

/// `dmuxai serve --port 0` (view-only dashboard).
pub fn start_serve(env: &Env, extra_env: &[(&str, &str)]) -> (ChildGuard, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    cmd.args(["serve", "--port", "0"])
        .current_dir(&env.workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    apply_web_env(&mut cmd, env, extra_env);
    let mut child = cmd.spawn().expect("spawn dmuxai serve");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("serve URL line");
    let url = line
        .trim()
        .strip_prefix("dmuxai dashboard: ")
        .unwrap_or_else(|| panic!("unexpected serve line: {line:?}"))
        .trim_end_matches('/')
        .to_string();
    (
        ChildGuard {
            child: Some(child),
            _serial: web_e2e_serial(),
        },
        url,
    )
}

/// Runs a Playwright script (`argv[1]` = chrome, then `args`) under the 55 s
/// deadline and returns the last JSON line it printed.
pub fn run_probe(
    python: impl AsRef<OsStr>,
    chrome: &Path,
    script_body: &str,
    args: &[&str],
) -> serde_json::Value {
    let mut cmd = Command::new(python.as_ref());
    cmd.args(["-c", script_body]);
    cmd.arg(chrome);
    for a in args {
        cmd.arg(a);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = cmd.spawn().expect("spawn playwright");
    let (tx, rx) = mpsc::channel();
    let mut stdout = child.stdout.take().expect("py stdout");
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let out_bytes = match rx.recv_timeout(PROBE_DEADLINE) {
        Ok(b) => {
            let _ = child.wait();
            b
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("playwright timed out after {PROBE_DEADLINE:?}");
        }
    };
    let text = String::from_utf8_lossy(&out_bytes);
    let json_line = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON from playwright:\n{text}"));
    serde_json::from_str(json_line).expect("probe json")
}
