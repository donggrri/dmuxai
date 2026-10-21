//! Dashboard chat sends a prompt once and shows the user's message once
//! (double-message bug after #80/#81): the message appears right after
//! Enter (optimistic echo), a second Enter or Send click while the first
//! request is in flight does not post again, and the echo is replaced by the
//! prompt's own server copy (never duplicated, never dropped while the server
//! still shows the prompt, and dropped when the server refuses it later).
//! Needs Chrome/Chromium and python Playwright, so it is `#[ignore]`d; run
//! with `cargo test --test web_chat_single_send -- --ignored`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use dmuxai::web::commands::{DashboardCommand, PromptAccepted};
use dmuxai::web::{
    DashboardHub, DashboardServices, ServerConfig, SessionAgent, SessionMessage, SessionView,
    SnapshotSource, start_server,
};

const PROMPT: &str = "Explain the README";
const SECOND_PROMPT: &str = "And the license";

fn find_chrome() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DMUXAI_CHROME") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome.exe",
        "msedge.exe",
    ];
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn session(transcript: Vec<SessionMessage>, busy: bool, prompt_seq: u64) -> SessionView {
    SessionView {
        conversation_id: "conv-1".to_owned(),
        conversation_title: "Sample".to_owned(),
        selected_agent: "cursor".to_owned(),
        agents: vec![SessionAgent {
            id: "cursor".to_owned(),
            label: "Cursor".to_owned(),
            mode: "agent".to_owned(),
            modes: vec!["agent".to_owned()],
            busy,
            allowed: true,
            ..Default::default()
        }],
        transcript,
        prompt_seq,
        ..Default::default()
    }
}

fn message(index: usize, role: &str, content: &str, streaming: bool) -> SessionMessage {
    SessionMessage {
        id: format!("conv-1:{index}"),
        role: role.to_owned(),
        agent: (role == "agent").then(|| "cursor".to_owned()),
        content: content.to_owned(),
        content_html: None,
        streaming,
        images: Vec::new(),
        prompt_id: None,
        queued: false,
    }
}

