//! Dashboard follow-ups while a reply streams (benchmarked from Cursor's
//! agent UI): the composer stays usable during a run ("Send follow-up" and a
//! Stop button), a follow-up sent then is shown as a queued user message,
//! several keep their order, each can be removed or edited before it starts,
//! and Stop keeps the queue (the next follow-up starts after the cancelled
//! run). A fake app stands in for dmuxai behind the real server and assets.
//! Needs Chrome/Chromium and python Playwright, so it is `#[ignore]`d; run
//! with `cargo test --test web_followup_queue -- --ignored`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dmuxai::web::commands::{DashboardCommand, PromptAccepted, RunTodoError, SessionCommand};
use dmuxai::web::{
    DashboardHub, DashboardServices, ServerConfig, SessionAgent, SessionMessage, SessionView,
    SnapshotSource, start_server,
};

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

/// The fake app: one run at a time, follow-ups queue behind it.
#[derive(Default)]
struct Fake {
    seq: u64,
    /// Finished or cancelled turns: (prompt, reply).
    done: Vec<(String, String)>,
    /// The running prompt and when it ends (None: runs until Stop).
    running: Option<(String, Option<Instant>)>,
    queue: Vec<(u64, String)>,
    /// Prompts in the order they started.
    started: Vec<String>,
    cancels: usize,
    submits: usize,
}

impl Fake {
    fn view(&self) -> SessionView {
        let mut transcript = vec![msg(0, "system", "ready", false)];
        let mut index = 1;
        for (prompt, reply) in &self.done {
            transcript.push(msg(index, "user", prompt, false));
            transcript.push(msg(index + 1, "agent", reply, false));
            index += 2;
        }
        if let Some((prompt, _)) = &self.running {
            transcript.push(msg(index, "user", prompt, false));
            transcript.push(msg(index + 1, "agent", "Working on it", true));
        }
        for (id, text) in &self.queue {
            transcript.push(SessionMessage {
                id: format!("conv-1:queued-{id}"),
                prompt_id: Some(*id),
                queued: true,
                ..msg(0, "user", text, false)
            });
        }
        let busy = self.running.is_some();
        SessionView {
            conversation_id: "conv-1".to_owned(),
            conversation_title: "Follow-ups".to_owned(),
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
            prompt_seq: self.seq,
            run_active: busy,
            ..Default::default()
        }
    }

    fn start(&mut self, text: String) {
        // The first prompt runs until Stop; follow-ups take 1.5 s each.
        let end = (!self.started.is_empty()).then(|| Instant::now() + Duration::from_millis(1500));
        self.started.push(text.clone());
        self.running = Some((text, end));
    }

    /// Ends a run whose time is up and starts the next queued follow-up.
    fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some((prompt, Some(end))) = &self.running
            && Instant::now() >= *end
        {
            let reply = format!("Done: {prompt}");
            self.done.push((prompt.clone(), reply));
            self.running = None;
            changed = true;
        }
        if self.running.is_none() && !self.queue.is_empty() {
            let (_, text) = self.queue.remove(0);
            self.start(text);
            changed = true;
        }
        changed
    }
}

