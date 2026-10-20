//! TC-1204 (SPEC-dashboard-fullwidth FR-1200, FR-1202, FR-1203, FR-1205,
//! FR-1209, FR-1210): in headless Chrome, the center column fills the content
//! area, the Documents panel starts collapsed and toggles from the keyboard,
//! and an agent turn's text is rendered once: in the chat, with a compact
//! document card, while the Documents row keeps a one-line reference. A turn
//! whose message is not in the chat still shows its output tail in
//! Documents. Needs Chrome/Chromium and python Playwright, so it is
//! `#[ignore]`d; run with
//! `cargo test --test web_dashboard_fullwidth -- --ignored`.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

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

// TC-1204: document_text_rendered_once_in_headless_chrome
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn document_text_rendered_once_in_headless_chrome() {
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

    let workspace = tempfile::tempdir().expect("temp workspace");
    let home = tempfile::tempdir().expect("temp home");
    std::fs::write(workspace.path().join("README.md"), "# Full width\n").expect("write doc");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace.path())
        .env("HOME", home.path())
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn dmuxai serve");
    let stdout = child.stdout.take().expect("stdout is piped");
    let _guard = ChildGuard(Some(child));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("dashboard URL line");
    let url = line
        .trim()
        .strip_prefix("dmuxai dashboard: ")
        .unwrap_or_else(|| panic!("unexpected first stdout line: {line:?}"))
        .to_owned();

    let script = r##"
import sys, json, time
from playwright.sync_api import sync_playwright

BODY = "SECOND-LINE-ONLY-ONCE"
TURNS = {"turns": [
    {"id": "t1", "agent": "demo", "graph_id": None, "node_id": None, "started_ms": 1, "finished_ms": 2,
     "files": [{"path": "docs/plan.md", "change": "created"}], "truncated": False,
     "output_tail": ["I wrote docs/plan.md.", BODY, "third line"], "links": [], "message_id": "c1:1"},
    {"id": "t2", "agent": "demo", "graph_id": "g", "node_id": "n1", "started_ms": 1, "finished_ms": 2,
     "files": [{"path": "notes.txt", "change": "modified"}], "truncated": False,
     "output_tail": ["GRAPH-ONLY-OUTPUT"], "links": [], "message_id": "other:3"},
]}
SNAP = {"source": "tui", "workspace": "/tmp/x", "agents": [], "session": {
    "conversation_id": "c1", "conversation_title": "t", "selected_agent": "demo", "details_expanded": False,
    "agents": [{"id": "demo", "label": "demo", "allowed": True, "busy": False, "modes": ["ask"], "mode": "ask",
                "models": [], "model": None, "effort": None, "models_loading": False}],
    "conversations": [], "transcript_omitted": 0,
    "transcript": [
        {"id": "c1:0", "role": "user", "agent": None, "content": "write the plan", "streaming": False},
        {"id": "c1:1", "role": "agent", "agent": "demo", "content": "I wrote docs/plan.md.\n" + BODY + "\nthird line\n```rust\nfn main() {}\n```", "streaming": False},
    ]}}

out = {}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1440, "height": 900}, bypass_csp=True)
    pg.on("pageerror", lambda e: print("PAGEERROR", e, file=sys.stderr))
    pg.route("**/api/docs/changes*", lambda route: route.fulfill(
        status=200, content_type="application/json", body=json.dumps(TURNS)))
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#session:not([hidden]) .session-prompt", timeout=15000)
    out["initial"] = pg.evaluate("""() => ({
        expanded: document.querySelector('#docs-toggle').getAttribute('aria-expanded'),
        hidden: document.querySelector('#docs-body').hidden })""")
    pg.evaluate("snap => document.dispatchEvent(new CustomEvent('dmuxai:snapshot', {detail: snap}))", SNAP)
    pg.wait_for_selector("#docs-changes li.doc-turn", state="attached", timeout=15000)
    pg.wait_for_selector(".msg-doc-card", timeout=15000)
    pg.click(".msg-doc-card-open")
    pg.wait_for_timeout(500)
    out["after_open"] = pg.evaluate("""(body) => {
        const text = document.body.innerText;
        const t1 = document.querySelector('#docs-changes li.doc-turn[data-turn-id="t1"]');
        const t2 = document.querySelector('#docs-changes li.doc-turn[data-turn-id="t2"]');
        const card = document.querySelector('li.msg[data-msg-id="c1:1"] .msg-doc-card');
        const r = q => { const x = document.querySelector(q).getBoundingClientRect(); return {left: x.left, width: x.width}; };
        return {
            expanded: document.querySelector('#docs-toggle').getAttribute('aria-expanded'),
            hidden: document.querySelector('#docs-body').hidden,
            body_count: text.split(body).length - 1,
            graph_only_count: text.split('GRAPH-ONLY-OUTPUT').length - 1,
            t1_tail: t1 ? t1.querySelectorAll('pre.doc-tail').length : -1,
            t1_ref: t1 ? t1.querySelectorAll('.doc-turn-ref').length : -1,
            t2_tail: t2 ? t2.querySelectorAll('pre.doc-tail').length : -1,
            card_text: card ? card.innerText : null,
            code_blocks: document.querySelectorAll('li.msg[data-msg-id="c1:1"] pre.msg-code').length,
            user_align: getComputedStyle(document.querySelector('li.msg-user')).alignItems,
            vw: window.innerWidth, main: r('main'),
            dock_l: parseFloat(getComputedStyle(document.body).paddingLeft),
            dock_r: parseFloat(getComputedStyle(document.body).paddingRight), session: r('#session'), prompt: r('.session-prompt')
        };
    }""", BODY)
    pg.focus("#docs-toggle")
    pg.keyboard.press("Enter")
    out["after_enter"] = pg.evaluate("""() => ({
        expanded: document.querySelector('#docs-toggle').getAttribute('aria-expanded'),
        hidden: document.querySelector('#docs-body').hidden })""")
    print(json.dumps(out))
    b.close()
