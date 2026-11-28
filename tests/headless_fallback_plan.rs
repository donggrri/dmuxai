//! Headless fallback-plan and plan-only tests (SPEC-board-intake FR-3860,
//! NFR-3860, FR-3866 / TC-3860, TC-3866; GitHub issue #375).
//!
//! Fake runner binaries are shell scripts written into the sandbox: they
//! answer the judge / planner prompts and record every other prompt (a step
//! execution) to a log outside the workspace.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const PLAN_JSON: &str = r#"{"tasks":[{"id":"task-1","title":"Implement","done_check":"change applied","areas":[],"size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"implement","job":"Apply the requested change","runner":"fake","depends_on":[],"task":"task-1"}]}"#;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
    exec_log: PathBuf,
}

impl Sandbox {
    /// `planner` is the shell `case` body run for judge / planner prompts.
    fn new(judge: &str, planner: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let workspace = root.join("ws");
        for sub in ["ws", "data", "state", "cfg", "home"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        let exec_log = root.join("executed.log");
        let script = root.join("fake-runner.sh");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nfor last; do :; done\ncase \"$last\" in\n  *'DmuxAI triage judge'*) {judge} ;;\n  *'DmuxAI planner'*) {planner} ;;\n  *) printf '%s\\n---\\n' \"$last\" >> '{log}'; echo done ;;\nesac\n",
                log = exec_log.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let config = format!(
            r#"{spec_off}[routing]
default_agent = "fake"

[graph]
allowed_runners = ["fake"]

[[agents.custom]]
name = "fake"
command = ["{script}", "{{prompt}}"]
prompt_via = "arg"
output = "text"
default_model = "composer-2.5"
timeout_secs = 60
"#,
            spec_off = dmuxai::settings::WORKFLOW_SPEC_FIRST_OFF_TOML,
            script = script.display(),
        );
        fs::write(workspace.join("dmuxai.toml"), config).unwrap();
        Self {
            _dir: dir,
            root,
            workspace,
            exec_log,
        }
    }

    fn run(&self, args: &[&str], stdin: &str) -> Output {
        use std::io::Write as _;
        let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .args(args)
            .current_dir(&self.workspace)
            .env("DMUXAI_WORKSPACE", &self.workspace)
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("HOME", self.root.join("home"))
            .env("DMUXAI_TRUST_WORKSPACE_CONFIG", "1")
            .env_remove("DMUXAI_STATE_DIR")
            .env_remove("DMUXAI_WEB_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn dmuxai");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().expect("wait dmuxai")
    }

    fn executed(&self) -> String {
        fs::read_to_string(&self.exec_log).unwrap_or_default()
    }

    /// Workspace files outside `.dmuxai` (a plan-only run must not add any).
    fn workspace_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut stack = vec![self.workspace.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.file_name().is_some_and(|n| n == ".dmuxai") {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    fn pending_file(&self, hash: &str) -> Option<PathBuf> {
        let name = format!("{hash}.json");
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().is_some_and(|n| n == name.as_str())
                    && path
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|n| n == "graph-pending")
                {
                    return Some(path);
                }
            }
        }
        None
    }
}