/// The server's stand-in for a prompt that has no user message yet.
fn pending_prompt(id: u64, content: &str) -> SessionMessage {
    SessionMessage {
        id: format!("conv-1:prompt-{id}"),
        prompt_id: Some(id),
        ..message(0, "user", content, false)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scenario {
    /// Like the triage judge: the step posts the user message 1.5 s later.
    PostedLater,
    /// A plan waiting for approval: the prompt never gets its own message.
    PlanWaits,
    /// Accepted (202), then refused by the runner.
    RefusedLater,
    /// Two quick sends, each posted later.
    TwoSends,
}

struct Outcome {
    v: serde_json::Value,
    submits: usize,
}

async fn run_scenario(scenario: Scenario) -> Option<Outcome> {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome/Chromium found (set DMUXAI_CHROME)");
        return None;
    };
    let python = if cfg!(windows) { "python" } else { "python3" };
    let probe = Command::new(python)
        .args(["-c", "import playwright"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.map(|status| status.success()).unwrap_or(false) {
        println!("SKIP: python playwright not installed");
        return None;
    }

    let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_owned());
    hub.publish(Vec::new(), None);
    hub.set_session(Some(session(
        vec![message(0, "system", "ready", false)],
        false,
        0,
    )));
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(8);
    let config = ServerConfig {
        bind: "127.0.0.1".parse().unwrap(),
        port: 0,
        allow_remote: false,
        token: "test-token-single-send".to_owned(),
        services: DashboardServices {
            events_path: None,
            todos: None,
            commands: Some(command_tx),
            mcp: None,
            web_attached: true,
            memory: None,
            docs: None,
        },
    };
    let server = start_server(config, hub.clone())
        .await
        .expect("start server");
    let url = server.url.clone();

    let submits = Arc::new(AtomicUsize::new(0));
    let submits_seen = submits.clone();
    let seq = Arc::new(AtomicU64::new(0));
    let fake_hub = hub.clone();
    tokio::spawn(async move {
        let accepted: Arc<std::sync::Mutex<Vec<(u64, String)>>> = Arc::default();
        while let Some(command) = command_rx.recv().await {
            let DashboardCommand::SubmitPrompt { text, reply, .. } = command else {
                continue;
            };
            submits_seen.fetch_add(1, Ordering::SeqCst);
            let id = seq.fetch_add(1, Ordering::SeqCst) + 1;
            accepted.lock().unwrap().push((id, text.clone()));
            // Like the app: the record exists before the reply goes out.
            let ready = message(0, "system", "ready", false);
            let pending: Vec<SessionMessage> = accepted
                .lock()
                .unwrap()
                .iter()
                .map(|(id, text)| pending_prompt(*id, text))
                .collect();
            let mut transcript = vec![ready.clone()];
            transcript.extend(pending.clone());
            fake_hub.set_session(Some(session(transcript, true, id)));
            let _ = reply.send(Ok(PromptAccepted {
                conversation_id: "conv-1".to_owned(),
                agent: "cursor".to_owned(),
                prompt_id: Some(id),
                message_id: None,
                queued: false,
            }));
            let hub = fake_hub.clone();
            let accepted = accepted.clone();
            tokio::spawn(async move {
                match scenario {
                    Scenario::PostedLater | Scenario::TwoSends => {
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                        // The step posts this prompt; others stay pending.
                        let all = accepted.lock().unwrap().clone();
                        let mut transcript = vec![ready.clone()];
                        let mut index = 1;
                        for (other, other_text) in &all {
                            if *other <= id {
                                transcript.push(message(index, "user", other_text, false));
                                index += 1;
                            }
                        }
                        for (other, other_text) in &all {
                            if *other > id {
                                transcript.push(pending_prompt(*other, other_text));
                            }
                        }
                        let seq_now = all.last().map(|(id, _)| *id).unwrap_or(id);
                        hub.set_session(Some(session(transcript.clone(), true, seq_now)));
                        tokio::time::sleep(Duration::from_millis(800)).await;
                        let mut done = transcript;
                        done.push(message(
                            index,
                            "agent",
                            "The README describes the project.",
                            false,
                        ));
                        hub.set_session(Some(session(done, false, seq_now)));
                    }
                    Scenario::PlanWaits => {
                        // Nothing more: the prompt waits for plan approval.
                    }
                    Scenario::RefusedLater => {
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                        let refused = message(
                            1,
                            "system",
                            "Cursor error: Cursor is already running a turn",
                            false,
                        );
                        hub.set_session(Some(session(vec![ready, refused], false, id)));
                    }
                }
            });
        }
    });

    let script = r##"
import sys, json, time
from playwright.sync_api import sync_playwright

TEXT, SECOND, MODE = sys.argv[3], sys.argv[4], sys.argv[5]
out = {"posts": 0, "samples": []}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1280, "height": 800}, color_scheme="light")
    pg.on("request", lambda r: out.__setitem__("posts", out["posts"] + 1)
          if r.method == "POST" and r.url.endswith("/api/prompt") else None)
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#session:not([hidden]) .session-input:not([disabled])", timeout=15000)
    pg.fill(".session-input", TEXT)
    pg.press(".session-input", "Enter")
    # A second Enter and a Send click while the first send is in flight.
    pg.press(".session-input", "Enter")
    pg.eval_on_selector("[data-parity=\"prompt-send\"]", "e => { e.disabled = false; e.click(); }")
    start = time.time()
    second_sent = False
    while time.time() - start < 5.0:
        if MODE == "two" and not second_sent and time.time() - start > 0.3:
            pg.fill(".session-input", SECOND)
            pg.press(".session-input", "Enter")
            second_sent = True
            out["second_at"] = round(time.time() - start, 2)
        out["samples"].append(pg.evaluate("""([t, t2]) => {
            const count = s => Array.from(document.querySelectorAll('#session li.msg-user'))
                .filter(li => (li.querySelector('.msg-text') || li).textContent.trim() === s).length;
            return {
              at: 0,
              users: count(t),
              users2: count(t2),
              pending: document.querySelectorAll('#session li.msg-pending').length,
              agent: Array.from(document.querySelectorAll('#session li.msg-agent .msg-text'))
                  .map(n => n.textContent.trim()).join('|'),
              system: Array.from(document.querySelectorAll('#session li.msg-system'))
                  .map(n => n.textContent.trim()).join('|'),
              input: document.querySelector('.session-input').value
            };
        }""", [TEXT, SECOND]))
        out["samples"][-1]["at"] = round(time.time() - start, 2)
        pg.wait_for_timeout(100)
    print(json.dumps(out))
    b.close()
"##;

    let mode = if scenario == Scenario::TwoSends {
        "two"
    } else {
        "one"
    };
    let output = tokio::task::spawn_blocking(move || {
        Command::new(python)
            .args(["-c", script])
            .arg(&chrome)
            .arg(&url)
            .arg(PROMPT)
            .arg(SECOND_PROMPT)
            .arg(mode)
            .stderr(Stdio::inherit())
            .output()
            .expect("run playwright")
    })
    .await
    .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let json_line = text
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON result from playwright:\n{text}"));
    let v: serde_json::Value = serde_json::from_str(json_line).expect("json result");
    assert!(!v["samples"].as_array().expect("samples").is_empty(), "{v}");
    Some(Outcome {
        v,
        submits: submits.load(Ordering::SeqCst),
    })
}

