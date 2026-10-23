//! PR-2.5 part 4: per-agent system prompts dashboard panel. Needs Chrome/Chromium
//! and python Playwright, so it is `#[ignore]`d.

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

// TC-1720 (FR-1706): panel renders
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn agent_prompts_panel_renders_in_headless_chrome() {
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
    let workspace = tempfile::tempdir().expect("temp workspace");
    let home = tempfile::tempdir().expect("temp home");

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
import sys, json
from playwright.sync_api import sync_playwright
out = {}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1400, "height": 900}, bypass_csp=True)
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#agent-prompts-toggle", timeout=15000)
    pg.click("#agent-prompts-toggle")
    pg.wait_for_selector("#agent-prompts-popover:not([hidden])", timeout=15000)
    pg.wait_for_function("document.querySelectorAll('.agent-prompt-row').length >= 1", timeout=15000)
    out["row_count"] = pg.eval_on_selector_all(".agent-prompt-row", "els => els.length")
    # save a prompt through the first row's editor
    first = pg.locator(".agent-prompt-row").first
    first.locator("textarea").fill("hello system prompt")
    first.locator("button:has-text('Save')").click()
    pg.wait_for_function("document.querySelector('.agent-prompt-row').getAttribute('data-set') === 'true'", timeout=15000)
    out["first_set"] = pg.eval_on_selector(".agent-prompt-row", "e => e.getAttribute('data-set')")
    pg.screenshot(path=sys.argv[3])
    print(json.dumps(out))
    b.close()
"##;
    let screenshot_file = workspace.path().join("agent_prompts.png");
    let mut py_child = Command::new(python)
        .args(["-c", script])
        .arg(&chrome)
        .arg(&url)
        .arg(screenshot_file.to_string_lossy().as_ref())
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

    assert!(v["row_count"].as_i64().unwrap_or(0) >= 1, "{v}");
    assert_eq!(v["first_set"], "true", "{v}");
    assert!(screenshot_file.exists(), "screenshot was not captured");
}
