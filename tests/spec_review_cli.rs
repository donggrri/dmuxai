//! `dmuxai spec-review` CLI tests (SPEC-spec-review-gate FR-3708, FR-3709 / TC-3709).
//!
//! Deciding needs stdin and stdout on a TTY, so those cases run the binary
//! under a pseudo terminal; read-only commands and refusals use plain pipes.

#![cfg(unix)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use dmuxai::graph::spec_review::{ReviewStatus, SpecReviewRecord, SpecReviewStore, Verdict};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::Value;

const SPEC_PATH: &str = "docs/specs/SPEC-x.md";

struct Env {
    _dir: tempfile::TempDir,
    state: PathBuf,
    workspace: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("state");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(workspace.join("docs/specs")).unwrap();
        std::fs::write(workspace.join(SPEC_PATH), "# Spec X\n\nbody\n").unwrap();
        std::fs::create_dir_all(&state).unwrap();
        Self {
            _dir: dir,
            state,
            workspace,
        }
    }

    fn store(&self) -> SpecReviewStore {
        SpecReviewStore::new(&self.state)
    }

    fn create(&self, id: &str, status: ReviewStatus) {
        let record = SpecReviewRecord {
            review_id: id.to_owned(),
            graph_id: id.to_owned(),
            run_id: String::new(),
            workspace: self.workspace.display().to_string(),
            prompt_summary: "implement x".to_owned(),
            spec_paths: vec![SPEC_PATH.to_owned()],
            spec_digest: "d".repeat(64),
            round: 1,
            max_rounds: 5,
            status,
            created_ms: dmuxai::graph::spec_review::unix_now_ms(),
            updated_ms: 0,
            board_item_id: None,
            verdict: None,
            by: None,
            feedback: None,
            decided_ms: None,
        };
        self.store().create(&record).expect("create record");
    }

    /// Runs the CLI with pipes (stdin/stdout are not a TTY).
    fn run(&self, args: &[&str], run_id: Option<&str>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
        cmd.args(args)
            .current_dir(&self.workspace)
            .env("DMUXAI_STATE_DIR", &self.state)
            .env_remove("DMUXAI_RUN_ID")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(run_id) = run_id {
            cmd.env("DMUXAI_RUN_ID", run_id);
        }
        cmd.output().expect("run dmuxai")
    }

    /// Runs the CLI on a pseudo terminal; returns the exit code and output.
    fn run_tty(&self, args: &[&str], run_id: Option<&str>) -> (i32, String) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_dmuxai"));
        cmd.args(args);
        cmd.cwd(&self.workspace);
        cmd.env("DMUXAI_STATE_DIR", &self.state);
        cmd.env_remove("DMUXAI_RUN_ID");
        if let Some(run_id) = run_id {
            cmd.env("DMUXAI_RUN_ID", run_id);
        }
        let mut child = pair.slave.spawn_command(cmd).expect("spawn on pty");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("pty reader");
        let output = std::thread::spawn(move || {
            let mut text = String::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                text.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            text
        });
        let status = child.wait().expect("wait child");
        drop(pair.master);
        let text = output.join().unwrap_or_default();
        (status.exit_code() as i32, text)
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

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn state_has_no_records(path: &Path) -> bool {
    !path.join("spec-reviews").exists()
}

// TC-3709 / FR-3709: `list` and `show` print the documented fields as JSON.
#[test]
fn tc_3709_list_and_show_json_fields() {
    let env = Env::new();
    env.create("run-aaa111", ReviewStatus::Pending);
    env.create("run-bbb222", ReviewStatus::Approved);

    let list = env.run(&["spec-review", "list", "--json"], None);
    assert_eq!(list.status.code(), Some(0), "{}", stderr(&list));
    let json = stdout_json(&list);
    let reviews = json["reviews"].as_array().expect("reviews array");
    assert_eq!(reviews.len(), 1, "only pending reviews by default");
    let first = &reviews[0];
    assert_eq!(first["review_id"], "run-aaa111");
    assert_eq!(first["round"], 1);
    assert_eq!(first["status"], "pending");
    assert_eq!(first["spec_paths"][0], SPEC_PATH);
    assert!(first["workspace"].is_string());
    assert!(first["age_secs"].is_u64());

    let all = env.run(&["spec-review", "list", "--all", "--json"], None);
    assert_eq!(stdout_json(&all)["reviews"].as_array().unwrap().len(), 2);

    let show = env.run(&["spec-review", "show", "run-aaa111", "--json"], None);
    assert_eq!(show.status.code(), Some(0), "{}", stderr(&show));
    let json = stdout_json(&show);
    assert_eq!(json["review_id"], "run-aaa111");
    assert_eq!(json["spec_files"][0]["path"], SPEC_PATH);
    assert_eq!(json["spec_files"][0]["content"], "# Spec X\n\nbody\n");
    assert_eq!(json["spec_files"][0]["truncated"], false);

    let text = env.run(&["spec-review", "show", "run-aaa111"], None);
    let text = String::from_utf8_lossy(&text.stdout).into_owned();
    assert!(
        text.contains("status: pending") && text.contains("# Spec X"),
        "{text}"
    );
}