fn count(sample: &serde_json::Value, key: &str) -> u64 {
    sample[key].as_u64().unwrap_or(0)
}

fn assert_sent_once_and_shown_at_once(outcome: &Outcome) {
    let v = &outcome.v;
    let first = &v["samples"][0];
    assert_eq!(count(first, "users"), 1, "message shown at once: {v}");
    assert_eq!(
        first["input"].as_str(),
        Some(""),
        "input cleared at once: {v}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
async fn prompt_is_sent_and_shown_once_in_headless_chrome() {
    let Some(outcome) = run_scenario(Scenario::PostedLater).await else {
        return;
    };
    let v = &outcome.v;
    assert_eq!(v["posts"].as_u64(), Some(1), "exactly one POST: {v}");
    assert_eq!(outcome.submits, 1, "one prompt reached the app");
    assert_sent_once_and_shown_at_once(&outcome);
    for sample in v["samples"].as_array().unwrap() {
        assert_eq!(
            count(sample, "users"),
            1,
            "duplicated or vanished: {sample} in {v}"
        );
    }
    let last = v["samples"].as_array().unwrap().last().unwrap();
    assert_eq!(
        count(last, "pending"),
        0,
        "echo replaced by server copy: {v}"
    );
    assert!(
        last["agent"]
            .as_str()
            .unwrap_or("")
            .contains("describes the project"),
        "reply rendered: {v}"
    );
}

// Review fix (#82): a plan waiting for approval keeps the prompt visible
// (the server shows it as a pending prompt), with no timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
async fn prompt_waiting_for_plan_approval_stays_once() {
    let Some(outcome) = run_scenario(Scenario::PlanWaits).await else {
        return;
    };
    let v = &outcome.v;
    assert_eq!(v["posts"].as_u64(), Some(1), "exactly one POST: {v}");
    assert_sent_once_and_shown_at_once(&outcome);
    for sample in v["samples"].as_array().unwrap() {
        assert_eq!(
            count(sample, "users"),
            1,
            "duplicated or vanished: {sample} in {v}"
        );
    }
    let last = v["samples"].as_array().unwrap().last().unwrap();
    assert_eq!(
        count(last, "pending"),
        0,
        "the server copy replaced the echo: {v}"
    );
}

// Review fix (#82): accepted (202) and then refused: no ghost message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
async fn prompt_refused_after_accept_leaves_no_ghost() {
    let Some(outcome) = run_scenario(Scenario::RefusedLater).await else {
        return;
    };
    let v = &outcome.v;
    assert_sent_once_and_shown_at_once(&outcome);
    let samples = v["samples"].as_array().unwrap();
    for sample in samples {
        assert!(count(sample, "users") <= 1, "duplicated: {sample} in {v}");
    }
    let last = samples.last().unwrap();
    assert_eq!(
        count(last, "users"),
        0,
        "refused prompt must not linger: {v}"
    );
    assert_eq!(count(last, "pending"), 0, "no ghost echo: {v}");
    assert!(
        last["system"]
            .as_str()
            .unwrap_or("")
            .contains("already running"),
        "the refusal is shown: {v}"
    );
}

// Review fix (#82): two quick sends each keep their own echo until their own
// server copy arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
async fn two_quick_sends_each_show_once() {
    let Some(outcome) = run_scenario(Scenario::TwoSends).await else {
        return;
    };
    let v = &outcome.v;
    assert_eq!(v["posts"].as_u64(), Some(2), "one POST per send: {v}");
    assert_eq!(outcome.submits, 2);
    assert_sent_once_and_shown_at_once(&outcome);
    let second_at = v["second_at"].as_f64().expect("second send happened");
    for sample in v["samples"].as_array().unwrap() {
        assert_eq!(count(sample, "users"), 1, "first prompt: {sample} in {v}");
        let at = sample["at"].as_f64().unwrap_or(0.0);
        if at > second_at {
            assert_eq!(count(sample, "users2"), 1, "second prompt: {sample} in {v}");
        }
    }
    let last = v["samples"].as_array().unwrap().last().unwrap();
    assert_eq!(count(last, "pending"), 0, "both echoes replaced: {v}");
}
