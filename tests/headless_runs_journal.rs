//! Headless runs journal tests (SPEC-headless-runs-dashboard FR-2800..FR-2803 / TC-2800..TC-2803).
//!
//! Verifies that headless runs create an active manifest before dispatch, record terminal status,
//! copy masked progress lines to `<run dir>/progress.jsonl`, and tolerate journal storage failures.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

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
    let sandbox = Sandbox::new(&fake_agent_config("sleep5"));
    let child = sandbox.spawn_with_env(&["run", "hello", "--agent", "fake", "--json"], &[]);

    let runs_dir = sandbox.repo_runs_dir();
    let start = Instant::now();
    let mut found_active: Option<Value> = None;

    while start.elapsed() < Duration::from_secs(20) {
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

    // 2. Exit 7 (awaiting_approval) records status "awaiting_approval" and end_summary.plan_hash
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-agent.sh");
    let plan_gate_config = format!(
        r#"[routing]
default_agent = "fake"

[graph]
allowed_runners = ["fake"]

[[agents.custom]]
name = "fake"
command = ["sh", "{fixture}", "plan", "{{prompt}}"]
prompt_via = "arg"
output = "text"
default_model = "composer-2.5"
timeout_secs = 60
"#,
        fixture = fixture.display().to_string().replace('\\', "/"),
    );
    let sandbox_gate = Sandbox::new(&plan_gate_config);
    let out_gate = sandbox_gate.run(&[
        "run",
        "ship the whole feature across several steps",
        "--json",
    ]);
    assert_eq!(out_gate.status.code(), Some(7));
    let json_gate = stdout_json(&out_gate);
    assert_eq!(json_gate["exit_code"], 7);
    let gate_run_id = json_gate["run_id"].as_str().expect("run_id present");

    let gate_manifest_path = sandbox_gate
        .repo_runs_dir()
        .join(gate_run_id)
        .join("manifest.json");
    let gate_manifest_content =
        fs::read_to_string(gate_manifest_path).expect("read awaiting_approval manifest");
    let gate_manifest: Value =
        serde_json::from_str(&gate_manifest_content).expect("parse awaiting_approval manifest");
    assert_eq!(gate_manifest["status"], "awaiting_approval");
    assert_eq!(gate_manifest["dmuxai_exit_code"], 7);
    if let Some(plan_hash) = json_gate["plan_hash"].as_str() {
        assert_eq!(
            gate_manifest["end_summary"]["plan_hash"].as_str(),
            Some(plan_hash)
        );
    }
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

// TC-2804 / FR-2804: headless run events carry run_id for the emitted seq range
#[cfg(unix)]
#[test]
fn tc_2804_headless_events_carry_run_id() {
    let sandbox = Sandbox::new(&fake_agent_config("jsonl"));
    let output = sandbox.run(&["run", "hi", "--agent", "fake", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let json = stdout_json(&output);
    let run_id = json["run_id"].as_str().expect("run_id in outcome");
    let first = json["events"]["first_seq"].as_u64();
    let last = json["events"]["last_seq"].as_u64();
    let (Some(first), Some(last)) = (first, last) else {
        panic!("expected event seq range in outcome: {json}");
    };
    assert!(last >= first);

    let events_path = sandbox.workspace.join(".dmuxai").join("events.jsonl");
    let content = fs::read_to_string(events_path).expect("read events log");
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line).expect("parse event line");
        let seq = record["seq"].as_u64().expect("seq");
        if (first..=last).contains(&seq) {
            assert_eq!(
                record["run_id"].as_str(),
                Some(run_id),
                "event seq {seq} must carry run_id"
            );
        }
    }
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

// TC-2815 / FR-2815: active manifest gets resolved_runner, resolved_model and links.graph_id before completion
#[cfg(unix)]
#[test]
fn tc_2815_active_manifest_has_graph_and_runner() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-agent.sh");
    let config = format!(
        r#"[routing]
default_agent = "fake"

[graph]
allowed_runners = ["fake"]

[[agents.custom]]
name = "fake"
command = ["sh", "{fixture}", "sleep5", "{{prompt}}"]
prompt_via = "arg"
output = "jsonl"
text_path = "/part/text"
text_when = {{ path = "/type", equals = "text" }}
error_path = "/error/message"
default_model = "test-model-1"
timeout_secs = 30
"#,
        fixture = fixture.display().to_string().replace('\\', "/"),
    );
    let sandbox = Sandbox::new(&config);
    let child = sandbox.spawn_with_env(&["run", "hello", "--json"], &[]);

    let runs_dir = sandbox.repo_runs_dir();
    let start = Instant::now();
    let mut found_active: Option<Value> = None;

    while start.elapsed() < Duration::from_secs(20) {
        if runs_dir.exists()
            && let Ok(entries) = fs::read_dir(&runs_dir)
        {
            for entry in entries.flatten() {
                let manifest_path = entry.path().join("manifest.json");
                if manifest_path.exists()
                    && let Ok(content) = fs::read_to_string(&manifest_path)
                    && let Ok(val) = serde_json::from_str::<Value>(&content)
                    && val["status"] == "active"
                    && val["resolved_runner"].is_string()
                    && val["links"]["graph_id"].is_string()
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

    let active_manifest = match found_active {
        Some(val) => val,
        None => {
            let output = child.wait_with_output().expect("wait for child");
            panic!(
                "expected active manifest with resolved_runner and graph_id; exit={:?}, stdout={}, stderr={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    };
    assert_eq!(active_manifest["status"], "active");
    assert_eq!(active_manifest["resolved_runner"], "fake");
    assert_eq!(active_manifest["resolved_model"], "test-model-1");
    let active_graph_id = active_manifest["links"]["graph_id"]
        .as_str()
        .expect("links.graph_id string");
    assert!(!active_graph_id.is_empty());
    let active_run_id = active_manifest["run_id"]
        .as_str()
        .expect("run_id string")
        .to_string();

    let output = child.wait_with_output().expect("wait for child");
    assert_eq!(output.status.code(), Some(0));
    let json = stdout_json(&output);
    assert_eq!(json["run_id"], active_run_id);

    let final_manifest_path = runs_dir.join(&active_run_id).join("manifest.json");
    let final_content = fs::read_to_string(final_manifest_path).expect("read final manifest");
    let final_manifest: Value = serde_json::from_str(&final_content).expect("parse final manifest");
    assert_eq!(final_manifest["status"], "completed");
    assert_eq!(final_manifest["resolved_runner"], "fake");
    assert_eq!(final_manifest["resolved_model"], "test-model-1");
    assert_eq!(final_manifest["links"]["graph_id"], active_graph_id);
}

// TC-2816 / FR-2816: `--approve-plan-id <hash>` supersedes earlier
// `awaiting_approval` manifests with the same plan hash once the approved plan
// starts executing; foreign hashes and the approving run itself stay untouched.
#[cfg(unix)]
#[test]
fn tc_2816_approval_supersedes_awaiting_run() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-agent.sh");
    let config = format!(
        r#"[routing]
default_agent = "fake"

[graph]
allowed_runners = ["fake"]

[[agents.custom]]
name = "fake"
command = ["sh", "{fixture}", "plan", "{{prompt}}"]
prompt_via = "arg"
output = "text"
default_model = "composer-2.5"
timeout_secs = 60
"#,
        fixture = fixture.display().to_string().replace('\\', "/"),
    );
    let prompt = "ship the whole feature across several steps";
    let sandbox = Sandbox::new(&config);

    // 1. First run exits 7 with a pending plan hash H and an awaiting manifest.
    let first = sandbox.run(&["run", prompt, "--json"]);
    assert_eq!(
        first.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json = stdout_json(&first);
    let plan_hash = first_json["plan_hash"]
        .as_str()
        .expect("plan hash on exit 7")
        .to_string();
    let first_run_id = first_json["run_id"]
        .as_str()
        .expect("first run_id")
        .to_string();

    let runs_dir = sandbox.repo_runs_dir();
    let first_manifest_value: Value = serde_json::from_str(
        &fs::read_to_string(runs_dir.join(&first_run_id).join("manifest.json"))
            .expect("read first manifest"),
    )
    .expect("parse first manifest");
    assert_eq!(first_manifest_value["status"], "awaiting_approval");
    assert_eq!(
        first_manifest_value["end_summary"]["plan_hash"],
        plan_hash.as_str()
    );

    // 2. A second awaiting manifest with the same hash H (duplicated run dir).
    let dup_run_id = "bbbbbbbbbbbbbbbb";
    let mut dup_manifest = first_manifest_value.clone();
    dup_manifest["run_id"] = json!(dup_run_id);
    let dup_dir = runs_dir.join(dup_run_id);
    fs::create_dir_all(&dup_dir).expect("dup run dir");
    fs::write(
        dup_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&dup_manifest).expect("serialize dup manifest"),
    )
    .expect("write dup manifest");

    // 3. An awaiting manifest with a different plan hash must stay untouched.
    let other_run_id = "cccccccccccccccc";
    let mut other_manifest = first_manifest_value.clone();
    other_manifest["run_id"] = json!(other_run_id);
    other_manifest["end_summary"]["plan_hash"] = json!("9999999999999999");
    let other_dir = runs_dir.join(other_run_id);
    fs::create_dir_all(&other_dir).expect("other run dir");
    fs::write(
        other_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&other_manifest).expect("serialize other manifest"),
    )
    .expect("write other manifest");

    // 4. The approving run executes the saved plan and completes.
    let second = sandbox.run(&[
        "run",
        prompt,
        "--graph",
        "--json",
        "--approve-plan-id",
        &plan_hash,
    ]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let second_json = stdout_json(&second);
    let second_run_id = second_json["run_id"]
        .as_str()
        .expect("second run_id")
        .to_string();
    assert_ne!(second_run_id, first_run_id, "approving run is a new run id");

    // 5. Both hash-H manifests became approved, keeping plan_hash, recording
    //    approved_by_run_id = the new run id.
    for run_id in [&first_run_id, dup_run_id] {
        let content = fs::read_to_string(runs_dir.join(run_id).join("manifest.json"))
            .unwrap_or_else(|e| panic!("read manifest of {run_id}: {e}"));
        let manifest: Value = serde_json::from_str(&content).expect("parse superseded manifest");
        assert_eq!(
            manifest["status"], "approved",
            "run {run_id} must be approved"
        );
        assert_eq!(manifest["end_summary"]["plan_hash"], plan_hash.as_str());
        assert_eq!(
            manifest["end_summary"]["approved_by_run_id"],
            second_run_id.as_str(),
            "run {run_id} must record the approving run id"
        );
    }

    // 6. The foreign-hash manifest is still awaiting_approval without a record.
    let other_content = fs::read_to_string(runs_dir.join(other_run_id).join("manifest.json"))
        .expect("read other manifest");
    let other_final: Value = serde_json::from_str(&other_content).expect("parse other manifest");
    assert_eq!(other_final["status"], "awaiting_approval");
    assert_eq!(other_final["end_summary"]["plan_hash"], "9999999999999999");
    assert!(
        other_final["end_summary"]
            .get("approved_by_run_id")
            .is_none(),
        "foreign-hash manifest must not gain approved_by_run_id"
    );

    // 7. The approving run's own manifest is completed, not approved.
    let second_content = fs::read_to_string(runs_dir.join(&second_run_id).join("manifest.json"))
        .expect("read approving run manifest");
    let second_manifest: Value =
        serde_json::from_str(&second_content).expect("parse approving run manifest");
    assert_eq!(second_manifest["status"], "completed");
    assert!(
        second_manifest["end_summary"]
            .as_object()
            .is_none_or(|summary| summary.get("approved_by_run_id").is_none()),
        "the approving run must not mark itself approved"
    );
}

// TC-2816 / FR-2816: an approving run that never executes the plan (missing or
// malformed `--approve-plan-id`) must not supersede the earlier awaiting run.
#[cfg(unix)]
#[test]
fn tc_2816_bad_or_missing_hash_does_not_supersede_awaiting() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-agent.sh");
    let config = format!(
        r#"[routing]
default_agent = "fake"

[graph]
allowed_runners = ["fake"]

[[agents.custom]]
name = "fake"
command = ["sh", "{fixture}", "plan", "{{prompt}}"]
prompt_via = "arg"
output = "text"
default_model = "composer-2.5"
timeout_secs = 60
"#,
        fixture = fixture.display().to_string().replace('\\', "/"),
    );
    let prompt = "ship the whole feature across several steps";
    let sandbox = Sandbox::new(&config);

    // 1. A first run leaves an awaiting manifest with plan hash H.
    let first = sandbox.run(&["run", prompt, "--json"]);
    assert_eq!(
        first.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json = stdout_json(&first);
    let plan_hash = first_json["plan_hash"]
        .as_str()
        .expect("plan hash on exit 7")
        .to_string();
    let first_run_id = first_json["run_id"]
        .as_str()
        .expect("first run_id")
        .to_string();

    let runs_dir = sandbox.repo_runs_dir();
    let manifest_path = runs_dir.join(&first_run_id).join("manifest.json");
    let before: Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).expect("read first manifest"))
            .expect("parse first manifest");
    assert_eq!(before["status"], "awaiting_approval");

    // 2. A well-formed but missing hash fails before the plan executes.
    let missing = sandbox.run(&[
        "run",
        prompt,
        "--graph",
        "--json",
        "--approve-plan-id",
        "1234567890abcdef",
    ]);
    assert_eq!(
        missing.status.code(),
        Some(3),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&missing.stdout),
        String::from_utf8_lossy(&missing.stderr)
    );

    // 3. A malformed hash is a usage error and never starts a run either.
    let malformed = sandbox.run(&[
        "run",
        prompt,
        "--graph",
        "--json",
        "--approve-plan-id",
        "NOT-A-PLAN-HASH",
    ]);
    assert_eq!(
        malformed.status.code(),
        Some(2),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&malformed.stdout),
        String::from_utf8_lossy(&malformed.stderr)
    );

    // 4. The awaiting manifest is byte-for-byte unchanged.
    let after: Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path).expect("read manifest after failed approvals"),
    )
    .expect("parse manifest after failed approvals");
    assert_eq!(
        after, before,
        "failed approvals must not touch the manifest"
    );
    assert_eq!(after["status"], "awaiting_approval");
    assert_eq!(after["end_summary"]["plan_hash"], plan_hash.as_str());
    assert!(
        after["end_summary"]
            .as_object()
            .is_none_or(|summary| summary.get("approved_by_run_id").is_none()),
        "the awaiting run must not record an approver"
    );
}