fn stdout_json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\nstdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn describe(output: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

const FAIL_FAST: &str = "echo 'planner boom sk-ant-secret1234567890abcdef' >&2; exit 3";
/// Several heuristic roles (research, implement, test) so a failed judge still
/// routes complex and parks a fallback plan.
const COMPLEX_PROMPT: &str = "research the intake flow, implement it and test it";

// TC-3860 / FR-3860, NFR-3860 (#375): when the judge and planner fail fast,
// headless `run --graph` still parks the fallback plan in
// `graph-pending/<hash>.json` with `plan_hash`, `plan_source: "fallback"` and a
// masked `planner_error`, and `--approve-plan-id <hash>` runs it. A planner
// plan reports `plan_source: "planner"` without `planner_error`.
#[test]
fn tc_3860_fallback_plan_has_hash() {
    let sandbox = Sandbox::new(FAIL_FAST, FAIL_FAST);
    let parked = sandbox.run(&["run", "--graph", "--json", "-"], COMPLEX_PROMPT);
    assert_eq!(parked.status.code(), Some(7), "{}", describe(&parked));
    let json = stdout_json(&parked);
    let hash = json["plan_hash"]
        .as_str()
        .unwrap_or_else(|| panic!("plan_hash missing: {json}"))
        .to_owned();
    assert_eq!(hash.len(), 16, "{hash}");
    assert_eq!(json["plan_source"], "fallback", "{json}");
    let planner_error = json["planner_error"]
        .as_str()
        .unwrap_or_else(|| panic!("planner_error missing: {json}"));
    assert!(!planner_error.is_empty());
    assert!(planner_error.chars().count() <= 500, "{planner_error}");
    assert!(
        !planner_error.contains("sk-ant-secret1234567890abcdef"),
        "{planner_error}"
    );
    let pending = sandbox
        .pending_file(&hash)
        .unwrap_or_else(|| panic!("graph-pending/{hash}.json not written"));
    let saved: Value = serde_json::from_slice(&fs::read(&pending).unwrap()).unwrap();
    assert!(
        saved["steps"].as_array().is_some_and(|s| !s.is_empty()),
        "{saved}"
    );
    assert!(sandbox.executed().is_empty(), "{}", sandbox.executed());

    let approved = sandbox.run(
        &["run", "--graph", "--json", "--approve-plan-id", &hash, "-"],
        COMPLEX_PROMPT,
    );
    assert_eq!(approved.status.code(), Some(0), "{}", describe(&approved));
    assert!(!sandbox.executed().is_empty(), "approved plan ran no step");

    let planner = Sandbox::new(
        "echo '{\"route\":\"complex\",\"reason\":\"multi-step\"}'",
        &format!("echo '{PLAN_JSON}'"),
    );
    let planned = planner.run(&["run", "--graph", "--json", "-"], COMPLEX_PROMPT);
    assert_eq!(planned.status.code(), Some(7), "{}", describe(&planned));
    let json = stdout_json(&planned);
    assert_eq!(json["plan_source"], "planner", "{json}");
    assert!(json.get("planner_error").is_none(), "{json}");
    assert!(
        planner
            .pending_file(json["plan_hash"].as_str().unwrap())
            .is_some()
    );
}

// TC-3866 / FR-3866: `run --graph --plan-only` always plans (the judge's
// simple route and the trivial-prompt shortcut are skipped), never executes a
// step, and ends with exit 7 + plan_hash for a planner or fallback plan;
// conflicting flags are usage errors (exit 2).
#[test]
fn tc_3866_plan_only_never_executes() {
    let sandbox = Sandbox::new(
        "echo '{\"route\":\"simple\",\"reason\":\"one-step\"}'",
        &format!("echo '{PLAN_JSON}'"),
    );
    let before = sandbox.workspace_files();
    for prompt in ["hi", COMPLEX_PROMPT] {
        let out = sandbox.run(&["run", "--graph", "--plan-only", "--json", "-"], prompt);
        assert_eq!(out.status.code(), Some(7), "{prompt}: {}", describe(&out));
        let json = stdout_json(&out);
        let hash = json["plan_hash"]
            .as_str()
            .unwrap_or_else(|| panic!("{prompt}: plan_hash missing: {json}"));
        assert_eq!(json["plan_source"], "planner", "{prompt}: {json}");
        assert!(sandbox.pending_file(hash).is_some(), "{prompt}");
        assert!(
            sandbox.executed().is_empty(),
            "{prompt}: {}",
            sandbox.executed()
        );
        assert_eq!(sandbox.workspace_files(), before, "{prompt}");
    }

    let fallback = Sandbox::new(FAIL_FAST, FAIL_FAST);
    let out = fallback.run(&["run", "--graph", "--plan-only", "--json", "-"], "hi");
    assert_eq!(out.status.code(), Some(7), "{}", describe(&out));
    let json = stdout_json(&out);
    assert_eq!(json["plan_source"], "fallback", "{json}");
    assert!(
        fallback
            .pending_file(json["plan_hash"].as_str().unwrap())
            .is_some()
    );
    assert!(fallback.executed().is_empty(), "{}", fallback.executed());

    for args in [
        &["run", "--graph", "--plan-only", "--approve-plan", "-"][..],
        &[
            "run",
            "--graph",
            "--plan-only",
            "--approve-plan-id",
            "0123456789abcdef",
            "-",
        ],
        &["run", "--graph", "--plan-only", "--agent", "fake", "-"],
        &["run", "--graph", "--plan-only", "--agents", "fake", "-"],
        &["run", "--plan-only", "-"],
    ] {
        let out = sandbox.run(args, "hi");
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", describe(&out));
    }
    assert!(sandbox.executed().is_empty(), "{}", sandbox.executed());
}
