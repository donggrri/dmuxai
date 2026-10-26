//! Integration tests for workspace configuration trust.

use std::ffi::OsString;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

struct TrustSandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
}

impl TrustSandbox {
    fn new(workspace_config: &str) -> Self {
        let sandbox = Self::bare();
        std::fs::write(sandbox.workspace.join("dmuxai.toml"), workspace_config).unwrap();
        sandbox
    }

    fn bare() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let workspace = root.join("ws");
        for sub in ["ws", "state", "cfg", "home"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }

        // Write a fake agent in the user-level config directory so plain runs succeed.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("fake-agent.sh");
        let user_toml = format!(
            r#"[graph]
enabled = false

[[agents.custom]]
name = "user-fake"
command = ["sh", "{fixture}", "jsonl", "{{prompt}}"]
prompt_via = "arg"
output = "jsonl"
text_path = "/part/text"
text_when = {{ path = "/type", equals = "text" }}
error_path = "/error/message"
timeout_secs = 30
"#,
            fixture = fixture.display().to_string().replace('\\', "/")
        );
        let user_cfg_dir = root.join("cfg").join("dmuxai");
        std::fs::create_dir_all(&user_cfg_dir).unwrap();
        std::fs::write(user_cfg_dir.join("dmuxai.toml"), user_toml).unwrap();

