//! TC-842 (SPEC-todo-sidebar FR-840..FR-842): the header `To-dos` button opens
//! the sidebar on the To-dos tab, the state filter and search narrow the list,
//! and the Files toggle switches tabs. Needs Chrome/Chromium and python
//! Playwright, so it is `#[ignore]`d; run with
//! `cargo test --test web_todo_sidebar -- --ignored`.

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

// TC-842: todo_sidebar_renders_in_headless_chrome (FR-840..FR-842)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn todo_sidebar_renders_in_headless_chrome() {
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
    for title in [
        "Add CSV export to reports",
        "Write SPEC for PDF export",
        "Fix flaky table test",
    ] {
        let status = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .args(["todo", "add", title])
            .current_dir(workspace.path())
            .env("HOME", home.path())
            .env_remove("DMUXAI_WORKSPACE")
            .stdout(Stdio::null())
            .status()
            .expect("dmuxai todo add");
        assert!(status.success(), "todo add failed");
    }

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
def visible_titles(pg):
    return pg.eval_on_selector_all("#todo-list .todo", "els => els.map(e => e.textContent)")
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    # bypass_csp only lets Playwright run its own wait predicates; the page CSP is unchanged.
    pg = b.new_page(viewport={"width": 1400, "height": 900}, bypass_csp=True)
    pg.goto(sys.argv[2])
    pg.wait_for_function("document.querySelector('#todos-count') && document.querySelector('#todos-count').textContent === '3'", timeout=15000)
    out["sidebar_open_before"] = pg.eval_on_selector("#sidebar", "e => e.classList.contains('open')")
    pg.click("#todos-toggle")
    pg.wait_for_selector("#sidebar.open #tab-panel-todos:not([hidden])", timeout=15000)
    out["todos_expanded"] = pg.get_attribute("#todos-toggle", "aria-expanded")
    out["tab_todos_selected"] = pg.get_attribute("#tab-todos", "aria-selected")
    pg.wait_for_function("document.querySelectorAll('#todo-list .todo').length === 3", timeout=15000)
    out["open_count"] = len(visible_titles(pg))
    # Mark one done through its checkbox (FR-841 actions keep working).
    pg.locator("#todo-list .todo", has_text="Fix flaky table test").locator("input[type=checkbox]").check()
    pg.wait_for_function("document.querySelector('#todos-count').textContent === '2'", timeout=15000)
    pg.wait_for_function("document.querySelectorAll('#todo-list .todo').length === 2", timeout=15000)
    out["open_after_done"] = visible_titles(pg)
    pg.select_option("#todo-filter-state", "done")
    out["done_filter"] = visible_titles(pg)
    pg.select_option("#todo-filter-state", "all")
    out["all_filter_count"] = len(visible_titles(pg))
    pg.fill("#todo-search", "export")
    out["search_export"] = visible_titles(pg)
    pg.screenshot(path=sys.argv[3])
    pg.click("#sidebar-toggle-files")
    out["files_selected"] = pg.get_attribute("#tab-files", "aria-selected")
    out["files_panel_hidden"] = pg.eval_on_selector("#tab-panel-files", "e => e.hidden")
    out["todos_panel_hidden"] = pg.eval_on_selector("#tab-panel-todos", "e => e.hidden")
    out["todos_expanded_after"] = pg.get_attribute("#todos-toggle", "aria-expanded")
    # Esc closes the sidebar when focus is not in a text field.
    pg.focus("#tab-files")
    pg.keyboard.press("Escape")
    out["open_after_esc"] = pg.eval_on_selector("#sidebar", "e => e.classList.contains('open')")
    print(json.dumps(out))
    b.close()
"##;
    let screenshot_file = workspace.path().join("todo_sidebar.png");
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

    assert_eq!(v["sidebar_open_before"], false, "{v}");
    assert_eq!(v["todos_expanded"], "true", "{v}");
    assert_eq!(v["tab_todos_selected"], "true", "{v}");
    assert_eq!(v["open_count"], 3, "{v}");
    let open_after: Vec<String> = serde_json::from_value(v["open_after_done"].clone()).unwrap();
    assert!(open_after.iter().all(|t| !t.contains("Fix flaky")), "{v}");
    let done: Vec<String> = serde_json::from_value(v["done_filter"].clone()).unwrap();
    assert_eq!(done.len(), 1, "{v}");
    assert!(done[0].contains("Fix flaky table test"), "{v}");
    assert_eq!(v["all_filter_count"], 3, "{v}");
    let search: Vec<String> = serde_json::from_value(v["search_export"].clone()).unwrap();
    assert_eq!(search.len(), 2, "{v}");
    assert!(search.iter().all(|t| t.contains("export")), "{v}");
    assert_eq!(v["files_selected"], "true", "{v}");
    assert_eq!(v["files_panel_hidden"], false, "{v}");
    assert_eq!(v["todos_panel_hidden"], true, "{v}");
    assert_eq!(v["todos_expanded_after"], "false", "{v}");
    assert_eq!(v["open_after_esc"], false, "{v}");
    assert!(screenshot_file.exists(), "screenshot was not captured");
}
