//! TC-1204, TC-1207, TC-1208 (SPEC-dashboard-fullwidth FR-1200, FR-1202, FR-1203, FR-1205,
//! FR-1209, FR-1210): in headless Chrome, the center column fills the content
//! area, the Documents panel starts collapsed and toggles from the keyboard,
//! and an agent turn's text is rendered once: in the chat, with a compact
//! document card, while the Documents row keeps a one-line reference. A turn
//! whose message is not in the chat still shows its output tail in
//! Documents. Needs Chrome/Chromium and python Playwright, so it is
//! `#[ignore]`d; run with
//! `cargo test --test web_dashboard_fullwidth -- --ignored`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
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

/// Chrome and python Playwright, or `None` (the test prints SKIP).
fn prepare() -> Option<(PathBuf, &'static str)> {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome/Chromium found (set DMUXAI_CHROME)");
        return None;
    };
    let python = "python3";
    let probe = Command::new(python)
        .args(["-c", "import playwright"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.map(|status| status.success()).unwrap_or(false) {
        println!("SKIP: python playwright not installed");
        return None;
    }
    Some((chrome, python))
}

/// `dmuxai serve --port 0` in `workspace`; returns the guard and the URL.
fn start_serve(workspace: &Path, home: &Path) -> (ChildGuard, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace)
        .env("HOME", home)
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn dmuxai serve");
    let stdout = child.stdout.take().expect("stdout is piped");
    let guard = ChildGuard(Some(child));
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
    (guard, url)
}

/// Runs a Playwright script with `argv = [chrome, url]`; returns the last
/// JSON line it prints.
fn run_probe(python: &str, chrome: &Path, url: &str, script: &str) -> serde_json::Value {
    let mut py_child = Command::new(python)
        .args(["-c", script])
        .arg(chrome)
        .arg(url)
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
    serde_json::from_str(json_line).expect("json result")
}

// TC-1204: document_text_rendered_once_in_headless_chrome
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn document_text_rendered_once_in_headless_chrome() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let workspace = tempfile::tempdir().expect("temp workspace");
    let home = tempfile::tempdir().expect("temp home");
    std::fs::write(workspace.path().join("README.md"), "# Full width\n").expect("write doc");
    let (_guard, url) = start_serve(workspace.path(), home.path());

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

    let v = run_probe(python, &chrome, &url, script);

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

/// A completed two-node run, for the Graph view toggle checks.
const DONE_RUN: &str = r#"{"plan":{"schema":"dmuxai.graph/v1","id":"run-done","orchestrator_model":"composer-2.5",
 "user_prompt":"Add CSV export","limits":{"max_parallel":2,"max_retries":1},
 "nodes":[{"id":"a","role":"plan","job":"plan","runner":"cursor","model_id":"composer-2.5","skill":null,"requires_gate":false,"status":"completed","attempt":1,"started_at_ms":1000,"finished_at_ms":61000},
          {"id":"b","role":"implement","job":"build","runner":"cursor","model_id":"composer-2.5","skill":null,"requires_gate":false,"status":"completed","attempt":1,"started_at_ms":61000,"finished_at_ms":121000}],
 "edges":[{"from":"a","to":"b"}]},"conversation_id":"c","phase":"completed","waiting_gate_node":null}"#;

/// A plan waiting for approval (#67 plan gate).
const APPROVAL_RUN: &str = r#"{"plan":{"schema":"dmuxai.graph/v1","id":"plan-wait","orchestrator_model":"composer-2.5",
 "user_prompt":"Add CSV export","limits":{"max_parallel":2,"max_retries":1},"nodes":[],"edges":[]},
 "conversation_id":"c","phase":"awaiting_approval","waiting_gate_node":null,
 "approval":{"state":"pending","revisions":[{"n":1,"source":"planner","steps":[
   {"id":"explore","role":"explore","job":"Read the reports page","runner":"cursor","model":"composer-2.5","depends_on":[]},
   {"id":"implement","role":"implement","job":"Add the CSV writer","runner":"cursor","model":"composer-2.5","depends_on":["explore"]}]}]}}"#;

fn workspace_with_run(json: &str) -> tempfile::TempDir {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let graphs = workspace.path().join(".dmuxai").join("graphs");
    std::fs::create_dir_all(&graphs).expect("graphs dir");
    std::fs::write(graphs.join("run.json"), json).expect("write run");
    workspace
}

// TC-1207: center_views_toggle_in_headless_chrome (FR-1211, FR-1212,
// FR-1213, FR-1214, FR-1216)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn center_views_toggle_in_headless_chrome() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let workspace = workspace_with_run(DONE_RUN);
    let home = tempfile::tempdir().expect("temp home");
    let (_guard, url) = start_serve(workspace.path(), home.path());
    let script = r##"
import sys, json
from playwright.sync_api import sync_playwright

STATE = """() => {
  const r = q => { const e = document.querySelector(q); const b = e.getBoundingClientRect(); return {left: b.left, width: b.width}; };
  const cs = getComputedStyle(document.body);
  return {
    graph_hidden: document.querySelector('#graph').hidden,
    docs_hidden: document.querySelector('#docs').hidden,
    graph_exp: document.querySelector('#graph-toggle').getAttribute('aria-expanded'),
    diff_exp: document.querySelector('#diff-toggle').getAttribute('aria-expanded'),
    chat_visible: !document.querySelector('#session').hidden,
    graph_rect: r('#graph'), canvas_w: document.querySelector('#graph-canvas').clientWidth,
    vw: innerWidth, dock_l: parseFloat(cs.paddingLeft), dock_r: parseFloat(cs.paddingRight),
    nodes: document.querySelectorAll('.graph-svg g.node').length,
    focus: document.activeElement ? (document.activeElement.id || document.activeElement.className) : null,
    stored: sessionStorage.getItem('dmuxai.centerView')
  };
}"""

out = {}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1440, "height": 900}, bypass_csp=True)
    pg.on("pageerror", lambda e: print("PAGEERROR", e, file=sys.stderr))
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#graph-toggle", timeout=15000)
    pg.wait_for_function("() => document.querySelector('#graph-id').textContent.length > 0", timeout=15000)
    out["initial"] = pg.evaluate(STATE)
    pg.click("#graph-toggle")
    pg.wait_for_function("() => document.querySelectorAll('.graph-svg g.node').length === 2", timeout=15000)
    pg.wait_for_timeout(400)
    out["graph_open"] = pg.evaluate(STATE)
    pg.click("#graph-toggle")
    out["graph_closed_click"] = pg.evaluate(STATE)
    pg.focus("#diff-toggle")
    pg.keyboard.press("Enter")
    out["diff_open"] = pg.evaluate(STATE)
    pg.click("#graph-toggle")
    out["switch_to_graph"] = pg.evaluate(STATE)
    pg.reload()
    pg.wait_for_selector("#graph-toggle", timeout=15000)
    pg.wait_for_timeout(500)
    out["after_reload"] = pg.evaluate(STATE)
    pg.keyboard.press("Escape")
    out["after_esc"] = pg.evaluate(STATE)
    print(json.dumps(out))
    b.close()
