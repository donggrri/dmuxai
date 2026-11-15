//! Headless runs journal tests (SPEC-headless-runs-dashboard FR-2800..FR-2803 / TC-2800..TC-2803).
//!
//! Verifies that headless runs create an active manifest before dispatch, record terminal status,
//! copy masked progress lines to `<run dir>/progress.jsonl`, and tolerate journal storage failures.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
    data_home: PathBuf,
}

impl Sandbox {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let workspace = root.join("ws");
        let data_home = root.join("data");
        let state_home = root.join("state");
        let cfg_home = root.join("cfg");
        let home = root.join("home");

        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&data_home).unwrap();
        fs::create_dir_all(&state_home).unwrap();
        fs::create_dir_all(&cfg_home).unwrap();
        fs::create_dir_all(&home).unwrap();

        fs::write(workspace.join("dmuxai.toml"), config).unwrap();

        Self {
            _dir: dir,
            root,
            workspace,
            data_home,
        }
    }

    fn command(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
        cmd.args(args)
            .current_dir(&self.workspace)
            .env("DMUXAI_WORKSPACE", &self.workspace)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("HOME", self.root.join("home"))
            .env("DMUXAI_TRUST_WORKSPACE_CONFIG", "1")
            .env_remove("DMUXAI_STATE_DIR")
            .env_remove("DMUXAI_WEB_TOKEN")
            .env_remove("DMUXAI_OPENCODE_BIN")
            .env_remove("DMUXAI_GROK_BIN");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with_env(args, &[])
    }

    fn run_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.command(args, env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("execute dmuxai")
    }

    fn spawn_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Child {
        self.command(args, env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn dmuxai")
    }

    fn repo_runs_dir(&self) -> PathBuf {
        let key = dmuxai::state_path::repo_key(&self.workspace).expect("repo_key");
        self.data_home
            .join("dmuxai")
            .join("repos")
            .join(key)
            .join("runs")
    }
}

fn stdout_json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "failed to parse stdout as JSON: {e}\nstdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn fake_agent_config(behavior: &str) -> String {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-agent.sh");
    format!(
        r#"[graph]
enabled = false

[[agents.custom]]
name = "fake"
command = ["sh", "{fixture}", "{behavior}", "{{prompt}}"]
prompt_via = "arg"
output = "jsonl"
text_path = "/part/text"
text_when = {{ path = "/type", equals = "text" }}
error_path = "/error/message"
timeout_secs = 30
"#,
        fixture = fixture.display().to_string().replace('\\', "/"),
    )
}