// TC-3709 / FR-3709: an invalid id is a usage error (2) and never touches the
// filesystem; an unknown id is not found (3).
#[test]
fn tc_3709_invalid_and_unknown_id_exit_codes() {
    let env = Env::new();
    for bad in ["../etc/passwd", "a/b", "", "UPPER CASE", ".hidden"] {
        let output = env.run(&["spec-review", "show", bad], None);
        assert_eq!(
            output.status.code(),
            Some(2),
            "show {bad:?}: {}",
            stderr(&output)
        );
    }
    assert!(
        state_has_no_records(&env.state),
        "an invalid id must not create the store"
    );
    let unknown = env.run(&["spec-review", "show", "run-missing1"], None);
    assert_eq!(unknown.status.code(), Some(3), "{}", stderr(&unknown));
    let unknown_json = env.run(&["spec-review", "show", "run-missing1", "--json"], None);
    assert_eq!(unknown_json.status.code(), Some(3));
    let usage = env.run(&["spec-review", "request-changes", "run-aaa111"], None);
    assert_eq!(usage.status.code(), Some(2), "request-changes needs --text");
}

// TC-3708 / TC-3709: deciding without a TTY is refused (5) and leaves the record alone.
#[test]
fn tc_3709_decide_without_tty_is_forbidden() {
    let env = Env::new();
    env.create("run-aaa111", ReviewStatus::Pending);
    let output = env.run(&["spec-review", "approve", "run-aaa111"], None);
    assert_eq!(output.status.code(), Some(5), "{}", stderr(&output));
    assert!(
        stderr(&output)
            .contains("spec review decisions need an interactive terminal or the dashboard"),
        "{}",
        stderr(&output)
    );
    let record = env.store().load("run-aaa111").unwrap();
    assert_eq!(record.status, ReviewStatus::Pending);
    assert!(record.verdict.is_none());
}

// TC-3709 / FR-3708: a human on a TTY decides; a second decision conflicts (4);
// DMUXAI_RUN_ID makes even a TTY decision forbidden (5).
#[test]
fn tc_3709_decide_on_tty_conflict_and_run_id() {
    let env = Env::new();
    env.create("run-aaa111", ReviewStatus::Pending);
    env.create("run-bbb222", ReviewStatus::Pending);

    let (code, out) = env.run_tty(
        &["spec-review", "approve", "run-aaa111"],
        Some("run-agent-1"),
    );
    assert_eq!(code, 5, "DMUXAI_RUN_ID set: {out}");
    assert_eq!(
        env.store().load("run-aaa111").unwrap().status,
        ReviewStatus::Pending
    );

    let (code, out) = env.run_tty(&["spec-review", "approve", "run-aaa111"], None);
    assert_eq!(code, 0, "{out}");
    let record = env.store().load("run-aaa111").unwrap();
    assert_eq!(record.status, ReviewStatus::Approved);
    assert_eq!(record.verdict, Some(Verdict::Approve));
    assert_eq!(record.by.as_deref(), Some("owner"));

    let (code, out) = env.run_tty(&["spec-review", "reject", "run-aaa111"], None);
    assert_eq!(code, 4, "already decided: {out}");

    let (code, _) = env.run_tty(
        &[
            "spec-review",
            "request-changes",
            "run-bbb222",
            "--text",
            "add error cases",
        ],
        None,
    );
    assert_eq!(code, 0);
    let record = env.store().load("run-bbb222").unwrap();
    assert_eq!(record.status, ReviewStatus::ChangesRequested);
    assert_eq!(record.feedback.as_deref(), Some("add error cases"));

    let (code, out) = env.run_tty(&["spec-review", "approve", "run-missing1"], None);
    assert_eq!(code, 3, "{out}");
    let (code, out) = env.run_tty(&["spec-review", "approve", "../x"], None);
    assert_eq!(code, 2, "{out}");
}