"##;
    let v = run_probe(python, &chrome, &url, script);

    // FR-1211: hidden by default, chat stays the center view.
    let init = &v["initial"];
    assert_eq!(init["graph_hidden"], true, "{v}");
    assert_eq!(init["docs_hidden"], true, "{v}");
    assert_eq!(init["graph_exp"], "false", "{v}");
    assert_eq!(init["diff_exp"], "false", "{v}");
    assert_eq!(init["chat_visible"], true, "{v}");
    // FR-1213: the open Graph view covers the center area.
    let open = &v["graph_open"];
    assert_eq!(open["graph_hidden"], false, "{v}");
    assert_eq!(open["graph_exp"], "true", "{v}");
    assert_eq!(open["nodes"], 2, "{v}");
    let content = open["vw"].as_f64().unwrap()
        - open["dock_l"].as_f64().unwrap()
        - open["dock_r"].as_f64().unwrap();
    let gw = open["graph_rect"]["width"].as_f64().unwrap();
    assert!(
        (gw - (content - 28.0)).abs() <= 2.0,
        "graph view {gw} vs content {content}: {v}"
    );
    let gl = open["graph_rect"]["left"].as_f64().unwrap();
    assert!(
        (gl - (open["dock_l"].as_f64().unwrap() + 14.0)).abs() <= 2.0,
        "{v}"
    );
    assert!(open["canvas_w"].as_f64().unwrap() >= content - 80.0, "{v}");
    assert_eq!(open["focus"], "btn sidebar-close center-view-close", "{v}");
    // FR-1212: clicking again hides it.
    assert_eq!(v["graph_closed_click"]["graph_hidden"], true, "{v}");
    assert_eq!(v["graph_closed_click"]["graph_exp"], "false", "{v}");
    // Keyboard opens Diff; one view at a time.
    assert_eq!(v["diff_open"]["docs_hidden"], false, "{v}");
    assert_eq!(v["diff_open"]["diff_exp"], "true", "{v}");
    let sw = &v["switch_to_graph"];
    assert_eq!(sw["docs_hidden"], true, "{v}");
    assert_eq!(sw["graph_hidden"], false, "{v}");
    // FR-1216: the open view survives a reload in the same tab.
    assert_eq!(v["after_reload"]["graph_hidden"], false, "{v}");
    assert_eq!(v["after_reload"]["stored"], "graph", "{v}");
    // FR-1212: Esc hides it.
    assert_eq!(v["after_esc"]["graph_hidden"], true, "{v}");
    assert_eq!(v["after_esc"]["stored"], "", "{v}");
}

// TC-1208: approval_opens_graph_in_headless_chrome (FR-1214, FR-1215)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn approval_opens_graph_in_headless_chrome() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let workspace = workspace_with_run(APPROVAL_RUN);
    let home = tempfile::tempdir().expect("temp home");
    let (_guard, url) = start_serve(workspace.path(), home.path());
    let script = r##"
import sys, json
from playwright.sync_api import sync_playwright

STATE = """() => ({
  graph_hidden: document.querySelector('#graph').hidden,
  approval_visible: !document.querySelector('#graph-approval').hidden && document.querySelector('#graph-approval').getClientRects().length > 0,
  label: document.querySelector('#graph-toggle').getAttribute('aria-label'),
  indicator: document.querySelector('#graph-indicator').hidden ? null : document.querySelector('#graph-indicator').textContent,
  card: (() => {
    const c = document.querySelector('#session .session-transcript > li.chat-aux-approval');
    if (!c) { return null; }
    const btn = k => { const b = c.querySelector('.chat-aux-approval-' + k); return b ? {disabled: b.disabled} : null; };
    return {text: c.innerText, approve: btn('approve'), edit: btn('edit'), reject: btn('reject'), open: btn('open'),
            visible: c.getClientRects().length > 0};
  })()
})"""

out = {}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1440, "height": 900}, bypass_csp=True)
    pg.on("pageerror", lambda e: print("PAGEERROR", e, file=sys.stderr))
    pg.goto(sys.argv[2])
    pg.wait_for_function("() => document.querySelector('#graph-phase').textContent === 'awaiting_approval'", timeout=15000)
    pg.wait_for_timeout(300)
    out["initial"] = pg.evaluate(STATE)
    pg.keyboard.press("Escape")
    pg.wait_for_timeout(1500)
    out["closed"] = pg.evaluate(STATE)
    pg.click("li.chat-aux-approval .chat-aux-approval-open")
    out["reopened"] = pg.evaluate(STATE)
    print(json.dumps(out))
    b.close()
"##;
    let v = run_probe(python, &chrome, &url, script);

    // FR-1214: a pending plan opens the Graph view with the approval panel.
    let init = &v["initial"];
    assert_eq!(init["graph_hidden"], false, "{v}");
    assert_eq!(init["approval_visible"], true, "{v}");
    assert_eq!(init["indicator"], "!", "{v}");
    assert_eq!(init["label"], "Graph, plan awaiting approval", "{v}");
    // FR-1215 / FR-1221: the chat carries the approval card the whole time.
    assert!(init["card"].is_object(), "{v}");
    // Closed while pending -> it stays closed (no re-open on the next
    // snapshot) and the inline card is visible in the chat.
    let closed = &v["closed"];
    assert_eq!(closed["graph_hidden"], true, "{v}");
    let card = &closed["card"];
    assert_eq!(card["visible"], true, "{v}");
    let text = card["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("Plan rev 1 is waiting for your approval"),
        "{v}"
    );
    assert!(text.contains("2 steps"), "{v}");
    // `dmuxai serve` is view-only: the decisions are shown but disabled;
    // "Open graph" still works.
    for k in ["approve", "edit", "reject"] {
        assert_eq!(card[k]["disabled"], true, "{k}: {v}");
    }
    assert_eq!(card["open"]["disabled"], false, "{v}");
    assert!(
        text.contains("view-only") || text.contains("View-only"),
        "{v}"
    );
    assert_eq!(v["reopened"]["graph_hidden"], false, "{v}");
    assert_eq!(v["reopened"]["approval_visible"], true, "{v}");
}

