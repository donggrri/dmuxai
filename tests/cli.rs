use std::process::{Command, Stdio};

const EXPECTED_NOTTY_MESSAGE: &str =
    "dmuxai needs an interactive terminal (TTY); run it in a terminal, or see dmuxai --help";

/// TC-380 / FR-380: `dmuxai --help` and `-h` print usage and exit 0 without requiring a TTY.
#[test]
fn help_flag_prints_usage_without_tty() {
    for flag in ["--help", "-h"] {
        let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .arg(flag)
            .stdin(Stdio::null())
            .output()
            .expect("execute dmuxai");

        assert_eq!(
            output.status.code(),
            Some(0),
            "expected exit code 0 for {flag}, got: {:?}",
            output.status
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("DmuxAI"),
            "stdout missing DmuxAI description for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("dmuxai [--help|--version]"),
            "stdout missing usage pattern for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("DMUXAI_FORCE_COLOR"),
            "stdout missing DMUXAI_FORCE_COLOR for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("NO_COLOR"),
            "stdout missing NO_COLOR for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("XDG_STATE_HOME"),
            "stdout missing XDG_STATE_HOME for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("dmuxai.toml"),
            "stdout missing dmuxai.toml for {flag}: {stdout}"
        );
    }
}

/// TC-381 / FR-381: `dmuxai --version` and `-V` print version string and exit 0 without a TTY.
#[test]
fn version_flag_prints_version_without_tty() {
    let expected = format!("dmuxai {}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .arg(flag)
            .stdin(Stdio::null())
            .output()
            .expect("execute dmuxai");

        assert_eq!(
            output.status.code(),
            Some(0),
            "expected exit code 0 for {flag}, got: {:?}",
            output.status
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout, expected,
            "expected version string {expected}, got {stdout}"
        );
    }
}

/// TC-382 / FR-380: unknown flags print an error message, show usage, and exit 2.
#[test]
fn unknown_flag_exits_with_code_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .arg("--unknown-flag")
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2, got: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: unexpected argument '--unknown-flag'"),
        "stderr missing error description: {stderr}"
    );
    assert!(
        stderr.contains("dmuxai [--help|--version]"),
        "stderr missing usage text: {stderr}"
    );
}

/// TC-382 / FR-382: starting without a TTY prints a clear error message to stderr and exits 1 without "Os code 6".
#[test]
fn non_tty_start_fails_with_clear_message() {
    let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for non-TTY start, got: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(EXPECTED_NOTTY_MESSAGE),
        "expected clear non-TTY error message, got: {stderr}"
    );
    assert!(
        !stderr.contains("Os { code: 6"),
        "stderr should not mention Os code 6, got: {stderr}"
    );
}

/// TC-871 / FR-870: `dmuxai -C <a> status --json` from cwd `<b>` reports `<a>`;
/// `-C /nonexistent` exits 2 with one line on stderr.
#[test]
fn workspace_dir_flag_sets_workspace() {
    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");
    let state_dir = tempfile::tempdir().expect("state dir");
    let canon_a = dir_a
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // Test `-C <dir>`, `--workspace-dir <dir>`, and `--workspace-dir=<dir>`.
    for flag_args in [
        vec!["-C", &canon_a, "status", "--json"],
        vec!["--workspace-dir", &canon_a, "status", "--json"],
        vec![&format!("--workspace-dir={canon_a}"), "status", "--json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .args(&flag_args)
            .current_dir(dir_b.path())
            .env("DMUXAI_STATE_DIR", state_dir.path())
            .env_remove("DMUXAI_WORKSPACE")
            .stdin(Stdio::null())
            .output()
            .expect("execute dmuxai");

        assert_eq!(
            output.status.code(),
            Some(0),
            "expected exit 0 for {flag_args:?}, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("status json");
        assert_eq!(parsed["workspace"].as_str(), Some(canon_a.as_str()));
    }

    // Missing directory exits 2 with a single line on stderr.
    let missing = dir_b.path().join("nonexistent-dir-871");
    let missing_str = missing.to_string_lossy().into_owned();
    let bad = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["-C", &missing_str, "status", "--json"])
        .current_dir(dir_b.path())
        .env("DMUXAI_STATE_DIR", state_dir.path())
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");

    assert_eq!(bad.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&bad.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one stderr line, got: {stderr:?}"
    );
    assert_eq!(lines[0], format!("error: -C {missing_str}: does not exist"));
}

/// Headless plan gate helpers (TC-955 / FR-947, FR-944). Unix only (fake-agent.sh).
#[cfg(unix)]
mod plan_gate {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};

    use serde_json::Value;

    pub struct Sandbox {
        _dir: tempfile::TempDir,
        root: PathBuf,
        workspace: PathBuf,
    }

    impl Sandbox {
        pub fn new(config: &str) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let root = dir.path().to_path_buf();
            let workspace = root.join("ws");
            for sub in ["ws", "state", "cfg", "home"] {
                std::fs::create_dir_all(root.join(sub)).unwrap();
            }
            std::fs::write(workspace.join("dmuxai.toml"), config).unwrap();
            Self {
                _dir: dir,
                root,
                workspace,
            }
        }

        pub fn workspace(&self) -> &std::path::Path {
            &self.workspace
        }

        pub fn run(&self, args: &[&str]) -> Output {
            Command::new(env!("CARGO_BIN_EXE_dmuxai"))
                .args(args)
                .current_dir(&self.workspace)
                .env("DMUXAI_WORKSPACE", &self.workspace)
                .env("DMUXAI_STATE_DIR", self.root.join("state"))
                .env("XDG_CONFIG_HOME", self.root.join("cfg"))
                .env("APPDATA", self.root.join("cfg"))
                .env("HOME", self.root.join("home"))
                .env_remove("DMUXAI_WEB_TOKEN")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .expect("execute dmuxai")
        }
    }

    pub fn stdout_json(output: &Output) -> Value {
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "stdout is not one JSON document ({e}): {stdout}\nstderr: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
    }

    pub fn plan_gate_config(behavior: &str) -> String {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("fake-agent.sh");
        format!(
            r#"[routing]
default_agent = "fake"

[graph]
allowed_runners = ["fake"]

[[agents.custom]]
name = "fake"
command = ["sh", "{fixture}", "{behavior}", "{{prompt}}"]
prompt_via = "arg"
output = "text"
default_model = "composer-2.5"
timeout_secs = 60
"#,
            fixture = fixture.display().to_string().replace('\\', "/"),
            behavior = behavior,
        )
    }
}

