//! Headless spec review gate (SPEC-spec-review-gate FR-3702..FR-3705, FR-3717
//! / TC-3717): a real `dmuxai run` pauses after the spec step, a human decision
//! written to the record lets it continue.

#![cfg(unix)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dmuxai::graph::spec_review::{
    DecideRequest, ReviewStatus, SpecReviewRecord, SpecReviewStore, Verdict,
};
use serde_json::Value;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
    state: PathBuf,
    log: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let workspace = root.join("ws");
        let state = root.join("state");
        for sub in ["ws", "state", "cfg", "home", "data"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            let status = Command::new("git")
                .args(&args)
                .current_dir(&workspace)
                .status()
                .expect("git");
            assert!(status.success());
        }
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("spec-agent.sh");
        let config = format!(
            r#"[routing]
default_agent = "fake"

[workflow]
spec_first = "full"
test_command = "true"

[graph]
allowed_runners = ["fake"]

[[agents.custom]]
name = "fake"
command = ["sh", "{fixture}", "{{prompt}}"]
prompt_via = "arg"
output = "jsonl"
text_path = "/part/text"
text_when = {{ path = "/type", equals = "text" }}
error_path = "/error/message"
timeout_secs = 60
"#,
            fixture = fixture.display().to_string().replace('\\', "/"),
        );
        fs::write(workspace.join("dmuxai.toml"), config).unwrap();
        Self {
            log: root.join("calls.log"),
            _dir: dir,
            root,
            workspace,
            state,
        }
    }

    fn spawn(&self, extra: &[&str], env: &[(&str, &str)]) -> Child {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
        cmd.args([
            "run",
            "implement x",
            "--agent",
            "fake",
            "--json",
            "--progress",
        ])
        .args(extra)
        .current_dir(&self.workspace)
        .env("DMUXAI_WORKSPACE", &self.workspace)
        .env("DMUXAI_STATE_DIR", &self.state)
        .env("XDG_DATA_HOME", self.root.join("data"))
        .env("XDG_CONFIG_HOME", self.root.join("cfg"))
        .env("HOME", self.root.join("home"))
        .env("DMUXAI_TRUST_WORKSPACE_CONFIG", "1")
        .env("DMUXAI_SPEC_REVIEW_POLL_MS", "50")
        .env("DMUXAI_SPEC_REVIEW_HEARTBEAT_MS", "600000")
        .env("SPEC_AGENT_LOG", &self.log)
        .env_remove("DMUXAI_RUN_ID")
        .env_remove("DMUXAI_WEB_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.spawn().expect("spawn dmuxai")
    }

    fn store(&self) -> SpecReviewStore {
        SpecReviewStore::new(&self.state)
    }

    fn calls(&self) -> usize {
        fs::read_to_string(&self.log)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    /// Waits for the single review record to reach `round` in `pending`.
    fn wait_pending(&self, round: u32, child: &mut Child) -> SpecReviewRecord {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(60) {
            if let Some(record) = self
                .store()
                .list(true)
                .into_iter()
                .find(|record| record.status == ReviewStatus::Pending && record.round == round)
            {
                return record;
            }
            if let Ok(Some(status)) = child.try_wait() {
                let mut err = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut err);
                }
                panic!("run ended ({status}) before review round {round}\n{err}");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("no pending review round {round} within 60s");
    }

    fn decide(&self, id: &str, verdict: Verdict, text: Option<&str>) {
        self.store()
            .decide(
                id,
                &DecideRequest {
                    verdict,
                    text,
                    spec_digest: None,
                    by: "owner",
                },
            )
            .expect("decide");
    }
}

fn finish(child: Child) -> (Option<i32>, Value, String) {
    let output = child.wait_with_output().expect("wait for dmuxai");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let json = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"));
    (
        output.status.code(),
        json,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn hint_lines(stderr: &str) -> usize {
    stderr
        .lines()
        .filter(|line| line.contains("spec review: waiting for a human"))
        .count()
}

// TC-3717 / FR-3717 (also FR-3702, FR-3704): the run pauses after the spec step,
// starts no implementation step while pending, continues after an approval and
// reports `spec_review` in the JSON output; the hint is printed once.
#[test]
fn tc_3717_run_pauses_for_review_then_approves() {
    let sandbox = Sandbox::new();
    let mut child = sandbox.spawn(&["--approve-plan"], &[]);
    let record = sandbox.wait_pending(1, &mut child);
    assert_eq!(record.spec_paths, vec!["docs/specs/SPEC-x.md".to_owned()]);
    // Only the spec step has run; the implementation is held.
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(sandbox.calls(), 1, "no implementation step while pending");
    assert!(child.try_wait().unwrap().is_none(), "run still waiting");
    sandbox.decide(&record.review_id, Verdict::Approve, None);

    let (code, json, stderr) = finish(child);
    assert_eq!(code, Some(0), "{json}\n{stderr}");
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["spec_review"]["review_id"], record.review_id);
    assert_eq!(json["spec_review"]["status"], "approved");
    assert_eq!(json["spec_review"]["round"], 1);
    assert_eq!(hint_lines(&stderr), 1, "hint exactly once:\n{stderr}");
    assert!(sandbox.calls() >= 3, "implementation ran after approval");
}

// TC-3705 / FR-3705: Request changes re-runs the spec step with the feedback;
// the next review is round 2, which is then approved.
#[test]
fn tc_3705_request_changes_reruns_spec_step() {
    let sandbox = Sandbox::new();
    let mut child = sandbox.spawn(&["--approve-plan"], &[]);
    let first = sandbox.wait_pending(1, &mut child);
    sandbox.decide(
        &first.review_id,
        Verdict::RequestChanges,
        Some("cover the error cases"),
    );
    let second = sandbox.wait_pending(2, &mut child);
    assert_eq!(second.review_id, first.review_id);
    assert_ne!(
        second.spec_digest, first.spec_digest,
        "spec changed in round 2"
    );
    let spec = fs::read_to_string(sandbox.workspace.join("docs/specs/SPEC-x.md")).unwrap();
    assert!(spec.contains("cover the error cases"), "{spec}");
    sandbox.decide(&second.review_id, Verdict::Approve, None);

    let (code, json, stderr) = finish(child);
    assert_eq!(code, Some(0), "{json}\n{stderr}");
    assert_eq!(json["spec_review"]["status"], "approved");
    assert_eq!(json["spec_review"]["round"], 2);
}

// TC-3704 / FR-3704: a rejected spec ends the run with exit 1 and no implementation.
#[test]
fn tc_3704_rejected_review_fails_run_without_implementation() {
    let sandbox = Sandbox::new();
    let mut child = sandbox.spawn(&["--approve-plan"], &[]);
    let record = sandbox.wait_pending(1, &mut child);
    sandbox.decide(&record.review_id, Verdict::Reject, Some("wrong direction"));
    let (code, json, _stderr) = finish(child);
    assert_eq!(code, Some(1), "{json}");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], "spec rejected by reviewer", "{json}");
    assert_eq!(json["spec_review"]["status"], "rejected");
    assert_eq!(sandbox.calls(), 1, "no implementation step ran");
}