// TC-1211: inline_activity_rows_in_headless_chrome (FR-1217, FR-1218,
// FR-1219, FR-1220)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn inline_activity_rows_in_headless_chrome() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let workspace = workspace_with_run(DONE_RUN);
    let home = tempfile::tempdir().expect("temp home");
    let (_guard, url) = start_serve(workspace.path(), home.path());
    let script = r##"
import sys, json, copy
from playwright.sync_api import sync_playwright

TURNS = {"turns": [
    {"id": "t1", "agent": "demo", "graph_id": "run-done", "node_id": "b", "started_ms": 1, "finished_ms": 2,
     "files": [{"path": "src/export.rs", "change": "created"}, {"path": "docs/plan.md", "change": "modified"}],
     "truncated": False, "output_tail": ["Added the export."], "links": [], "message_id": "c1:4"},
]}
NODES = [
    {"id": "a", "role": "plan", "job": "plan", "runner": "cursor", "model_id": "m", "status": "completed", "attempt": 1,
     "started_at_ms": 1000, "finished_at_ms": 61000, "elapsed_ms": 60000, "deps": []},
    {"id": "b", "role": "implement", "job": "build", "runner": "cursor", "model_id": "m", "status": "completed", "attempt": 1,
     "started_at_ms": 61000, "finished_at_ms": 121000, "elapsed_ms": 60000, "deps": ["a"]},
]
SNAP = {"source": "tui", "workspace": "/tmp/x", "agents": [],
    "graph": {"id": "run-done", "phase": "completed", "user_prompt": "Add CSV export", "nodes": NODES},
    "session": {
    "conversation_id": "c1", "conversation_title": "t", "selected_agent": "demo", "details_expanded": False,
    "agents": [{"id": "demo", "label": "demo", "allowed": True, "busy": False, "modes": ["ask"], "mode": "ask",
                "models": [], "model": None, "effort": None, "models_loading": False}],
    "conversations": [], "transcript_omitted": 0,
    "transcript": [
        {"id": "c1:0", "role": "user", "agent": None, "content": "Add CSV export", "streaming": False},
        {"id": "c1:1", "role": "activity", "agent": "demo", "content": "\u25b8 read src/reports.rs \u2713 1.0s", "streaming": False},
        {"id": "c1:2", "role": "activity", "agent": "demo", "content": "\u25b8 edit src/export.rs \u2713 0.4s", "streaming": False},
        {"id": "c1:3", "role": "activity", "agent": "demo", "content": "\u25b8 run cargo test export \u2713 6.0s", "streaming": False},
        {"id": "c1:4", "role": "agent", "agent": "demo", "content": "Added the export.", "streaming": False},
    ]}}

ROWS = """() => {
  const t = document.querySelector('.session-transcript');
  const run = t.querySelector(':scope > li.chat-aux-run');
  const acts = t.querySelector(':scope > li.chat-aux-acts');
  const head = li => li ? li.querySelector('.chat-aux-head') : null;
  const kids = [...t.children].map(li => li.getAttribute('data-msg-id') || (li.classList.contains('chat-aux') ? li.getAttribute('data-aux-key') : li.className));
  return {
    order: kids,
    run_text: run ? head(run).innerText : null,
    run_expanded: run ? head(run).getAttribute('aria-expanded') : null,
    run_live: run ? run.classList.contains('chat-aux-live') : null,
    steps_hidden: run && run.querySelector('.chat-aux-steps') ? run.querySelector('.chat-aux-steps').hidden : null,
    steps: run ? [...run.querySelectorAll('.chat-aux-step-btn')].filter(b => b.getClientRects().length).map(b => b.innerText) : [],
    acts_text: acts ? head(acts).innerText : null,
    acts_expanded: acts ? head(acts).getAttribute('aria-expanded') : null,
    acts_visible: [...t.querySelectorAll(':scope > li.msg-activity')].filter(li => li.getClientRects().length).length,
    working: !!t.querySelector(':scope > li.msg-working'),
    files: [...document.querySelectorAll('.msg-doc-card-file-btn')].map(b => b.innerText),
    graph_hidden: document.querySelector('#graph').hidden,
    docs_hidden: document.querySelector('#docs').hidden,
    detail_visible: !document.querySelector('#graph-detail').hidden,
    detail_text: document.querySelector('#graph-detail').innerText,
    active: document.activeElement ? (document.activeElement.innerText || '') : ''
  };
}"""

out = {}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1440, "height": 900}, bypass_csp=True)
    pg.on("pageerror", lambda e: print("PAGEERROR", e, file=sys.stderr))
    pg.route("**/api/docs/changes*", lambda route: route.fulfill(
        status=200, content_type="application/json", body=json.dumps(TURNS)))
    pg.goto(sys.argv[2])
    pg.wait_for_function("() => document.querySelector('#graph-phase').textContent === 'completed'", timeout=15000)
    send = lambda snap: pg.evaluate("snap => document.dispatchEvent(new CustomEvent('dmuxai:snapshot', {detail: snap}))", snap)
    send(SNAP)
    pg.wait_for_selector("li.chat-aux-run", timeout=15000)
    pg.wait_for_selector(".msg-doc-card-file-btn", timeout=15000)
    out["initial"] = pg.evaluate(ROWS)
    pg.click("li.chat-aux-acts .chat-aux-head")
    pg.click("li.chat-aux-run .chat-aux-head")
    out["expanded"] = pg.evaluate(ROWS)
    # A step opens the Graph view focused on that node.
    pg.click("li.chat-aux-run .chat-aux-step-btn >> nth=1")
    pg.wait_for_timeout(900)
    out["step"] = pg.evaluate(ROWS)
    pg.keyboard.press("Escape")
    pg.keyboard.press("Escape")
    pg.wait_for_timeout(300)
    # A file row opens Diff at that file.
    pg.click(".msg-doc-card-file-btn >> nth=1")
    pg.wait_for_timeout(900)
    out["file"] = pg.evaluate(ROWS)
    pg.keyboard.press("Escape")
    # Live: a running graph and, separately, a single running agent.
    live = copy.deepcopy(SNAP)
    live["graph"]["phase"] = "active"
    live["graph"]["nodes"][1].update({"status": "running", "finished_at_ms": None, "elapsed_ms": None})
    live["session"]["agents"][0]["run"] = {"state": "running", "started_at_ms": 61000}
    live["session"]["transcript"] = live["session"]["transcript"][:4]
    send(live)
    pg.wait_for_selector("li.chat-aux-run.chat-aux-live", timeout=5000)
    out["live_graph"] = pg.evaluate(ROWS)
    solo = copy.deepcopy(live)
    solo["graph"] = None
    solo["agents"] = [{"id": "demo", "label": "demo", "activity": "\u25b8 run cargo test export",
                       "run": {"state": "running", "started_at_ms": 61000}}]
    send(solo)
    pg.wait_for_timeout(300)
    out["live_agent"] = pg.evaluate(ROWS)
    print(json.dumps(out))
    b.close()