/// TC-955: complex plan needs `--approve-plan`; `--agent` sets triage override.
#[cfg(unix)]
#[test]
fn run_complex_needs_approve_plan() {
    use dmuxai::graph::{GraphRunPhase, load_run};
    use plan_gate::{Sandbox, plan_gate_config, stdout_json};

    let sandbox = Sandbox::new(&plan_gate_config("plan"));
    let first = sandbox.run(&["run", "ship the feature", "--json"]);
    let json = stdout_json(&first);
    assert_eq!(first.status.code(), Some(7), "{json}");
    assert_eq!(json["ok"], false);
    assert_eq!(json["exit_code"], 7);
    assert_eq!(json["error"].as_str(), Some("approval required (headless)"));
    assert_eq!(json["triage"]["source"].as_str(), Some("judge"));
    assert_eq!(json["triage"]["route"].as_str(), Some("complex"));
    assert!(
        json["plan"]["steps"]
            .as_array()
            .is_some_and(|s| s.len() >= 2)
    );
    let graph_id = json["graph_id"].as_str().expect("graph_id");
    let run = load_run(sandbox.workspace(), graph_id).expect("saved run");
    assert_eq!(run.phase, GraphRunPhase::Cancelled);
    assert!(
        run.plan_history
            .iter()
            .any(|change| change.reason == "approval required (headless)")
    );

    let approved = sandbox.run(&["run", "ship the feature", "--json", "--approve-plan"]);
    let approved_json = stdout_json(&approved);
    assert_eq!(approved.status.code(), Some(0), "{approved_json}");
    assert_eq!(approved_json["ok"], true);
    let graph_id = approved_json["graph_id"].as_str().expect("graph_id");
    let run = load_run(sandbox.workspace(), graph_id).expect("approved run");
    assert_eq!(
        run.approval
            .as_ref()
            .and_then(|approval| approval.decided_by),
        Some(dmuxai::graph::DecisionBy::Cli)
    );

    let direct = sandbox.run(&["run", "ship the feature", "--json", "--agent", "fake"]);
    let direct_json = stdout_json(&direct);
    assert_eq!(direct_json["triage"]["source"].as_str(), Some("override"));
}

/// FR-944: sensitive keyword in headless simple run exits 7 at the gate.
#[cfg(unix)]
#[test]
fn run_sensitive_gate_exits_7_headless() {
    use dmuxai::graph::{GraphRunPhase, load_run};
    use plan_gate::{Sandbox, plan_gate_config, stdout_json};

    let sandbox = Sandbox::new(&plan_gate_config("judge-simple"));
    let output = sandbox.run(&["run", "deploy the app", "--json"]);
    let json = stdout_json(&output);
    assert_eq!(output.status.code(), Some(7), "{json}");
    assert_eq!(
        json["error"].as_str(),
        Some("sensitive step needs /graph continue (headless)")
    );
    let graph_id = json["graph_id"].as_str().expect("graph_id");
    let run = load_run(sandbox.workspace(), graph_id).expect("gated run");
    assert_eq!(run.phase, GraphRunPhase::WaitingGate);
    assert!(
        run.plan_history
            .iter()
            .any(|change| change.reason == "sensitive step needs /graph continue (headless)")
    );

    let with_approve = sandbox.run(&["run", "deploy the app", "--json", "--approve-plan"]);
    let approve_json = stdout_json(&with_approve);
    assert_eq!(with_approve.status.code(), Some(7), "{approve_json}");
    assert_eq!(
        approve_json["error"].as_str(),
        Some("sensitive step needs /graph continue (headless)")
    );
    let graph_id = approve_json["graph_id"].as_str().expect("graph_id");
    let gated = load_run(sandbox.workspace(), graph_id).expect("gated run");
    assert_eq!(gated.phase, GraphRunPhase::WaitingGate);
}