"##;

    let mut py_child = Command::new(python)
        .args(["-c", script])
        .arg(&chrome)
        .arg(&url)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn playwright");
    let (tx_out, rx_out) = mpsc::channel();
    let mut py_stdout = py_child.stdout.take().expect("py stdout");
    std::thread::spawn(move || {
        use std::io::Read;
        let mut out = Vec::new();
        let _ = py_stdout.read_to_end(&mut out);
        let _ = tx_out.send(out);
    });
    let out_bytes = match rx_out.recv_timeout(Duration::from_secs(90)) {
        Ok(b) => {
            let _ = py_child.wait();
            b
        }
        Err(_) => {
            let _ = py_child.kill();
            let _ = py_child.wait();
            panic!("playwright execution timed out after 90s");
        }
    };
    let text = String::from_utf8_lossy(&out_bytes);
    let json_line = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON result from playwright:\n{text}"));
    let v: serde_json::Value = serde_json::from_str(json_line).expect("json result");

    // FR-1202: collapsed by default.
    assert_eq!(v["initial"]["expanded"], "false", "{v}");
    assert_eq!(v["initial"]["hidden"], true, "{v}");
    // FR-1210: the card opens the panel.
    let open = &v["after_open"];
    assert_eq!(open["expanded"], "true", "{v}");
    assert_eq!(open["hidden"], false, "{v}");
    // FR-1209: the reply body is on the page exactly once (the chat).
    assert_eq!(open["body_count"], 1, "reply text rendered twice: {v}");
    assert_eq!(open["t1_tail"], 0, "{v}");
    assert_eq!(open["t1_ref"], 1, "{v}");
    // A turn whose message is not in the chat keeps its tail in Documents.
    assert_eq!(open["t2_tail"], 1, "{v}");
    assert_eq!(open["graph_only_count"], 1, "{v}");
    let card = open["card_text"].as_str().expect("doc card in chat");
    assert!(card.contains("plan.md"), "{v}");
    assert!(!card.contains("SECOND-LINE-ONLY-ONCE"), "{v}");
    // FR-1207 / FR-1206: code block and right-aligned user bubble.
    assert_eq!(open["code_blocks"], 1, "{v}");
    assert_eq!(open["user_align"], "flex-end", "{v}");
    // FR-1200: main spans the content area (viewport minus docked sidebars).
    let vw = open["vw"].as_f64().unwrap();
    let dock_l = open["dock_l"].as_f64().unwrap();
    let dock_r = open["dock_r"].as_f64().unwrap();
    let content = vw - dock_l - dock_r;
    let main_l = open["main"]["left"].as_f64().unwrap();
    let main_w = open["main"]["width"].as_f64().unwrap();
    assert!((main_l - dock_l).abs() <= 1.0, "{v}");
    assert!(
        (main_w - content).abs() <= 1.0,
        "main {main_w} vs content {content}: {v}"
    );
    let session_w = open["session"]["width"].as_f64().unwrap();
    assert!(
        session_w >= content - 40.0,
        "chat panel {session_w} not full width: {v}"
    );
    // FR-1208: the composer column is centered in the content area and capped.
    let prompt = &open["prompt"];
    let p_left = prompt["left"].as_f64().unwrap();
    let p_w = prompt["width"].as_f64().unwrap();
    assert!((700.0..=860.0).contains(&p_w), "{v}");
    let center = dock_l + content / 2.0;
    assert!((p_left + p_w / 2.0 - center).abs() <= 2.0, "{v}");
    // FR-1203: Enter on the focused toggle collapses again.
    assert_eq!(v["after_enter"]["expanded"], "false", "{v}");
    assert_eq!(v["after_enter"]["hidden"], true, "{v}");
}