// TC-3706 / FR-3706 + TC-3701 / FR-3701: the timeout expires a pending review;
// `--no-spec-review` skips the review entirely.
#[test]
fn tc_3706_timeout_expires_and_tc_3701_no_spec_review_skips() {
    let sandbox = Sandbox::new();
    let child = sandbox.spawn(
        &["--approve-plan"],
        &[("DMUXAI_SPEC_REVIEW_TIMEOUT_MS", "600")],
    );
    let (code, json, _stderr) = finish(child);
    assert_eq!(code, Some(1), "{json}");
    let error = json["error"].as_str().unwrap_or_default();
    assert!(error.starts_with("spec review timed out after"), "{json}");
    assert_eq!(json["spec_review"]["status"], "expired");
    assert_eq!(sandbox.calls(), 1);

    let skip = Sandbox::new();
    let child = skip.spawn(&["--approve-plan", "--no-spec-review"], &[]);
    let (code, json, stderr) = finish(child);
    assert_eq!(code, Some(0), "{json}\n{stderr}");
    assert!(
        json.get("spec_review").is_none(),
        "no review requested: {json}"
    );
    assert!(skip.store().list(true).is_empty());
    assert!(skip.calls() >= 3);
}

// TC-3704 / FR-3704: Ctrl-C while the run waits ends it (exit 130) and sets the
// record to `cancelled`.
#[test]
fn tc_3704_interrupt_while_waiting_cancels_record() {
    let sandbox = Sandbox::new();
    let mut child = sandbox.spawn(&["--approve-plan"], &[]);
    let record = sandbox.wait_pending(1, &mut child);
    // SAFETY: plain signal delivery to the child process we spawned.
    let rc = unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
    assert_eq!(rc, 0);
    let (code, json, _stderr) = finish(child);
    assert_eq!(code, Some(130), "{json}");
    assert_eq!(json["ok"], false);
    let stored = sandbox.store().load(&record.review_id).unwrap();
    assert_eq!(stored.status, ReviewStatus::Cancelled);
    assert_eq!(json["spec_review"]["status"], "cancelled");
    assert_eq!(sandbox.calls(), 1);
}
