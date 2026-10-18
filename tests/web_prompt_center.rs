//! TC-851 (SPEC-centered-prompt FR-850..FR-852, UX-850): the prompt box
//! (`.session-prompt`) stays centered in the content area across widths
//! 1280, 1440, 1920 and 800 and the four sidebar states (none, right, left,
//! both), without overlapping open docked sidebars, and keeps typed prompt
//! text across toggles. Needs Chrome/Chromium and python Playwright, so it is
//! `#[ignore]`d; run with
//! `cargo test --test web_prompt_center -- --ignored`.

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

// TC-851: prompt_stays_centered_in_headless_chrome (FR-850..FR-852, UX-850)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn prompt_stays_centered_in_headless_chrome() {
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
    std::fs::write(workspace.path().join("README.md"), "# Centered prompt\n").expect("write doc");

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

out = {"measurements": {}}

def rect_of(pg, sel):
    return pg.eval_on_selector(sel, """e => {
        const r = e.getBoundingClientRect();
        return {left: r.left, right: r.right, width: r.width, top: r.top, bottom: r.bottom, height: r.height};
    }""")

def set_left_dock(pg, enabled):
    is_open = pg.eval_on_selector("#sessions-sidebar", "e => e.classList.contains('open')")
    if is_open != enabled:
        pg.eval_on_selector("#sessions-toggle", "e => e.click()")

def close_right_sidebar(pg, width):
    is_open = pg.eval_on_selector("#sidebar", "e => e.classList.contains('open')")
    if is_open:
        if width > 900:
            pg.click("#sidebar-toggle-files")
        else:
            pg.click("#sidebar-close")

with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1280, "height": 900}, bypass_csp=True)
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#session:not([hidden]) .session-prompt", timeout=15000)
    pg.wait_for_selector("#sidebar-toggle-files", timeout=15000)
    pg.wait_for_selector("#sessions-toggle", timeout=15000)

    # Enable input in view-only serve mode so we can type into .session-input and verify text preservation.
    pg.eval_on_selector(".session-input", "e => { e.disabled = false; }")
    pg.focus(".session-input")
    pg.type(".session-input", "keep this prompt centered")

    for width in [1280, 1440, 1920, 800]:
        pg.set_viewport_size({"width": width, "height": 900})
        close_right_sidebar(pg, width)
        set_left_dock(pg, False)
        pg.wait_for_timeout(300)

        w_key = str(width)
        out["measurements"][w_key] = {}

        # 1. none
        out["measurements"][w_key]["none"] = {
            "prompt": rect_of(pg, ".session-prompt"),
        }

        # 2. right (click #sidebar-toggle-files)
        pg.click("#sidebar-toggle-files")
        pg.wait_for_selector("#sidebar.open", timeout=15000)
        pg.wait_for_timeout(300)
        out["measurements"][w_key]["right"] = {
            "prompt": rect_of(pg, ".session-prompt"),
            "right_sidebar": rect_of(pg, "#sidebar"),
        }

        # 3. left (close right, open real left sessions sidebar)
        close_right_sidebar(pg, width)
        set_left_dock(pg, True)
        pg.wait_for_selector("#sessions-sidebar.open", timeout=15000)
        pg.wait_for_timeout(300)
        out["measurements"][w_key]["left"] = {
            "prompt": rect_of(pg, ".session-prompt"),
            "left_sidebar": rect_of(pg, "#sessions-sidebar"),
        }

        # 4. both (open right while left is open)
        pg.eval_on_selector("#sidebar-toggle-files", "e => e.click()")
        pg.wait_for_selector("#sidebar.open", timeout=15000)
        pg.wait_for_timeout(300)
        out["measurements"][w_key]["both"] = {
            "prompt": rect_of(pg, ".session-prompt"),
            "left_sidebar": rect_of(pg, "#sessions-sidebar"),
            "right_sidebar": rect_of(pg, "#sidebar"),
        }

        close_right_sidebar(pg, width)
        set_left_dock(pg, False)

    out["input_value"] = pg.eval_on_selector(".session-input", "e => e.value")
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

    assert_eq!(
        v["input_value"].as_str(),
        Some("keep this prompt centered"),
        "typed prompt text must survive toggles: {v}"
    );

    for width in [1280_u32, 1440, 1920, 800] {
        let w_f = f64::from(width);
        let by_state = &v["measurements"][width.to_string()];
        for state in ["none", "right", "left", "both"] {
            let entry = &by_state[state];
            let prompt = &entry["prompt"];
            let left = prompt["left"].as_f64().expect("prompt.left");
            let right = prompt["right"].as_f64().expect("prompt.right");
            let p_width = prompt["width"].as_f64().expect("prompt.width");
            let prompt_center = left + p_width / 2.0;

            if width > 900 {
                let left_dock = if state == "left" || state == "both" {
                    320.0
                } else {
                    0.0
                };
                let right_dock = if state == "right" || state == "both" {
                    320.0
                } else {
                    0.0
                };
                let content_center = (left_dock + (w_f - right_dock)) / 2.0;
                assert!(
                    (prompt_center - content_center).abs() <= 2.0,
                    "width={width} state={state}: prompt center {prompt_center} not within 2px of content center {content_center}: {entry}"
                );
                if state == "none" || state == "both" {
                    let viewport_center = w_f / 2.0;
                    assert!(
                        (prompt_center - viewport_center).abs() <= 2.0,
                        "width={width} state={state}: prompt center {prompt_center} not within 2px of viewport center {viewport_center}: {entry}"
                    );
                }
                if state == "left" || state == "both" {
                    let left_sb_right = entry["left_sidebar"]["right"]
                        .as_f64()
                        .expect("left_sidebar.right");
                    assert!(
                        left >= left_sb_right,
                        "width={width} state={state}: prompt left {left} overlaps left sidebar right {left_sb_right}: {entry}"
                    );
                }
                if state == "right" || state == "both" {
                    let right_sb_left = entry["right_sidebar"]["left"]
                        .as_f64()
                        .expect("right_sidebar.left");
                    assert!(
                        right <= right_sb_left,
                        "width={width} state={state}: prompt right {right} overlaps right sidebar left {right_sb_left}: {entry}"
                    );
                }
            } else {
                let viewport_center = w_f / 2.0;
                assert!(
                    (prompt_center - viewport_center).abs() <= 2.0,
                    "width={width} state={state}: narrow prompt center {prompt_center} not within 2px of viewport center {viewport_center}: {entry}"
                );
                let min_width = (w_f - 20.0).min(320.0);
                assert!(
                    p_width >= min_width,
                    "width={width} state={state}: narrow prompt width {p_width} < {min_width}: {entry}"
                );
            }
        }
    }
}
