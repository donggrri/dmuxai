//! TC-758 / UX-753 (SPEC-docs-preview): the dashboard opens `#doc=README.md`
//! and shows rendered GitHub-style Markdown. Needs Chrome/Chromium, so it is
//! `#[ignore]`d (Chrome + python Playwright); run with `cargo test --test web_docs_preview -- --ignored`.

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

// TC-758: docs_preview_renders_in_headless_chrome (UX-753)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn docs_preview_renders_in_headless_chrome() {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome/Chromium found (set DMUXAI_CHROME)");
        return;
    };
    let workspace = tempfile::tempdir().expect("temp workspace");
    std::fs::write(
        workspace.path().join("README.md"),
        "# Preview fixture\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n- [x] done\n\n```rust\nfn main() {}\n```\n",
    )
    .expect("write fixture");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace.path())
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

    // `--dump-dom` does not wait for the page's async fetches (and SSE keeps
    // virtual time from settling), so drive Chrome through Playwright.
    let script = r#"
import sys
from playwright.sync_api import sync_playwright
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page()
    pg.goto(sys.argv[2])
    pg.wait_for_selector(".md-body table", timeout=15000)
    print(pg.content())
    b.close()
"#;
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
    let output = Command::new(python)
        .args(["-c", script])
        .arg(&chrome)
        .arg(format!("{url}#doc=README.md"))
        .stderr(Stdio::inherit())
        .output()
        .expect("run playwright");
    let dom = String::from_utf8_lossy(&output.stdout);

    assert!(dom.contains("md-body"), "no .md-body in DOM:\n{dom}");
    assert!(
        dom.contains("<table>"),
        "markdown table not rendered:\n{dom}"
    );
    assert!(
        dom.contains("Preview fixture"),
        "heading not rendered:\n{dom}"
    );
    assert!(dom.contains("hl-"), "code not highlighted:\n{dom}");
}