        Self {
            _dir: dir,
            root,
            workspace,
        }
    }

    fn command(&self, args: &[&str], env: &[(&str, OsString)]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
        cmd.args(args)
            .current_dir(&self.workspace)
            .env("DMUXAI_WORKSPACE", &self.workspace)
            .env("DMUXAI_STATE_DIR", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("APPDATA", self.root.join("cfg"))
            .env("HOME", self.root.join("home"))
            .env_remove("DMUXAI_TRUST_WORKSPACE_CONFIG")
            .env_remove("DMUXAI_WEB_TOKEN")
            .env_remove("DMUXAI_OPENCODE_BIN")
            .env("DMUXAI_GROK_BIN", "/nonexistent/grok")
            .env("DMUXAI_CURSOR_BIN", "/nonexistent/cursor")
            .env("DMUXAI_AGY_BIN", "/nonexistent/agy")
            .env("DMUXAI_CODEX_BIN", "/nonexistent/codex");

        cmd.envs(env.iter().map(|(key, value)| (*key, value.clone())));
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in_env(args, &[])
    }

    fn run_in_env(&self, args: &[&str], env: &[(&str, OsString)]) -> Output {
        self.command(args, env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("execute dmuxai")
            .wait_with_output()
            .expect("wait for dmuxai")
    }

    fn state_store_path(&self) -> PathBuf {
        self.root.join("state").join("trusted-workspaces.json")
    }
}

fn stdout_json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn wait_for_marker(marker_path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if marker_path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    marker_path.exists()
}

// Case 1: untrusted: run exits 0, marker file absent, stderr contains "not trusted",
// and `--agent ws-agent` is rejected as unknown.
#[test]
fn tc_2207_untrusted_workspace_config() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let marker = tmp_dir.path().join("marker");
    let ws_toml = format!(
        r#"[notify]
command = ["sh", "-c", "echo x >> {}"]
events = ["pending", "started", "running", "completed"]

[[agents.custom]]
name = "ws-agent"
command = ["echo", "ws"]
"#,
        marker.display()
    );
    let sandbox = TrustSandbox::new(&ws_toml);

    // 1a) Run exits 0 with user-fake, marker file absent, stderr contains "not trusted"
    let output = sandbox.run(&["run", "hello", "--agent", "user-fake", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not trusted"),
        "stderr should mention not trusted: {stderr}"
    );
    assert!(
        !wait_for_marker(&marker, Duration::from_millis(500)),
        "marker file should not be created for untrusted config"
    );

    // 1b) `--agent ws-agent` is rejected as unknown
    let output_ws_agent = sandbox.run(&["run", "hello", "--agent", "ws-agent", "--json"]);
    assert_ne!(output_ws_agent.status.code(), Some(0));
    let ws_stderr = String::from_utf8_lossy(&output_ws_agent.stderr);
    let ws_stdout = String::from_utf8_lossy(&output_ws_agent.stdout);
    assert!(
        ws_stderr.contains("unknown agent 'ws-agent'")
            || ws_stdout.contains("unknown agent 'ws-agent'")
            || ws_stderr.contains("unknown runner 'ws-agent'")
            || ws_stdout.contains("unknown runner 'ws-agent'"),
        "ws-agent should be rejected as unknown. stdout: {ws_stdout}, stderr: {ws_stderr}"
    );
}

// Case 2: DMUXAI_TRUST_WORKSPACE_CONFIG=1: marker created; store file NOT created.
#[test]
fn tc_2208_trust_via_env_var() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let marker = tmp_dir.path().join("marker");
    let ws_toml = format!(
        r#"[notify]
command = ["sh", "-c", "echo x >> {}"]
events = ["pending", "started", "running", "completed"]
"#,
        marker.display()
    );
    let sandbox = TrustSandbox::new(&ws_toml);

    let output = sandbox.run_in_env(
        &["run", "hello", "--agent", "user-fake", "--json"],
        &[("DMUXAI_TRUST_WORKSPACE_CONFIG", OsString::from("1"))],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(
        wait_for_marker(&marker, Duration::from_secs(3)),
        "marker file should be created when DMUXAI_TRUST_WORKSPACE_CONFIG=1"
    );
    assert!(
        !sandbox.state_store_path().exists(),
        "store file must NOT be created on env opt-in"
    );
}

// Case 3: dmuxai run ... --trust-workspace-config: marker created; store file not created.
#[test]
fn tc_2209_trust_via_cli_flag() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let marker = tmp_dir.path().join("marker");
    let ws_toml = format!(
        r#"[notify]
command = ["sh", "-c", "echo x >> {}"]
events = ["pending", "started", "running", "completed"]
"#,
        marker.display()
    );
    let sandbox = TrustSandbox::new(&ws_toml);

    let output = sandbox.run(&[
        "run",
        "hello",
        "--agent",
        "user-fake",
        "--json",
        "--trust-workspace-config",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        wait_for_marker(&marker, Duration::from_secs(3)),
        "marker file should be created when --trust-workspace-config is passed"
    );
    assert!(
        !sandbox.state_store_path().exists(),
        "store file must not be created on flag opt-in"
    );
}

// Case 4: dmuxai trust without --yes on non-TTY: exit 1, store file not created.
// dmuxai trust --yes: exit 0, store file created; then a plain run creates the marker.
// dmuxai trust --status --json reports trusted.
#[test]
fn tc_2210_trust_cli_command_lifecycle() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let marker = tmp_dir.path().join("marker");
    let ws_toml = format!(
        r#"[notify]
command = ["sh", "-c", "echo x >> {}"]
events = ["pending", "started", "running", "completed"]
"#,
        marker.display()
    );
    let sandbox = TrustSandbox::new(&ws_toml);

    // 4a) dmuxai trust without --yes on non-TTY: exit 1, store file not created
    let output_no_yes = sandbox.run(&["trust"]);
    assert_eq!(output_no_yes.status.code(), Some(1));
    assert!(!sandbox.state_store_path().exists());

    // 4b) dmuxai trust --yes: exit 0, store file created
    let output_yes = sandbox.run(&["trust", "--yes"]);
    assert_eq!(output_yes.status.code(), Some(0));
    assert!(sandbox.state_store_path().exists());

    // 4c) Plain run creates the marker without trust flags or env
    let output_run = sandbox.run(&["run", "hello", "--agent", "user-fake", "--json"]);
    assert_eq!(output_run.status.code(), Some(0));
    assert!(
        wait_for_marker(&marker, Duration::from_secs(3)),
        "marker file should be created after trusting workspace"
    );

    // 4d) dmuxai trust --status --json reports trusted
    let output_status = sandbox.run(&["trust", "--status", "--json"]);
    assert_eq!(output_status.status.code(), Some(0));
    let status_json = stdout_json(&output_status);
    assert_eq!(status_json["status"], "trusted");
}

// Case 5: after trusting, append a comment line to dmuxai.toml: plain run ->
// marker not (re)created (delete marker before), status --json reports untrusted.
#[test]
fn tc_2211_modified_config_invalidates_trust() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let marker = tmp_dir.path().join("marker");
    let ws_toml = format!(
        r#"[notify]
command = ["sh", "-c", "echo x >> {}"]
events = ["pending", "started", "running", "completed"]
"#,
        marker.display()
    );
    let sandbox = TrustSandbox::new(&ws_toml);

    // Trust it first
    let output_yes = sandbox.run(&["trust", "--yes"]);
    assert_eq!(output_yes.status.code(), Some(0));

    // Append a comment line to dmuxai.toml
    let modified = format!("{ws_toml}\n# modified comment line\n");
    std::fs::write(sandbox.workspace.join("dmuxai.toml"), modified).unwrap();

    // Ensure marker does not exist
    if marker.exists() {
        std::fs::remove_file(&marker).unwrap();
    }

    // Plain run -> marker not created
    let output_run = sandbox.run(&["run", "hello", "--agent", "user-fake", "--json"]);
    assert_eq!(output_run.status.code(), Some(0));
    assert!(
        !wait_for_marker(&marker, Duration::from_millis(500)),
        "marker should not be created for modified untrusted config"
    );

    // status --json reports untrusted
    let output_status = sandbox.run(&["trust", "--status", "--json"]);
    assert_eq!(output_status.status.code(), Some(0));
    let status_json = stdout_json(&output_status);
    assert_eq!(status_json["status"], "untrusted");
}

