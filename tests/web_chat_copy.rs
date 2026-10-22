//! Dashboard chat "copy reply" button: each agent message has one copy
//! button (not system/user lines) that copies the raw Markdown source, keeps
//! working across snapshot re-renders, shows a "Copied" state, and falls back
//! to `document.execCommand('copy')` without `navigator.clipboard`.
//! Needs Chrome/Chromium and python Playwright, so it is `#[ignore]`d; run
//! with `cargo test --test web_chat_copy -- --ignored`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use dmuxai::web::chat_markdown::render_chat_markdown;
use dmuxai::web::{
    DashboardHub, DashboardServices, ServerConfig, SessionAgent, SessionMessage, SessionView,
    SnapshotSource, start_server,
};

const REPLY: &str = "## Plan\n\n- **Step 1**: read `README.md`\n- Step 2\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n";

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

fn message(index: usize, role: &str, content: &str) -> SessionMessage {
    SessionMessage {
        id: format!("conv-1:{index}"),
        role: role.to_owned(),
        agent: (role == "agent").then(|| "cursor".to_owned()),
        content: content.to_owned(),
        content_html: (role == "agent").then(|| render_chat_markdown(content)),
        streaming: false,
        images: Vec::new(),
        prompt_id: None,
        queued: false,
    }
}

fn session(revision_note: &str) -> SessionView {
    SessionView {
        conversation_id: "conv-1".to_owned(),
        conversation_title: "Sample".to_owned(),
        selected_agent: "cursor".to_owned(),
        agents: vec![SessionAgent {
            id: "cursor".to_owned(),
            label: "Cursor".to_owned(),
            mode: "agent".to_owned(),
            modes: vec!["agent".to_owned()],
            allowed: true,
            ..Default::default()
        }],
        transcript: vec![
            message(0, "system", "Runners: cursor."),
            message(1, "user", "Explain the plan"),
            message(2, "agent", REPLY),
            message(3, "system", revision_note),
        ],
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
async fn copy_button_copies_raw_markdown_in_headless_chrome() {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome/Chromium found (set DMUXAI_CHROME)");
        return;
    };
    let python = "python3";
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
    hub.set_session(Some(session("first snapshot")));
    let config = ServerConfig {
        bind: "127.0.0.1".parse().unwrap(),
        port: 0,
        allow_remote: false,
        token: "test-token-copy-reply".to_owned(),
        services: DashboardServices {
            web_attached: true,
            ..Default::default()
        },
    };
    let server = start_server(config, hub.clone())
        .await
        .expect("start server");
    let url = server.url.clone();

    // Re-render the transcript a few times while the page is open.
    let rerender_hub = hub.clone();
    tokio::spawn(async move {
        for n in 0..6 {
            tokio::time::sleep(Duration::from_millis(400)).await;
            rerender_hub.set_session(Some(session(&format!("snapshot {n}"))));
        }
    });

    let script = r##"
import sys, json
from playwright.sync_api import sync_playwright

out = {}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    ctx = b.new_context(viewport={"width": 1280, "height": 800}, color_scheme="light")
    ctx.grant_permissions(["clipboard-read", "clipboard-write"], origin=sys.argv[2].split("/?")[0].rstrip("/"))
    pg = ctx.new_page()
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#session li.msg-agent .msg-copy", state="attached", timeout=15000)
    pg.wait_for_timeout(2800)  # let the re-render snapshots land
    out["buttons"] = pg.evaluate("""() => ({
        agent: document.querySelectorAll('#session li.msg-agent .msg-copy').length,
        system: document.querySelectorAll('#session li.msg-system .msg-copy').length,
        user: document.querySelectorAll('#session li.msg-user .msg-copy').length,
        label: document.querySelector('#session li.msg-agent .msg-copy').getAttribute('aria-label'),
        hiddenOpacity: getComputedStyle(document.querySelector('#session li.msg-agent .msg-actions')).opacity
    })""")
    pg.hover("#session li.msg-agent .msg-text")
    pg.wait_for_timeout(300)
    out["hoverOpacity"] = pg.evaluate("() => getComputedStyle(document.querySelector('#session li.msg-agent .msg-actions')).opacity")
    pg.click("#session li.msg-agent .msg-copy")
    pg.wait_for_timeout(200)
    out["clipboard"] = pg.evaluate("() => navigator.clipboard.readText()")
    out["copied"] = pg.evaluate("""() => {
        const b = document.querySelector('#session li.msg-agent .msg-copy');
        return {label: b.getAttribute('aria-label'), done: b.classList.contains('msg-copy-done'),
                tip: (document.querySelector('#session li.msg-agent .msg-copy-tip') || {}).textContent};
    }""")
    pg.wait_for_timeout(1800)
    out["reset"] = pg.evaluate("() => document.querySelector('#session li.msg-agent .msg-copy').getAttribute('aria-label')")
    # Fallback path: no async clipboard API -> execCommand('copy') with the raw text.
    out["fallback"] = pg.evaluate("""() => new Promise(resolve => {
        Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
        let seen = null;
        const orig = document.execCommand.bind(document);
        document.execCommand = function (cmd) {
            if (cmd === 'copy') { const a = document.activeElement; seen = a && a.value; return true; }
            return orig.apply(document, arguments);
        };
        document.querySelector('#session li.msg-agent .msg-copy').click();
        setTimeout(() => resolve({ seen: seen,
            label: document.querySelector('#session li.msg-agent .msg-copy').getAttribute('aria-label') }), 300);
    })""")
    print(json.dumps(out))
    b.close()
"##;

    let output = tokio::task::spawn_blocking(move || {
        Command::new(python)
            .args(["-c", script])
            .arg(&chrome)
            .arg(&url)
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

    assert_eq!(
        v["buttons"]["agent"], 1,
        "one copy button per reply after re-renders: {v}"
    );
    assert_eq!(
        v["buttons"]["system"], 0,
        "no copy button on system lines: {v}"
    );
    assert_eq!(v["buttons"]["user"], 0, "no copy button on user lines: {v}");
    assert_eq!(v["buttons"]["label"], "Copy reply");
    assert_eq!(
        v["buttons"]["hiddenOpacity"], "0",
        "hidden until hover: {v}"
    );
    assert_eq!(v["hoverOpacity"], "1", "shown on hover: {v}");
    assert_eq!(v["clipboard"], REPLY, "raw markdown copied: {v}");
    assert_eq!(v["copied"]["label"], "Copied", "{v}");
    assert_eq!(v["copied"]["done"], true, "{v}");
    assert_eq!(v["copied"]["tip"], "Copied", "{v}");
    assert_eq!(v["reset"], "Copy reply", "label restored: {v}");
    assert_eq!(
        v["fallback"]["seen"], REPLY,
        "execCommand fallback copies raw text: {v}"
    );
    assert_eq!(v["fallback"]["label"], "Copied", "{v}");
}
