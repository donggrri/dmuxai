//! TC-2410 (SPEC-jira-todos UX-2409..UX-2411): the dashboard lists Jira
//! tickets from a fake Jira server, opens the configured browse URL, and copies
//! a ticket into the local to-do list. Needs Chrome/Chromium and python
//! Playwright, so it is `#[ignore]`d; run with
//! `cargo test --test web_jira_todos -- --ignored`.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
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

const JIRA_BODY: &str = r#"{"issues":[
{"key":"DEMO-1","fields":{"summary":"Demo summary one","status":{"name":"In Progress","statusCategory":{"key":"indeterminate"}},"priority":{"name":"High"},"issuetype":{"name":"Task"},"updated":"2026-01-01T00:00:00.000+0000"}},
{"key":"DEMO-2","fields":{"summary":"Demo summary two","status":{"name":"To Do","statusCategory":{"key":"new"}},"priority":{"name":"Medium"},"issuetype":{"name":"Bug"},"updated":"2026-01-02T00:00:00.000+0000"}}
]}"#;

fn spawn_fake_jira() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake Jira");
    let addr = listener.local_addr().expect("fake Jira address");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Ok(clone) = stream.try_clone() else {
                continue;
            };
            let mut reader = BufReader::new(clone);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                continue;
            }
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
            }
            if let Some(target) = request_line.split_whitespace().nth(1) {
                recorded.lock().unwrap().push(target.to_owned());
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                JIRA_BODY.len(),
                JIRA_BODY
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{}", addr.port()), calls)
}

// TC-2410 / UX-2409..UX-2411
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn tc_2410_jira_group_renders_and_copies() {
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

    let (jira_base, calls) = spawn_fake_jira();
    let workspace = tempfile::tempdir().expect("temp workspace");
    let home = tempfile::tempdir().expect("temp home");
    let config_home = tempfile::tempdir().expect("temp config");
    let config_dir = config_home.path().join("dmuxai");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("dmuxai.toml"),
        format!("[jira]\nbase_url = \"{jira_base}\"\nemail = \"user@example.com\"\n"),
    )
    .expect("write user dmuxai.toml");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", config_home.path())
        .env_remove("DMUXAI_CONFIG")
        .env_remove("DMUXAI_WORKSPACE")
        .env("DMUXAI_JIRA_TOKEN", "demo-token-XYZ")
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
import sys, json
from playwright.sync_api import sync_playwright
out = {}
base = sys.argv[3]
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    # bypass_csp only lets Playwright run its own wait predicates; the page CSP is unchanged.
    pg = b.new_page(viewport={"width": 1400, "height": 900}, bypass_csp=True)
    pg.goto(sys.argv[2])
    pg.click("#todos-toggle")
    pg.wait_for_selector("#sidebar.open #tab-panel-todos:not([hidden])", timeout=15000)
    pg.wait_for_function("document.querySelectorAll('#jira-list .jira-item').length === 2", timeout=15000)
    out["items"] = pg.eval_on_selector_all(
        "#jira-list .jira-item",
        "els => els.map(e => ({key: e.querySelector('.jira-key').textContent, summary: e.querySelector('.jira-summary').textContent, status: e.querySelector('.jira-status').textContent, category: e.querySelector('.jira-status').getAttribute('data-category')}))",
    )
    out["start_disabled"] = pg.eval_on_selector("#jira-list .jira-item .jira-start", "e => e.disabled")
    pg.evaluate("window.__opened = null; window.open = function (url, target, features) { window.__opened = { url: url, target: target, features: features }; return null; };")
    pg.locator("#jira-list .jira-item").first.locator("button", has_text="Open in Jira").click()
    out["opened"] = pg.evaluate("window.__opened")
    pg.locator("#jira-list .jira-item").first.locator("button", has_text="Copy to to-do").click()
    pg.wait_for_function("""async () => {
      const token = sessionStorage.getItem('dmuxai.token');
      const res = await fetch('/api/todos', { headers: { Authorization: 'Bearer ' + token } });
      const todos = await res.json();
      return Array.isArray(todos) && todos.some(t => typeof t.title === 'string' && t.title.indexOf('DEMO-1') === 0);
    }""", timeout=15000)
    out["todos"] = pg.evaluate("""async () => {
      const token = sessionStorage.getItem('dmuxai.token');
      const res = await fetch('/api/todos', { headers: { Authorization: 'Bearer ' + token } });
      return await res.json();
    }""")
    out["html"] = pg.content()
    b.close()
print(json.dumps(out))
"##;
    let mut py_child = Command::new(python)
        .args(["-c", script])
        .arg(&chrome)
        .arg(&url)
        .arg(&jira_base)
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
    let out_bytes = match rx_out.recv_timeout(Duration::from_secs(60)) {
        Ok(b) => {
            let _ = py_child.wait();
            b
        }
        Err(_) => {
            let _ = py_child.kill();
            let _ = py_child.wait();
            panic!("playwright execution timed out after 60s");
        }
    };
    let text = String::from_utf8_lossy(&out_bytes);
    let json_line = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON result from playwright:\n{text}"));
    let v: serde_json::Value = serde_json::from_str(json_line).expect("json result");

    let items: Vec<serde_json::Value> = serde_json::from_value(v["items"].clone()).expect("items");
    assert_eq!(items.len(), 2, "{v}");
    assert_eq!(items[0]["key"], "DEMO-1", "{v}");
    assert_eq!(items[0]["summary"], "Demo summary one", "{v}");
    assert_eq!(items[0]["status"], "In Progress", "{v}");
    assert_eq!(items[0]["category"], "indeterminate", "{v}");
    assert_eq!(items[1]["key"], "DEMO-2", "{v}");
    assert_eq!(items[1]["summary"], "Demo summary two", "{v}");
    assert_eq!(items[1]["status"], "To Do", "{v}");
    assert_eq!(items[1]["category"], "new", "{v}");
    assert_eq!(v["start_disabled"], true, "{v}");
    assert_eq!(
        v["opened"]["url"],
        format!("{jira_base}/browse/DEMO-1"),
        "{v}"
    );
    assert_eq!(v["opened"]["target"], "_blank", "{v}");
    assert!(
        v["opened"]["features"]
            .as_str()
            .unwrap_or_default()
            .contains("noopener,noreferrer"),
        "{v}"
    );
    let todos = v["todos"].as_array().expect("todos array");
    let copied = todos
        .iter()
        .find(|todo| {
            todo["title"]
                .as_str()
                .is_some_and(|title| title.starts_with("DEMO-1"))
        })
        .unwrap_or_else(|| panic!("copy did not create a DEMO-1 to-do: {v}"));
    assert_eq!(copied["external_key"], "DEMO-1", "{v}");
    let html = v["html"].as_str().unwrap_or_default();
    assert!(!html.contains("demo-token-XYZ"), "token leaked into HTML");
    let calls = calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .any(|path| path.starts_with("/rest/api/3/search/jql")),
        "fake Jira was not queried: {calls:?}"
    );
}