fn msg(index: usize, role: &str, content: &str, streaming: bool) -> SessionMessage {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
async fn followups_queue_in_order_and_survive_stop_in_headless_chrome() {
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

    let fake = Arc::new(Mutex::new(Fake::default()));
    let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_owned());
    hub.publish(Vec::new(), None);
    hub.set_session(Some(fake.lock().unwrap().view()));
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(8);
    let config = ServerConfig {
        bind: "127.0.0.1".parse().unwrap(),
        port: 0,
        allow_remote: false,
        token: "test-token-followup".to_owned(),
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

    let ticker_fake = fake.clone();
    let ticker_hub = hub.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut fake = ticker_fake.lock().unwrap();
            if fake.tick() {
                ticker_hub.set_session(Some(fake.view()));
            }
        }
    });
    let app_fake = fake.clone();
    let app_hub = hub.clone();
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            let mut fake = app_fake.lock().unwrap();
            match command {
                DashboardCommand::SubmitPrompt {
                    text,
                    allow_queue,
                    reply,
                    ..
                } => {
                    fake.submits += 1;
                    fake.seq += 1;
                    let id = fake.seq;
                    let queued = fake.running.is_some() || !fake.queue.is_empty();
                    if queued && !allow_queue {
                        let _ = reply.send(Err(RunTodoError::Busy("busy".to_owned())));
                        continue;
                    }
                    if queued {
                        fake.queue.push((id, text));
                    } else {
                        fake.start(text);
                    }
                    app_hub.set_session(Some(fake.view()));
                    let _ = reply.send(Ok(PromptAccepted {
                        conversation_id: "conv-1".to_owned(),
                        agent: "cursor".to_owned(),
                        prompt_id: Some(id),
                        message_id: (!queued)
                            .then(|| format!("conv-1:{}", 1 + 2 * fake.done.len())),
                        queued,
                    }));
                }
                DashboardCommand::Session { command, reply } => {
                    let result = match command {
                        SessionCommand::Cancel { .. } => {
                            fake.cancels += 1;
                            if let Some((prompt, _)) = fake.running.take() {
                                fake.done.push((prompt, "Cancelled.".to_owned()));
                            }
                            Ok(())
                        }
                        SessionCommand::QueueRemove { id } => {
                            let before = fake.queue.len();
                            fake.queue.retain(|(qid, _)| *qid != id);
                            if fake.queue.len() == before {
                                Err(RunTodoError::Missing("gone".to_owned()))
                            } else {
                                Ok(())
                            }
                        }
                        SessionCommand::QueueEdit { id, text } => {
                            match fake.queue.iter_mut().find(|(qid, _)| *qid == id) {
                                Some(item) => {
                                    item.1 = text;
                                    Ok(())
                                }
                                None => Err(RunTodoError::Missing("gone".to_owned())),
                            }
                        }
                        _ => Ok(()),
                    };
                    // A cancelled run is ended; the next follow-up starts on
                    // the following tick, like the app's run loop.
                    let view = fake.view();
                    app_hub.set_session(Some(view.clone()));
                    let _ = reply.send(result.map(|()| view));
                }
                _ => {}
            }
        }
    });

    let script = r##"
import sys, json, time
from playwright.sync_api import sync_playwright

out = {"steps": {}}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1280, "height": 900}, color_scheme="light")
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#session:not([hidden]) .session-input:not([disabled])", timeout=15000)

    def state():
        return pg.evaluate("""() => {
            const text = li => (li.querySelector('.msg-text') || li).textContent.trim();
            const users = Array.from(document.querySelectorAll('#session li.msg-user'));
            const stop = document.querySelector('[data-parity="prompt-stop"]');
            const items = Array.from(document.querySelectorAll('#session .session-transcript > li'));
            const working = items.findIndex(li => li.classList.contains('msg-working') || li.classList.contains('msg-streaming'));
            const firstQueued = items.findIndex(li => li.classList.contains('msg-queued'));
            return {
              users: users.filter(li => !li.classList.contains('msg-queued')).map(text),
              queued: users.filter(li => li.classList.contains('msg-queued')).map(text),
              badges: document.querySelectorAll('#session li.msg-queued .msg-queued-badge').length,
              pending: document.querySelectorAll('#session li.msg-pending').length,
              agent: Array.from(document.querySelectorAll('#session li.msg-agent .msg-text')).map(n => n.textContent.trim()),
              placeholder: document.querySelector('.session-input').placeholder,
              inputDisabled: document.querySelector('.session-input').disabled,
              stopVisible: !!stop && !stop.hidden,
              queuedAfterWorking: firstQueued < 0 || working < 0 || firstQueued > working
            };
        }""")

    def send(t):
        pg.fill(".session-input", t)
        pg.press(".session-input", "Enter")
        pg.wait_for_function("() => document.querySelector('.session-input').value === ''", timeout=5000)
        pg.wait_for_timeout(250)

    def queued_li(t):
        return pg.locator("#session li.msg-queued", has_text=t)

    out["steps"]["idle"] = state()
    send("Task A")
    pg.wait_for_timeout(400)
    out["steps"]["running"] = state()
    send("Task B")
    send("Task C")
    send("Task D")
    pg.wait_for_timeout(400)
    out["steps"]["queued"] = state()
    if len(sys.argv) > 3:
        pg.screenshot(path=sys.argv[3] + "-queued.png")
    queued_li("Task D").locator("[aria-label='Remove queued follow-up']").click()
    pg.wait_for_timeout(400)
    out["steps"]["removed"] = state()
    queued_li("Task C").get_by_role("button", name="Edit").click()
    pg.fill("#session li.msg-queued textarea.msg-queued-edit", "Task C edited")
    pg.locator("#session li.msg-queued").get_by_role("button", name="Save").click()
    pg.wait_for_timeout(400)
    out["steps"]["edited"] = state()
    pg.click("[data-parity='prompt-stop']")
    out["steps"]["stopped"] = state()
    # The next follow-up starts on its own once the cancelled run ended.
    pg.wait_for_function("""() => Array.from(document.querySelectorAll('#session li.msg-user:not(.msg-queued)'))
        .some(li => (li.querySelector('.msg-text') || li).textContent.trim() === 'Task B')""", timeout=5000)
    pg.wait_for_timeout(200)
    out["steps"]["after_stop"] = state()
    pg.wait_for_function("() => !document.querySelector('#session li.msg-queued') && !document.querySelector('#session li.msg-streaming')", timeout=10000)
    pg.wait_for_timeout(300)
    out["steps"]["done"] = state()
    if len(sys.argv) > 3:
        pg.screenshot(path=sys.argv[3] + "-ran.png")
    print(json.dumps(out))
    b.close()