// TC-2800 / FR-2800: active manifest before dispatch, pid present, origin cli_headless or mcp
#[cfg(unix)]
#[test]
fn tc_2800_active_manifest_before_dispatch() {
    // 1. cli_headless origin
    let sandbox = Sandbox::new(&fake_agent_config("delayed"));
    let child = sandbox.spawn_with_env(&["run", "hello", "--agent", "fake", "--json"], &[]);

    let runs_dir = sandbox.repo_runs_dir();
    let start = Instant::now();
    let mut found_active: Option<Value> = None;

    while start.elapsed() < Duration::from_secs(5) {
        if runs_dir.exists()
            && let Ok(entries) = fs::read_dir(&runs_dir)
        {
            for entry in entries.flatten() {
                let manifest_path = entry.path().join("manifest.json");
                if manifest_path.exists()
                    && let Ok(content) = fs::read_to_string(&manifest_path)
                    && let Ok(val) = serde_json::from_str::<Value>(&content)
                    && val["status"] == "active"
                {
                    found_active = Some(val);
                    break;
                }
            }
        }
        if found_active.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let active_manifest = found_active.expect("expected active manifest before agent completion");
    assert_eq!(active_manifest["origin"], "cli_headless");
    assert!(
        active_manifest["pid"].is_number(),
        "expected pid in active manifest"
    );
    let active_run_id = active_manifest["run_id"]
        .as_str()
        .expect("run_id string")
        .to_string();

    let output = child.wait_with_output().expect("wait for child");
    assert_eq!(output.status.code(), Some(0));
    let json = stdout_json(&output);
    assert_eq!(json["run_id"], active_run_id);

    // Read manifest after completion
    let final_manifest_path = runs_dir.join(&active_run_id).join("manifest.json");
    let final_content = fs::read_to_string(final_manifest_path).expect("read final manifest");
    let final_manifest: Value = serde_json::from_str(&final_content).expect("parse final manifest");
    assert_eq!(final_manifest["status"], "completed");

    // 2. DMUXAI_RUN_ORIGIN=mcp gives origin "mcp"
    let sandbox_mcp = Sandbox::new(&fake_agent_config("jsonl"));
    let output_mcp = sandbox_mcp.run_with_env(
        &["run", "hello", "--agent", "fake", "--json"],
        &[("DMUXAI_RUN_ORIGIN", "mcp")],
    );
    assert_eq!(output_mcp.status.code(), Some(0));
    let json_mcp = stdout_json(&output_mcp);
    let run_id_mcp = json_mcp["run_id"].as_str().expect("run_id string");
    let manifest_mcp_path = sandbox_mcp
        .repo_runs_dir()
        .join(run_id_mcp)
        .join("manifest.json");
    let manifest_mcp_content = fs::read_to_string(manifest_mcp_path).expect("read mcp manifest");
    let manifest_mcp: Value =
        serde_json::from_str(&manifest_mcp_content).expect("parse mcp manifest");
    assert_eq!(manifest_mcp["origin"], "mcp");
    assert_eq!(manifest_mcp["status"], "completed");
}

// TC-2801 / FR-2801: terminal manifest status and exit code, duration >= 0, masked error summary
#[cfg(unix)]
#[test]
fn tc_2801_terminal_manifest_status_and_exit() {
    let failing_config = r#"[graph]
enabled = false

[[agents.custom]]
name = "fake"
command = ["sh", "-c", "printf '{\"type\":\"error\",\"error\":{\"message\":\"failed with API_KEY=sk-secretkey1234567890\"}}\n'; exit 1"]
output = "jsonl"
text_path = "/part/text"
error_path = "/error/message"
timeout_secs = 30
"#;
    let sandbox = Sandbox::new(failing_config);
    let output = sandbox.run(&["run", "x", "--agent", "fake", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let json = stdout_json(&output);
    assert_eq!(json["ok"], false);
    let run_id = json["run_id"].as_str().expect("run_id present");

    let manifest_path = sandbox.repo_runs_dir().join(run_id).join("manifest.json");
    let manifest_content = fs::read_to_string(manifest_path).expect("read manifest");
    let manifest: Value = serde_json::from_str(&manifest_content).expect("parse manifest");

    assert_eq!(manifest["status"], "failed");
    assert_eq!(manifest["dmuxai_exit_code"], 1);
    assert!(manifest["duration_ms"].as_u64().is_some());
    let err_summary = manifest["error_summary"]
        .as_str()
        .expect("error_summary present");
    assert!(
        !err_summary.contains("sk-secretkey1234567890"),
        "secret token must be masked: {err_summary}"
    );
    assert!(
        err_summary.contains("[REDACTED]"),
        "masked text must include [REDACTED]: {err_summary}"
    );
}

// TC-2802 / FR-2802: progress copy masked without flag
#[cfg(unix)]
#[test]
fn tc_2802_progress_copy_masked_without_flag() {
    let config = r#"[graph]
enabled = false

[[agents.custom]]
name = "fake"
command = ["sh", "-c", "printf '{\"type\":\"text\",\"part\":{\"text\":\"found TOKEN=abc123secret\"}}\n'"]
output = "jsonl"
text_path = "/part/text"
error_path = "/error/message"
timeout_secs = 30
"#;
    let sandbox = Sandbox::new(config);
    let output = sandbox.run(&["run", "hi", "--agent", "fake", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let json = stdout_json(&output);
    let run_id = json["run_id"].as_str().expect("run_id present");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains(r#""type":"progress""#),
        "stderr must contain no progress lines without --progress"
    );

    let progress_path = sandbox.repo_runs_dir().join(run_id).join("progress.jsonl");
    assert!(progress_path.exists(), "progress.jsonl must exist");
    let progress_content = fs::read_to_string(progress_path).expect("read progress.jsonl");
    let lines: Vec<&str> = progress_content.lines().collect();
    assert!(
        !lines.is_empty(),
        "progress.jsonl must have at least 1 line"
    );
    assert!(
        !progress_content.contains("abc123secret"),
        "progress copy must not contain secret token: {progress_content}"
    );
    assert!(
        progress_content.contains("[REDACTED]"),
        "progress copy must contain redacted marker: {progress_content}"
    );
}

// TC-2803 / FR-2803: journal failure does not change outcome
#[cfg(unix)]
#[test]
fn tc_2803_journal_failure_does_not_change_outcome() {
    let sandbox = Sandbox::new(&fake_agent_config("jsonl"));
    // Replace XDG_DATA_HOME with a regular file so repo_runs_dir / JournalStore fails
    let file_data_home = sandbox.root.join("data_file");
    fs::write(&file_data_home, "not a directory").unwrap();

    let output = sandbox.run_with_env(
        &["run", "hi", "--agent", "fake", "--json"],
        &[("XDG_DATA_HOME", file_data_home.to_str().unwrap())],
    );
    assert_eq!(output.status.code(), Some(0));
    let json = stdout_json(&output);
    assert_eq!(json["ok"], true);

    let stderr = String::from_utf8_lossy(&output.stderr);
    let warning_count = stderr
        .lines()
        .filter(|l| l.contains("run journal unavailable"))
        .count();
    assert_eq!(
        warning_count, 1,
        "expected exactly one journal warning on stderr, got {warning_count}:\n{stderr}"
    );
}