"##;
    let v = run_probe(python, &chrome, &url, script);

    // FR-1217: one muted run row after the exchange, collapsed by default.
    let init = &v["initial"];
    let run = init["run_text"].as_str().expect("graph run row");
    assert!(run.contains("Ran"), "{v}");
    assert!(run.contains("2/2 steps"), "{v}");
    assert!(run.contains("2m"), "{v}");
    assert_eq!(init["run_expanded"], "false", "{v}");
    assert_eq!(init["steps_hidden"], true, "{v}");
    assert_eq!(init["run_live"], false, "{v}");
    let order: Vec<String> = init["order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap_or_default().to_string())
        .collect();
    let reply = order.iter().position(|k| k == "c1:4").expect("reply");
    assert_eq!(
        order.get(reply + 1).map(String::as_str),
        Some("graph:run-done"),
        "{v}"
    );
    // FR-1219: consecutive activity lines fold into one collapsed row.
    let acts = init["acts_text"].as_str().expect("actions row");
    assert!(acts.contains("3 actions"), "{v}");
    assert!(acts.contains("run cargo test export"), "{v}");
    assert_eq!(init["acts_expanded"], "false", "{v}");
    assert_eq!(init["acts_visible"], 0, "{v}");
    // FR-1220: the doc card lists each file with its change.
    let files = init["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{v}");
    assert!(files[0].as_str().unwrap().contains("Created"), "{v}");
    assert!(files[0].as_str().unwrap().contains("src/export.rs"), "{v}");
    assert!(files[1].as_str().unwrap().contains("Edited"), "{v}");
    // Expanding shows the steps with status icons and the activity lines.
    let exp = &v["expanded"];
    assert_eq!(exp["run_expanded"], "true", "{v}");
    assert_eq!(exp["acts_expanded"], "true", "{v}");
    assert_eq!(exp["acts_visible"], 3, "{v}");
    let steps = exp["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2, "{v}");
    assert!(steps[1].as_str().unwrap().contains('\u{2713}'), "{v}");
    assert!(steps[1].as_str().unwrap().contains("implement"), "{v}");
    // FR-1218: a step opens Graph with that node's detail.
    let step = &v["step"];
    assert_eq!(step["graph_hidden"], false, "{v}");
    assert_eq!(step["detail_visible"], true, "{v}");
    assert!(
        step["detail_text"].as_str().unwrap().contains("build"),
        "{v}"
    );
    // FR-1220: a file row opens Diff and focuses that file.
    let file = &v["file"];
    assert_eq!(file["docs_hidden"], false, "{v}");
    assert!(
        file["active"].as_str().unwrap().contains("docs/plan.md"),
        "{v}"
    );
    // Live graph row: spinner line with the running step and its runner.
    let lg = &v["live_graph"];
    assert_eq!(lg["run_live"], true, "{v}");
    let text = lg["run_text"].as_str().unwrap();
    assert!(text.contains("Running b on cursor"), "{v}");
    assert!(text.contains("1/2 steps"), "{v}");
    assert_eq!(lg["working"], false, "{v}");
    // Single-agent run: the agent's live activity line.
    let la = &v["live_agent"];
    let text = la["run_text"].as_str().expect("agent run row");
    assert!(text.contains("demo"), "{v}");
    assert!(text.contains("run cargo test export"), "{v}");
    assert_eq!(la["run_live"], true, "{v}");
    assert_eq!(la["working"], false, "{v}");
}

// TC-1218: per_turn_run_rows_and_session_graphs_in_headless_chrome (FR-1222,
// FR-1223, FR-1224). The page polls a mocked `/api/state` built from the
// served snapshot: conversation A has two runs, B has none.
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn per_turn_run_rows_and_session_graphs_in_headless_chrome() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let workspace = workspace_with_run(DONE_RUN);
    let home = tempfile::tempdir().expect("temp home");
    let (_guard, url) = start_serve(workspace.path(), home.path());
    let script = r##"
import sys, json, copy
from playwright.sync_api import sync_playwright

ROWS = """() => {
  const t = document.querySelector('.session-transcript');
  const kids = [...t.children].map(li => li.getAttribute('data-msg-id') || (li.classList.contains('chat-aux') ? 'row:' + (li.getAttribute('data-run-id') || li.getAttribute('data-aux-key')) : li.className));
  const rows = [...t.querySelectorAll(':scope > li.chat-aux-run')].map(li => ({
    run: li.getAttribute('data-run-id'),
    text: li.querySelector('.chat-aux-head').innerText,
    expanded: li.querySelector('.chat-aux-head').getAttribute('aria-expanded'),
    open_btn: !!li.querySelector('.chat-aux-open-run')
  }));
  return {order: kids, rows: rows,
    graph_hidden: document.querySelector('#graph').hidden,
    graph_id: document.querySelector('#graph-id') ? document.querySelector('#graph-id').textContent : null,
    graph_text: document.querySelector('#graph').innerText,
    detail_visible: !document.querySelector('#graph-detail').hidden,
    detail_text: document.querySelector('#graph-detail').innerText};
}"""

def session(cid, transcript):
    return {"conversation_id": cid, "conversation_title": cid, "selected_agent": "demo", "details_expanded": False,
        "agents": [{"id": "demo", "label": "demo", "allowed": True, "busy": False, "modes": ["ask"], "mode": "ask",
                    "models": [], "model": None, "effort": None, "models_loading": False}],
        "conversations": [], "transcript_omitted": 0, "transcript": transcript}

def band(run_id, turn, prompt, msg, steps):
    return {"run_id": run_id, "turn_index": turn, "phase": "completed", "user_prompt": prompt,
            "nodes": [s["id"] for s in steps], "message_id": msg, "steps": steps}

out = {"commands": []}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    probe = b.new_page()
    probe.goto(sys.argv[2])
    probe.wait_for_function("() => document.querySelector('#graph-phase').textContent === 'completed'", timeout=15000)
    from urllib.parse import urlparse, parse_qs
    tok = parse_qs(urlparse(sys.argv[2]).query)["token"][0]
    real = probe.evaluate("""async (t) => {
        const r = await fetch('/api/state', {headers: {Authorization: 'Bearer ' + t}});
        return await r.json(); }""", tok)
    probe.close()

    OLD_STEPS = [{"id": "a", "role": "plan", "runner": "cursor", "status": "completed", "started_at_ms": 1000, "finished_at_ms": 31000}]
    NEW_STEPS = [{"id": "a", "role": "plan", "runner": "cursor", "status": "completed", "started_at_ms": 1000, "finished_at_ms": 61000},
                 {"id": "b", "role": "implement", "runner": "cursor", "status": "completed", "started_at_ms": 61000, "finished_at_ms": 121000}]
    A_TRANSCRIPT = [
        {"id": "c1:0", "role": "user", "agent": None, "content": "Add CSV export", "streaming": False},
        {"id": "c1:1", "role": "agent", "agent": "demo", "content": "Planned the export.", "streaming": False},
        {"id": "c1:2", "role": "user", "agent": None, "content": "Now write it", "streaming": False},
        {"id": "c1:3", "role": "agent", "agent": "demo", "content": "Wrote the export.", "streaming": False},
    ]
    a_snap = copy.deepcopy(real)
    a_snap["source"] = "tui"
    a_snap["session"] = session("c1", A_TRANSCRIPT)
    a_snap["graph"]["user_prompt"] = "Now write it"
    a_snap["graph"]["turns"] = [band("run-old", 1, "Add CSV export", "c1:0", OLD_STEPS),
                                band("run-done", 2, "Now write it", "c1:2", NEW_STEPS)]
    old_snap = copy.deepcopy(a_snap)
    old_snap["graph"]["id"] = "run-old"
    old_snap["graph"]["user_prompt"] = "Add CSV export"
    old_snap["graph"]["nodes"][0]["detail"] = {}
    b_snap = copy.deepcopy(real)
    b_snap["source"] = "tui"
    b_snap["graph"] = None
    b_snap["session"] = session("c2", [
        {"id": "c2:0", "role": "user", "agent": None, "content": "Unrelated question", "streaming": False},
        {"id": "c2:1", "role": "agent", "agent": "demo", "content": "An answer.", "streaming": False}])
    state = {"snap": a_snap, "rev": 100}

    def fulfill_state(route):
        state["rev"] += 1
        snap = copy.deepcopy(state["snap"])
        snap["revision"] = state["rev"]
        route.fulfill(status=200, content_type="application/json", body=json.dumps(snap))

    def fulfill_command(route):
        body = json.loads(route.request.post_data or "{}")
        out["commands"].append(body)
        if body.get("command") == "show_graph_run" and body.get("run_id") == "run-old":
            state["snap"] = old_snap
        route.fulfill(status=200, content_type="application/json", body=json.dumps({"session": state["snap"]["session"]}))

    ctx = b.new_context(viewport={"width": 1440, "height": 900}, bypass_csp=True)
    ctx.add_init_script("delete window.EventSource;")
    pg = ctx.new_page()
    pg.on("pageerror", lambda e: print("PAGEERROR", e, file=sys.stderr))
    pg.route("**/api/state*", fulfill_state)
    pg.route("**/api/command*", fulfill_command)
    pg.goto(sys.argv[2])
    pg.wait_for_selector("li.chat-aux-run[data-run-id='run-old']", timeout=15000)
    pg.wait_for_selector("li.chat-aux-run[data-run-id='run-done']", timeout=15000)
    out["a"] = pg.evaluate(ROWS)
    # Conversation B: no graph and no rows from A.
    state["snap"] = b_snap
    pg.wait_for_function("() => !document.querySelector('li.chat-aux-run') && document.querySelector('li.msg[data-msg-id=\"c2:0\"]')", timeout=15000)
    pg.click("#graph-toggle")
    pg.wait_for_timeout(300)
    out["b"] = pg.evaluate(ROWS)
    pg.keyboard.press("Escape")
    # Back to A: the graph and both rows are back.
    state["snap"] = a_snap
    pg.wait_for_selector("li.chat-aux-run[data-run-id='run-old']", timeout=15000)
    out["a_again"] = pg.evaluate(ROWS)
    # The older run's step loads that run into the Graph view.
    pg.click("li.chat-aux-run[data-run-id='run-old'] .chat-aux-head")
    pg.click("li.chat-aux-run[data-run-id='run-old'] .chat-aux-step-btn")
    pg.wait_for_function("() => !document.querySelector('#graph').hidden && !document.querySelector('#graph-detail').hidden", timeout=15000)
    out["old_open"] = pg.evaluate(ROWS)
    print(json.dumps(out))
    b.close()
"##;
    let v = run_probe(python, &chrome, &url, script);

    // FR-1223: each exchange with a run gets its own collapsed row, right
    // after that exchange's reply.
    let a = &v["a"];
    let order: Vec<String> = a["order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap_or_default().to_string())
        .collect();
    let at = |k: &str| {
        order
            .iter()
            .position(|x| x == k)
            .unwrap_or_else(|| panic!("{k}: {v}"))
    };
    assert_eq!(at("row:run-old"), at("c1:1") + 1, "{v}");
    assert_eq!(at("row:run-done"), at("c1:3") + 1, "{v}");
    let rows = a["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{v}");
    for row in rows {
        assert_eq!(row["expanded"], "false", "{v}");
        assert_eq!(row["open_btn"], true, "{v}");
    }
    let old = rows.iter().find(|r| r["run"] == "run-old").unwrap();
    assert!(
        old["text"]
            .as_str()
            .unwrap()
            .contains("Ran \u{b7} 1/1 steps \u{b7} 30s"),
        "{v}"
    );
    let new = rows.iter().find(|r| r["run"] == "run-done").unwrap();
    assert!(new["text"].as_str().unwrap().contains("2/2 steps"), "{v}");
    // FR-1222 (client side): B shows no run rows and no graph of A.
    let b = &v["b"];
    assert_eq!(b["rows"].as_array().unwrap().len(), 0, "{v}");
    assert!(
        !b["graph_text"].as_str().unwrap().contains("run-done"),
        "{v}"
    );
    assert!(
        !b["order"].as_array().unwrap().iter().any(|x| x == "c1:0"),
        "{v}"
    );
    // Back on A both rows return.
    assert_eq!(v["a_again"]["rows"].as_array().unwrap().len(), 2, "{v}");
    // FR-1224: the older step sent show_graph_run and focused that run's node.
    let cmds = v["commands"].as_array().unwrap();
    assert!(
        cmds.iter()
            .any(|c| c["command"] == "show_graph_run" && c["run_id"] == "run-old"),
        "{v}"
    );
    let open = &v["old_open"];
    assert_eq!(open["graph_hidden"], false, "{v}");
    assert_eq!(open["detail_visible"], true, "{v}");
    assert!(
        open["graph_text"].as_str().unwrap().contains("run-old"),
        "{v}"
    );
}

// TC-1224: sessions_grouped_by_workspace_in_headless_chrome
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn sessions_grouped_by_workspace_in_headless_chrome() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let workspace = workspace_with_run(DONE_RUN);
    let home = tempfile::tempdir().expect("temp home");
    let (_guard, url) = start_serve(workspace.path(), home.path());
    let script = r##"
import sys, json, copy, time
from playwright.sync_api import sync_playwright

GROUPS = """() => [...document.querySelectorAll('#sessions-list > li.sessions-group')].map(g => ({
  ws: g.getAttribute('data-workspace'),
  current: g.classList.contains('sessions-group-current'),
  collapsed: g.classList.contains('sessions-group-collapsed'),
  icon: !!g.querySelector('.sessions-group-head svg.sessions-group-icon'),
  name: (g.querySelector('.sessions-group-name') || {}).textContent || '',
  name_weight: getComputedStyle(g.querySelector('.sessions-group-name')).fontWeight,
  label: (g.querySelector('.sessions-group-label') || {}).textContent || '',
  tip: g.querySelector('.sessions-group-head').title,
  more: (g.querySelector('.sessions-more-btn') || {}).textContent || null,
  rows: [...g.querySelectorAll('.sessions-row')].map(r => ({
    id: r.getAttribute('data-id'),
    title: r.querySelector('.sessions-row-title').textContent,
    time: r.querySelector('.sessions-row-time').textContent,
    active: r.getAttribute('aria-current') === 'true',
    disabled: r.getAttribute('aria-disabled') === 'true',
    tip: r.title,
    ellipsis: getComputedStyle(r.querySelector('.sessions-row-title')).textOverflow,
    time_right: r.querySelector('.sessions-row-time').getBoundingClientRect().right > r.querySelector('.sessions-row-title').getBoundingClientRect().right
  }))
}))"""

now = int(time.time() * 1000)
M = 60000; H = 60 * M; D = 24 * H
def conv(cid, title, age):
    return {"id": cid, "title": title, "updated_at_ms": now - age, "last_agent": "demo", "turn_count": 1, "running": False}

out = {"commands": [], "switch": []}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    probe = b.new_page()
    probe.goto(sys.argv[2])
    probe.wait_for_function("() => document.querySelector('#graph-phase').textContent === 'completed'", timeout=15000)
    from urllib.parse import urlparse, parse_qs
    tok = parse_qs(urlparse(sys.argv[2]).query)["token"][0]
    real = probe.evaluate("""async (t) => {
        const r = await fetch('/api/state', {headers: {Authorization: 'Bearer ' + t}});
        return await r.json(); }""", tok)
    probe.close()

    snap = copy.deepcopy(real)
    snap["source"] = "tui"
    snap["graph"] = None
    snap["session"] = {"conversation_id": "cur-2", "conversation_title": "Current two", "selected_agent": "demo",
        "details_expanded": False, "agents": [], "transcript": [], "transcript_omitted": 0, "commands": [],
        "conversations": [conv("cur-2", "Current two", 2 * M), conv("cur-1", "Current one with a very long title that must be ellipsized in the sidebar", 3 * D)],
        "workspaces": [
            {"path": "/home/u/src/dmux", "name": "dmux", "label": "~/src", "current": True, "available": True, "updated_at_ms": now - 2 * M, "conversations": []},
            {"path": "/home/u/old/gone", "name": "gone", "label": "~/old", "current": False, "available": False, "updated_at_ms": now - 9 * D,
             "conversations": [conv("gone-1", "Gone folder chat", 9 * D)]},
            {"path": "/home/u/src/api", "name": "api", "label": "~/src", "current": False, "available": True, "updated_at_ms": now - 5 * H,
             "conversations": [conv("api-%d" % i, "Api chat %d" % i, age) for i, age in enumerate([5 * H, 6 * H, 2 * D, 4 * D, 8 * D])]},
        ]}
    state = {"snap": snap, "rev": 100, "current": "/home/u/src/dmux"}

    def fulfill_state(route):
        state["rev"] += 1
        s = copy.deepcopy(state["snap"])
        s["revision"] = state["rev"]
        route.fulfill(status=200, content_type="application/json", body=json.dumps(s))

    def fulfill_command(route):
        body = json.loads(route.request.post_data or "{}")
        out["commands"].append(body)
        route.fulfill(status=200, content_type="application/json", body=json.dumps({"session": state["snap"]["session"]}))

    def fulfill_workspace(route):
        req = route.request
        if req.method == "POST" and req.url.split("?")[0].endswith("/api/workspace/switch"):
            body = json.loads(req.post_data or "{}")
            out["switch"].append(body)
            state["current"] = body.get("path")
            route.fulfill(status=202, content_type="application/json", body=json.dumps({"path": body.get("path")}))
            return
        if req.url.split("?")[0].endswith("/api/workspace"):
            route.fulfill(status=200, content_type="application/json", body=json.dumps(
                {"current": state["current"], "recent": [], "browse_roots": ["/home/u"], "can_switch": True}))
            return
        route.continue_()

    ctx = b.new_context(viewport={"width": 1440, "height": 900}, bypass_csp=True)
    ctx.add_init_script("delete window.EventSource;")
    pg = ctx.new_page()
    pg.on("pageerror", lambda e: print("PAGEERROR", e, file=sys.stderr))
    pg.route("**/api/state*", fulfill_state)
    pg.route("**/api/command*", fulfill_command)
    pg.route("**/api/workspace*", fulfill_workspace)
    pg.route("**/api/workspace/**", fulfill_workspace)
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#sessions-list li.sessions-group", timeout=15000)
    if not pg.evaluate("() => document.querySelector('#sessions-sidebar').classList.contains('open')"):
        pg.click("#sessions-toggle")
    pg.wait_for_function("() => [...document.querySelectorAll('#sessions-list .sessions-row')].some(r => r.getAttribute('aria-disabled') !== 'true' && r.getAttribute('data-id') === 'api-0' && !r.title.includes('not available'))", timeout=15000)
    out["initial"] = pg.evaluate(GROUPS)
    pg.click("li.sessions-group[data-workspace='/home/u/src/api'] .sessions-more-btn")
    out["more"] = pg.evaluate(GROUPS)
    pg.click("li.sessions-group[data-workspace='/home/u/src/api'] .sessions-more-btn")
    out["less"] = pg.evaluate(GROUPS)
    pg.fill("#sessions-filter", "api chat 4")
    pg.wait_for_timeout(200)
    out["filtered"] = pg.evaluate(GROUPS)
    pg.fill("#sessions-filter", "nothing matches this")
    pg.wait_for_timeout(200)
    out["no_match"] = pg.evaluate("() => document.querySelector('#sessions-list').innerText")
    pg.fill("#sessions-filter", "")
    pg.wait_for_timeout(200)
    pg.click("li.sessions-group[data-workspace='/home/u/src/api'] .sessions-group-head")
    pg.reload()
    pg.wait_for_selector("#sessions-list li.sessions-group", timeout=15000)
    pg.wait_for_timeout(500)
    out["after_reload"] = pg.evaluate(GROUPS)
    pg.click("li.sessions-group[data-workspace='/home/u/src/api'] .sessions-group-head")
    pg.wait_for_function("() => [...document.querySelectorAll('#sessions-list .sessions-row')].some(r => r.getAttribute('data-id') === 'api-0' && r.getAttribute('aria-disabled') !== 'true')", timeout=15000)
    # Disabled row (folder gone) does nothing; another workspace's row switches.
    pg.click("li.sessions-row[data-id='gone-1']")
    pg.click("li.sessions-row[data-id='api-1']")
    pg.wait_for_function("() => true", timeout=1000)
    deadline = time.time() + 10
    while time.time() < deadline and not out["switch"]:
        pg.wait_for_timeout(100)
    print(json.dumps(out))
    b.close()
"##;
    let v = run_probe(python, &chrome, &url, script);

    let initial = v["initial"].as_array().unwrap();
    // FR-1225: current workspace first and highlighted, then by activity.
    let order: Vec<&str> = initial.iter().map(|g| g["ws"].as_str().unwrap()).collect();
    assert_eq!(
        order,
        ["/home/u/src/dmux", "/home/u/src/api", "/home/u/old/gone"],
        "{v}"
    );
    assert_eq!(initial[0]["current"], true, "{v}");
    for g in initial {
        assert_eq!(g["icon"], true, "{v}");
        assert!(
            g["name_weight"].as_str().unwrap().parse::<u32>().unwrap() >= 600,
            "{v}"
        );
        assert_eq!(g["tip"], g["ws"], "{v}");
    }
    assert_eq!(initial[1]["name"], "api", "{v}");
    assert_eq!(initial[1]["label"], "~/src", "{v}");
    // FR-1226: newest first, compact time on the right, ellipsized titles,
    // the active session highlighted, 3 rows + More.
    let cur = initial[0]["rows"].as_array().unwrap();
    assert_eq!(cur[0]["id"], "cur-2", "{v}");
    assert_eq!(cur[0]["active"], true, "{v}");
    assert_eq!(cur[0]["time"], "2m", "{v}");
    assert_eq!(cur[1]["time"], "3d", "{v}");
    assert_eq!(cur[1]["ellipsis"], "ellipsis", "{v}");
    assert_eq!(cur[1]["time_right"], true, "{v}");
    let api = initial[1]["rows"].as_array().unwrap();
    let ids: Vec<&str> = api.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["api-0", "api-1", "api-2"], "{v}");
    assert_eq!(api[0]["time"], "5h", "{v}");
    assert_eq!(initial[1]["more"], "More", "{v}");
    assert_eq!(v["more"][1]["rows"].as_array().unwrap().len(), 5, "{v}");
    assert_eq!(v["more"][1]["more"], "Less", "{v}");
    assert_eq!(v["less"][1]["rows"].as_array().unwrap().len(), 3, "{v}");
    // FR-1227: a missing folder's row is disabled with a tooltip.
    let gone = &initial[2]["rows"][0];
    assert_eq!(gone["disabled"], true, "{v}");
    assert!(
        gone["tip"].as_str().unwrap().contains("no longer exists"),
        "{v}"
    );
    // Filter across groups hides empty groups.
    let filtered = v["filtered"].as_array().unwrap();
    assert_eq!(filtered.len(), 1, "{v}");
    assert_eq!(filtered[0]["ws"], "/home/u/src/api", "{v}");
    assert_eq!(filtered[0]["rows"][0]["id"], "api-4", "{v}");
    assert!(
        v["no_match"]
            .as_str()
            .unwrap()
            .contains("No matching sessions."),
        "{v}"
    );
    // Collapse state survives a reload in the same tab.
    let api_after = v["after_reload"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["ws"] == "/home/u/src/api")
        .unwrap()
        .clone();
    assert_eq!(api_after["collapsed"], true, "{v}");
    // Opening another workspace's session prepares it, then switches.
    let cmds = v["commands"].as_array().unwrap();
    assert!(
        cmds.iter()
            .any(|c| c["command"] == "prepare_open_conversation"
                && c["workspace"] == "/home/u/src/api"
                && c["id"] == "api-1"),
        "{v}"
    );
    assert!(!cmds.iter().any(|c| c["id"] == "gone-1"), "{v}");
    assert_eq!(v["switch"][0]["path"], "/home/u/src/api", "{v}");
}