"##;

    let shots = std::env::var("DMUXAI_FOLLOWUP_SHOTS").ok();
    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(python);
        cmd.args(["-c", script]).arg(&chrome).arg(&url);
        if let Some(prefix) = shots {
            cmd.arg(prefix);
        }
        cmd.stderr(Stdio::inherit())
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
    let step = |name: &str| v["steps"][name].clone();
    let strings = |value: &serde_json::Value| -> Vec<String> {
        value
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|s| s.as_str().unwrap_or("").to_owned())
            .collect()
    };

    let idle = step("idle");
    assert_eq!(idle["stopVisible"], false, "{v}");
    assert_ne!(idle["placeholder"], "Send follow-up", "{v}");

    let running = step("running");
    assert_eq!(running["placeholder"], "Send follow-up", "{v}");
    assert_eq!(running["stopVisible"], true, "{v}");
    assert_eq!(running["inputDisabled"], false, "{v}");

    let queued = step("queued");
    assert_eq!(strings(&queued["users"]), ["Task A"], "{v}");
    assert_eq!(
        strings(&queued["queued"]),
        ["Task B", "Task C", "Task D"],
        "{v}"
    );
    assert_eq!(queued["badges"], 3, "{v}");
    assert_eq!(queued["pending"], 0, "echoes claimed by queued items: {v}");
    assert_eq!(queued["queuedAfterWorking"], true, "{v}");

    assert_eq!(
        strings(&step("removed")["queued"]),
        ["Task B", "Task C"],
        "{v}"
    );
    assert_eq!(
        strings(&step("edited")["queued"]),
        ["Task B", "Task C edited"],
        "{v}"
    );
    // Stop ends Task A but keeps the queue.
    let stopped = step("stopped");
    let after_stop = step("after_stop");
    assert!(
        strings(&stopped["queued"]).len() + strings(&after_stop["users"]).len() >= 2,
        "{v}"
    );
    assert_eq!(strings(&after_stop["users"]), ["Task A", "Task B"], "{v}");
    assert_eq!(strings(&after_stop["queued"]), ["Task C edited"], "{v}");

    let done = step("done");
    assert_eq!(
        strings(&done["users"]),
        ["Task A", "Task B", "Task C edited"],
        "{v}"
    );
    assert!(strings(&done["queued"]).is_empty(), "{v}");
    assert_eq!(done["pending"], 0, "{v}");
    assert_eq!(
        strings(&done["agent"]),
        ["Cancelled.", "Done: Task B", "Done: Task C edited"],
        "{v}"
    );
    assert_eq!(done["stopVisible"], false, "{v}");

    let fake = fake.lock().unwrap();
    assert_eq!(fake.submits, 4);
    assert_eq!(fake.cancels, 1);
    assert_eq!(fake.started, ["Task A", "Task B", "Task C edited"]);
}