// Case 6: dmuxai trust --revoke -> status untrusted.
#[test]
fn tc_2212_trust_revoke() {
    let ws_toml = r#"[notify]
command = ["echo", "ws"]
events = ["completed"]
"#;
    let sandbox = TrustSandbox::new(ws_toml);

    // Trust it first
    let output_yes = sandbox.run(&["trust", "--yes"]);
    assert_eq!(output_yes.status.code(), Some(0));

    let status1 = sandbox.run(&["trust", "--status", "--json"]);
    assert_eq!(stdout_json(&status1)["status"], "trusted");

    // Revoke
    let output_revoke = sandbox.run(&["trust", "--revoke"]);
    assert_eq!(output_revoke.status.code(), Some(0));

    // Status is untrusted
    let status2 = sandbox.run(&["trust", "--status", "--json"]);
    assert_eq!(stdout_json(&status2)["status"], "untrusted");
}

// Case 7: webhook: start a std::net::TcpListener on 127.0.0.1:0 in the test,
// workspace `[notify] webhook_url = "http://127.0.0.1:<port>/hook"`,
// `webhook_secret_env = "DMUXAI_TEST_WS_SECRET"`, set that env var in the child;
// untrusted run -> after the run exits, listener (set_nonblocking, poll ~2s) received no connection.
// With DMUXAI_TRUST_WORKSPACE_CONFIG=1 -> a connection is received (read the request; assert it contains "POST").
// Do not print the secret value in assertion messages.
#[test]
fn tc_2214_webhook_trust_enforcement() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().unwrap().port();
    listener
        .set_nonblocking(true)
        .expect("set nonblocking listener");

    let ws_toml = format!(
        r#"[notify]
webhook_url = "http://127.0.0.1:{port}/hook"
webhook_secret_env = "DMUXAI_TEST_WS_SECRET"
events = ["pending", "started", "running", "completed"]
"#
    );
    let sandbox = TrustSandbox::new(&ws_toml);

    // 7a) Untrusted run -> after run exits, listener received no connection
    let output_untrusted = sandbox.run_in_env(
        &["run", "hello", "--agent", "user-fake", "--json"],
        &[("DMUXAI_TEST_WS_SECRET", OsString::from("super-secret-123"))],
    );
    assert_eq!(output_untrusted.status.code(), Some(0));

    // Poll for connection for ~2s
    let poll_deadline = Instant::now() + Duration::from_secs(2);
    let mut got_connection = false;
    while Instant::now() < poll_deadline {
        match listener.accept() {
            Ok(_) => {
                got_connection = true;
                break;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("listener accept error: {e}"),
        }
    }
    assert!(
        !got_connection,
        "untrusted run should NOT deliver webhook connection"
    );

    // 7b) With DMUXAI_TRUST_WORKSPACE_CONFIG=1 -> connection received, contains "POST"
    let output_trusted = sandbox.run_in_env(
        &["run", "hello", "--agent", "user-fake", "--json"],
        &[
            ("DMUXAI_TEST_WS_SECRET", OsString::from("super-secret-123")),
            ("DMUXAI_TRUST_WORKSPACE_CONFIG", OsString::from("1")),
        ],
    );
    assert_eq!(output_trusted.status.code(), Some(0));

    let mut stream_opt = None;
    let poll_deadline_trusted = Instant::now() + Duration::from_secs(4);
    while Instant::now() < poll_deadline_trusted {
        match listener.accept() {
            Ok((stream, _)) => {
                stream_opt = Some(stream);
                break;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("listener accept error: {e}"),
        }
    }
    let mut stream = stream_opt.expect("expected webhook connection when trusted");
    stream
        .set_nonblocking(false)
        .expect("set blocking for stream");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request_str = String::from_utf8_lossy(&buf[..n]);
    assert!(
        request_str.contains("POST"),
        "webhook request should contain POST"
    );
}