// Top bar Chat button: returns to the chat from the Graph/Diff views and the
// Memory popover, keeping the transcript scroll position and the draft.
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn chat_button_returns_from_views_in_headless_chrome() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let workspace = workspace_with_run(DONE_RUN);
    let home = tempfile::tempdir().expect("temp home");
    let (_guard, url) = start_serve(workspace.path(), home.path());
    let script = r##"
import sys, json
from playwright.sync_api import sync_playwright

STATE = """() => {
  const q = s => document.querySelector(s);
  const pressed = s => q(s).getAttribute('aria-pressed');
  const t = q('#session .session-transcript');
  const input = q('#session textarea.session-input');
  const a = document.activeElement;
  return {
    chat: pressed('#chat-toggle'), chat_active: q('#chat-toggle').classList.contains('active'),
    graph: pressed('#graph-toggle'), diff: pressed('#diff-toggle'), memory: pressed('#memory-toggle'),
    skills: pressed('#skills-toggle'), skills_hidden: q('#skills-popover').hidden,
    graph_hidden: q('#graph').hidden, docs_hidden: q('#docs').hidden, memory_hidden: q('#memory-popover').hidden,
    chat_visible: !q('#session').hidden && q('#session').getClientRects().length > 0,
    draft: input ? input.value : null, scroll: t ? t.scrollTop : null,
    scroll_max: t ? t.scrollHeight - t.clientHeight : null,
    open_links: document.querySelectorAll('#session .chat-aux-open-run').length,
    focus: a ? (a.id || a.className) : null
  };
}"""

