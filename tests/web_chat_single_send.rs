//! Dashboard chat sends a prompt once and shows the user's message once
//! (double-message bug after #80/#81): the message appears right after
//! Enter (optimistic echo), a second Enter or Send click while the first
//! request is in flight does not post again, and when the server transcript
//! catches up and the reply streams there is still exactly one user message.
//! Needs Chrome/Chromium and python Playwright, so it is `#[ignore]`d; run
//! with `cargo test --test web_chat_single_send -- --ignored`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use dmuxai::web::commands::{DashboardCommand, PromptAccepted};
use dmuxai::web::{
    DashboardHub, DashboardServices, ServerConfig, SessionAgent, SessionMessage, SessionView,
    SnapshotSource, start_server,
};

const PROMPT: &str = "Explain the README";

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

fn session(transcript: Vec<SessionMessage>, busy: bool) -> SessionView {
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
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
async fn prompt_is_sent_and_shown_once_in_headless_chrome() {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome/Chromium found (set DMUXAI_CHROME)");
        return;
    };
    let python = if cfg!(windows) { "python" } else { "python3" };
    let probe = Command::new(python)
        .args(["-c", "import playwright"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.map(|status| status.success()).unwrap_or(false) {
        println!("SKIP: python playwright not installed");
        return;
    }

    let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_owned());
    hub.publish(Vec::new(), None);
    hub.set_session(Some(session(
        vec![message(0, "system", "ready", false)],
        false,
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

    // Fake app: accept the prompt at once, but (like the triage judge) put
    // the user message into the transcript only 1.5 s later, then stream.
    let submits = Arc::new(AtomicUsize::new(0));
    let submits_seen = submits.clone();
    let fake_hub = hub.clone();
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            if let DashboardCommand::SubmitPrompt { text, reply, .. } = command {
                submits_seen.fetch_add(1, Ordering::SeqCst);
                let _ = reply.send(Ok(PromptAccepted {
                    conversation_id: "conv-1".to_owned(),
                    agent: "cursor".to_owned(),
                }));
                let hub = fake_hub.clone();
                tokio::spawn(async move {
                    let ready = message(0, "system", "ready", false);
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let user = message(1, "user", &text, false);
                    hub.set_session(Some(session(vec![ready.clone(), user.clone()], true)));
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                    let partial = message(2, "agent", "The README", true);
                    hub.set_session(Some(session(
                        vec![ready.clone(), user.clone(), partial],
                        true,
                    )));
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                    let done = message(2, "agent", "The README describes the project.", false);
                    hub.set_session(Some(session(vec![ready, user, done], false)));
                });
            }
        }
    });

    let script = r##"
import sys, json, time
from playwright.sync_api import sync_playwright

TEXT = sys.argv[3]
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
    while time.time() - start < 5.0:
        out["samples"].append(pg.evaluate("""t => ({
            at: 0,
            users: Array.from(document.querySelectorAll('#session li.msg-user'))
                .filter(li => (li.querySelector('.msg-text') || li).textContent.trim() === t).length,
            pending: document.querySelectorAll('#session li.msg-pending').length,
            agent: Array.from(document.querySelectorAll('#session li.msg-agent .msg-text'))
                .map(n => n.textContent.trim()).join('|'),
            input: document.querySelector('.session-input').value
        })""", TEXT))
        out["samples"][-1]["at"] = round(time.time() - start, 2)
        pg.wait_for_timeout(100)
    print(json.dumps(out))
    b.close()
"##;

    let output = tokio::task::spawn_blocking(move || {
        Command::new(python)
            .args(["-c", script])
            .arg(&chrome)
            .arg(&url)
            .arg(PROMPT)
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
    let samples = v["samples"].as_array().expect("samples");
    assert!(!samples.is_empty(), "{v}");

    assert_eq!(v["posts"].as_u64(), Some(1), "exactly one POST: {v}");
    assert_eq!(
        submits.load(Ordering::SeqCst),
        1,
        "one prompt reached the app"
    );
    let first = &samples[0];
    assert_eq!(
        first["users"].as_u64(),
        Some(1),
        "message shown at once: {v}"
    );
    assert_eq!(
        first["input"].as_str(),
        Some(""),
        "input cleared at once: {v}"
    );
    for sample in samples {
        assert!(
            sample["users"].as_u64().unwrap_or(0) <= 1,
            "user message duplicated: {sample} in {v}"
        );
        assert!(
            sample["users"].as_u64().unwrap_or(0) == 1,
            "user message vanished: {sample}"
        );
    }
    let last = samples.last().unwrap();
    assert_eq!(
        last["pending"].as_u64(),
        Some(0),
        "echo replaced by server message: {v}"
    );
    assert!(
        last["agent"]
            .as_str()
            .unwrap_or("")
            .contains("describes the project"),
        "reply rendered: {v}"
    );
}
