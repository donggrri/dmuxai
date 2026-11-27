//! TC-2964 (SPEC-web-local-access FR-2964): tokenless loopback dashboard works in headless Chrome.

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
    for candidate in [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in ["google-chrome", "google-chrome-stable", "chromium"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn prepare() -> Option<(PathBuf, &'static str)> {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome found");
        return None;
    };
    let python = "python3";
    let probe = Command::new(python)
        .args(["-c", "import playwright"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.map(|s| s.success()).unwrap_or(false) {
        println!("SKIP: python playwright not installed");
        return None;
    }
    Some((chrome, python))
}

fn start_web(workspace: &Path, state: &Path) -> (ChildGuard, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["--web", "-C"])
        .arg(workspace)
        .args(["--port", "0"])
        .env("DMUXAI_STATE_DIR", state)
        .env("XDG_STATE_HOME", state)
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WEB_REQUIRE_TOKEN")
        .env_remove("DMUXAI_WEB_TOKEN_REQUIRED")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn dmuxai --web");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line.trim_start().starts_with("dmuxai dashboard: http") {
                let _ = tx.send(line.trim_end().to_owned());
                return;
            }
            line.clear();
        }
        let _ = tx.send(String::new());
    });
    let line = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("dashboard URL");
    let url = line
        .strip_prefix("dmuxai dashboard: ")
        .unwrap_or(&line)
        .trim_end_matches('/')
        .to_string();
    (ChildGuard(Some(child)), url)
}

fn run_probe(python: &str, chrome: &Path, url: &str) -> serde_json::Value {
    let script = r#"
import json, sys
from playwright.sync_api import sync_playwright
chrome, url = sys.argv[1], sys.argv[2]
with sync_playwright() as p:
    browser = p.chromium.launch(executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page()
    page.goto(url, wait_until="networkidle", timeout=60000)
    resp = page.evaluate("""async () => {
    const r = await fetch('/api/state', {headers: {'X-Dmuxai-Client': '1'}});
    return {status: r.status, body: await r.text()};
  }""")
    text = page.inner_text("body")
    browser.close()
    print(json.dumps({"api_status": resp["status"], "api_body": resp["body"], "page_text": text}))
"#;
    let out = Command::new(python)
        .args(["-c", script])
        .arg(chrome)
        .arg(url)
        .output()
        .expect("playwright");
    let text = String::from_utf8_lossy(&out.stdout);
    let json_line = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| {
            panic!(
                "no JSON from playwright:\n{text}\nstderr:{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
    serde_json::from_str(json_line).expect("json")
}

#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_2964_tokenless_dashboard_api_state_ok_in_headless_chrome() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let (_guard, url) = start_web(workspace.path(), state.path());
    let result = run_probe(python, &chrome, &format!("{url}/"));
    assert_eq!(
        result.get("api_status").and_then(|v| v.as_u64()),
        Some(200),
        "{result}"
    );
    let page_text = result
        .get("page_text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(!page_text.contains("missing access token"));
    assert!(!page_text.contains("need token"));
}