def transcript(n):
    out = []
    for i in range(n):
        out.append({"id": "c:%d" % (2 * i), "role": "user", "agent": None, "content": "question %d" % i, "streaming": False})
        out.append({"id": "c:%d" % (2 * i + 1), "role": "agent", "agent": "demo",
                    "content": "answer %d\\n\\nline two\\n\\nline three" % i, "streaming": False})
    return out

out = {}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1440, "height": 900}, bypass_csp=True)
    pg.on("pageerror", lambda e: print("PAGEERROR", e, file=sys.stderr))
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#session:not([hidden]) textarea.session-input", timeout=15000)
    pg.wait_for_function("() => document.querySelector('#graph-id').textContent.length > 0", timeout=15000)
    out["initial"] = pg.evaluate(STATE)
    pg.evaluate("""(msgs) => {
      const t = document.querySelector('#session .session-transcript');
      for (const m of msgs) {
        const li = document.createElement('li');
        li.className = 'msg msg-filler';
        li.style.minHeight = '60px';
        li.textContent = m.content;
        t.insertBefore(li, t.firstChild);
        t.appendChild(li.cloneNode(true));
      }
    }""", transcript(30))
    # `dmuxai serve` is read-only, so the composer is disabled; set the
    # draft directly (the Chat button must not touch it either way).
    pg.evaluate("() => { document.querySelector('#session textarea.session-input').value = 'DRAFT-KEEP'; }")
    # Scroll so the "Open in Graph" link sits mid-transcript (not at the end),
    # so clicking it needs no further scrolling.
    pg.evaluate("() => { document.querySelector('#session .chat-aux-open-run').scrollIntoView({block: 'center'}); }")
    pg.wait_for_timeout(300)
    out["before"] = pg.evaluate(STATE)
    links = pg.query_selector_all("#session .chat-aux-open-run")
    if links:
        links[0].click()
    else:
        pg.evaluate("() => window.dmuxaiFocusGraphNode('a')")
    pg.wait_for_timeout(600)
    out["graph_open"] = pg.evaluate(STATE)
    pg.click("#chat-toggle")
    pg.wait_for_timeout(200)
    out["back"] = pg.evaluate(STATE)
    pg.click("#graph-toggle")
    out["graph_again"] = pg.evaluate(STATE)
    pg.click("#graph-toggle")
    out["graph_toggled_off"] = pg.evaluate(STATE)
    pg.click("#memory-toggle")
    out["memory_open"] = pg.evaluate(STATE)
    pg.click("#chat-toggle")
    out["memory_back"] = pg.evaluate(STATE)
    pg.click("#skills-toggle")
    out["skills_open"] = pg.evaluate(STATE)
    pg.click("#chat-toggle")
    out["skills_back"] = pg.evaluate(STATE)
    pg.click("#diff-toggle")
    pg.focus("#chat-toggle")
    pg.keyboard.press("Enter")
    out["diff_back_keyboard"] = pg.evaluate(STATE)
    pg.focus("#graph-toggle")
    pg.keyboard.press("Enter")
    pg.keyboard.press("Escape")
    out["graph_esc"] = pg.evaluate(STATE)
    print(json.dumps(out))
    b.close()
"##;
    let v = run_probe(python, &chrome, &url, script);

    let init = &v["initial"];
    assert_eq!(init["chat"], "true", "{v}");
    assert_eq!(init["chat_active"], true, "{v}");
    assert_eq!(init["graph"], "false", "{v}");
    assert_eq!(init["memory"], "false", "{v}");
    let before = &v["before"];
    let scroll = before["scroll"].as_f64().expect("transcript scrolls");
    let scroll_max = before["scroll_max"].as_f64().unwrap();
    assert!(
        scroll > 100.0 && scroll < scroll_max - 100.0,
        "transcript not scrolled to the middle: {v}"
    );
    assert_eq!(before["draft"], "DRAFT-KEEP", "{v}");
    assert!(
        before["open_links"].as_u64().unwrap() >= 1,
        "no Open in Graph link: {v}"
    );

    // "Open in Graph" opens the view; Chat is no longer the pressed tab.
    let open = &v["graph_open"];
    assert_eq!(open["graph_hidden"], false, "{v}");
    assert_eq!(open["graph"], "true", "{v}");
    assert_eq!(open["chat"], "false", "{v}");
    assert_eq!(open["chat_active"], false, "{v}");

    // Chat closes the view and keeps the scroll spot and the draft.
    let back = &v["back"];
    assert_eq!(back["graph_hidden"], true, "{v}");
    assert_eq!(back["graph"], "false", "{v}");
    assert_eq!(back["chat"], "true", "{v}");
    assert_eq!(back["chat_visible"], true, "{v}");
    assert_eq!(back["draft"], "DRAFT-KEEP", "{v}");
    assert_eq!(back["scroll"].as_f64().unwrap(), scroll, "{v}");
    // The read-only composer is disabled, so focus lands on the Chat button.
    assert_eq!(back["focus"], "chat-toggle", "{v}");

    // Graph still toggles from its own button.
    assert_eq!(v["graph_again"]["graph"], "true", "{v}");
    assert_eq!(v["graph_again"]["chat"], "false", "{v}");
    assert_eq!(v["graph_toggled_off"]["graph_hidden"], true, "{v}");
    assert_eq!(v["graph_toggled_off"]["chat"], "true", "{v}");

    // Memory popover: pressed while open; Chat closes it.
    assert_eq!(v["memory_open"]["memory_hidden"], false, "{v}");
    assert_eq!(v["memory_open"]["memory"], "true", "{v}");
    assert_eq!(v["memory_open"]["chat"], "false", "{v}");
    assert_eq!(v["memory_back"]["memory_hidden"], true, "{v}");
    assert_eq!(v["memory_back"]["memory"], "false", "{v}");
    assert_eq!(v["memory_back"]["chat"], "true", "{v}");
    assert_eq!(v["memory_back"]["draft"], "DRAFT-KEEP", "{v}");

    // Skills popover behaves the same way.
    assert_eq!(v["skills_open"]["skills_hidden"], false, "{v}");
    assert_eq!(v["skills_open"]["skills"], "true", "{v}");
    assert_eq!(v["skills_open"]["chat"], "false", "{v}");
    assert_eq!(v["skills_back"]["skills_hidden"], true, "{v}");
    assert_eq!(v["skills_back"]["skills"], "false", "{v}");
    assert_eq!(v["skills_back"]["chat"], "true", "{v}");

    // Keyboard: Enter on Chat closes Diff; Esc closes Graph.
    assert_eq!(v["diff_back_keyboard"]["docs_hidden"], true, "{v}");
    assert_eq!(v["diff_back_keyboard"]["chat"], "true", "{v}");
    assert_eq!(v["graph_esc"]["graph_hidden"], true, "{v}");
    assert_eq!(v["graph_esc"]["chat"], "true", "{v}");
    assert_eq!(v["graph_esc"]["scroll"].as_f64().unwrap(), scroll, "{v}");
}
