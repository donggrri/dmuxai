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

// TC-758: tc_758_docs_preview_renders_in_headless_chrome (UX-753)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn tc_758_docs_preview_renders_in_headless_chrome() {
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

// TC-834: tc_834_docs_tree_renders_in_headless_chrome (FR-832, UX-830)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn tc_834_docs_tree_renders_in_headless_chrome() {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome/Chromium found (set DMUXAI_CHROME)");
        return;
    };
    let workspace = tempfile::tempdir().expect("temp workspace");
    std::fs::create_dir_all(workspace.path().join("docs")).expect("create docs dir");
    std::fs::write(
        workspace.path().join("docs/guide.md"),
        "# Guide Document\n\nWelcome to docs tree preview.\n",
    )
    .expect("write guide.md");
    std::fs::write(workspace.path().join("README.md"), "# Root README\n").expect("write README.md");

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

    let script = r##"
import sys
from playwright.sync_api import sync_playwright
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page()
    pg.goto(sys.argv[2])
    # Toggle sidebar open if not already open
    pg.wait_for_selector("#sidebar-toggle-files", timeout=15000)
    toggle = pg.query_selector("#sidebar-toggle-files")
    if toggle.get_attribute("aria-expanded") != "true":
        toggle.click()
    pg.wait_for_selector("#sidebar.open", timeout=15000)
    # Wait for root folder in tree
    pg.wait_for_selector('[data-path="docs"]', timeout=15000)
    # Click to expand docs folder
    pg.click('[data-path="docs"]')
    # Wait for child file inside expanded docs folder
    pg.wait_for_selector('[data-path="docs/guide.md"]', timeout=15000)
    # Click file to open preview
    pg.click('[data-path="docs/guide.md"]')
    # Wait for preview pane with rendered markdown
    pg.wait_for_selector(".md-body", timeout=15000)
    # Save screenshot
    screenshot_path = sys.argv[3]
    pg.screenshot(path=screenshot_path)
    print(pg.content())
    b.close()
"##;
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
    let screenshot_file = workspace.path().join("docs_tree.png");
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
    let out_bytes = match rx_out.recv_timeout(Duration::from_secs(30)) {
        Ok(b) => {
            let _ = py_child.wait();
            b
        }
        Err(_) => {
            let _ = py_child.kill();
            let _ = py_child.wait();
            panic!("playwright execution timed out after 30s");
        }
    };
    let dom = String::from_utf8_lossy(&out_bytes);

    assert!(dom.contains("sidebar"), "sidebar not in DOM:\n{dom}");
    assert!(dom.contains("docs/guide.md"), "guide.md not in DOM:\n{dom}");
    assert!(
        dom.contains("Guide Document"),
        "guide preview heading not in DOM:\n{dom}"
    );
    assert!(screenshot_file.exists(), "screenshot was not captured");
}

// TC-2266 (FR-2260, FR-2262)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn tc_2266_docs_diff_unknown_extension_in_headless_chrome() {
    let git_status = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !git_status.map(|s| s.success()).unwrap_or(false) {
        println!("SKIP: git not found or git --version failed");
        return;
    }

    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome/Chromium found (set DMUXAI_CHROME)");
        return;
    };

    let workspace = tempfile::tempdir().expect("temp workspace");
    let ws_path = workspace.path();

    let run_git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(ws_path)
            .status()
            .expect("execute git");
        assert!(status.success(), "git command failed: {args:?}");
    };

    run_git(&["init", "-q"]);
    run_git(&["config", "user.name", "t"]);
    run_git(&["config", "user.email", "t@e"]);

    let bbappend_name = "openssh_%.bbappend";
    let bbappend_v1 =
        "FILESEXTRAPATHS_prepend := \"${THISDIR}/files:\"\nSRC_URI += \"file://sample.conf\"\n";
    let bbappend_path = ws_path.join(bbappend_name);
    std::fs::write(&bbappend_path, bbappend_v1).expect("write bbappend v1");

    let blob_name = "blob.bin";
    let blob_v1 = b"\x00\x01binary";
    let blob_path = ws_path.join(blob_name);
    std::fs::write(&blob_path, blob_v1).expect("write blob v1");

    run_git(&["add", bbappend_name, blob_name]);
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@e",
            "commit",
            "-m",
            "initial",
        ])
        .current_dir(ws_path)
        .status()
        .expect("git commit");
    assert!(status.success(), "git commit failed");

    let bbappend_v2 =
        "FILESEXTRAPATHS_prepend := \"${THISDIR}/files:\"\nSRC_URI += \"file://other.conf\"\n";
    std::fs::write(&bbappend_path, bbappend_v2).expect("write bbappend v2");

    let blob_v2 = b"\x00\x02binary2";
    std::fs::write(&blob_path, blob_v2).expect("write blob v2");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(ws_path)
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
import sys
from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page()

    # 1. goto url + '#doc=openssh_%25.bbappend'
    url = sys.argv[2]
    pg.goto(url + '#doc=' + 'openssh_%25.bbappend')
    pg.wait_for_selector('.doc-plain-view, .doc-code-view', timeout=15000)
    plain_content = pg.content()
    visible = 'file://other.conf' in plain_content
    print(f"BBAPPEND_VISIBLE:{visible}")
    if 'Preview not available for this file type.' in plain_content:
        print("BBAPPEND_HAS_UNAVAILABLE:TRUE")
    else:
        print("BBAPPEND_HAS_UNAVAILABLE:FALSE")

    # Click Diff toggle
    pg.wait_for_selector('#dmuxai-doc-diff-toggle', timeout=15000)
    pg.click('#dmuxai-doc-diff-toggle')
    pg.wait_for_selector('pre.doc-diff', timeout=15000)
    diff_text = pg.query_selector('pre.doc-diff').text_content()
    print("===BBAPPEND_DIFF_START===")
    print(diff_text)
    print("===BBAPPEND_DIFF_END===")

    # 2. goto url + '#doc=blob.bin'
    pg.goto(url + '#doc=blob.bin')
    # wait for 'Show git diff' button
    diff_btn = pg.wait_for_selector('text=Show git diff', timeout=15000)
    diff_btn.click()
    pg.wait_for_selector('pre.doc-diff', timeout=15000)
    blob_diff_text = pg.query_selector('pre.doc-diff').text_content()
    print("===BLOB_DIFF_START===")
    print(blob_diff_text)
    print("===BLOB_DIFF_END===")

    b.close()
"##;

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
    let out_bytes = match rx_out.recv_timeout(Duration::from_secs(45)) {
        Ok(b) => {
            let _ = py_child.wait();
            b
        }
        Err(_) => {
            let _ = py_child.kill();
            let _ = py_child.wait();
            panic!("playwright execution timed out after 45s");
        }
    };
    let output = String::from_utf8_lossy(&out_bytes);

    assert!(
        output.contains("BBAPPEND_VISIBLE:True"),
        "bbappend text not visible:\n{output}"
    );
    assert!(
        output.contains("BBAPPEND_HAS_UNAVAILABLE:FALSE"),
        "page contained 'Preview not available for this file type.':\n{output}"
    );

    let bbappend_diff = if let Some(start) = output.find("===BBAPPEND_DIFF_START===\n") {
        let rest = &output[start + "===BBAPPEND_DIFF_START===\n".len()..];
        if let Some(end) = rest.find("\n===BBAPPEND_DIFF_END===") {
            &rest[..end]
        } else {
            ""
        }
    } else {
        ""
    };

    assert!(
        bbappend_diff.contains("diff --git a/openssh_%.bbappend b/openssh_%.bbappend"),
        "missing diff header in bbappend diff:\n{bbappend_diff}"
    );
    assert!(
        bbappend_diff.contains("@@"),
        "missing hunk header in bbappend diff:\n{bbappend_diff}"
    );
    assert!(
        bbappend_diff.contains("-SRC_URI += \"file://sample.conf\""),
        "missing deleted line in bbappend diff:\n{bbappend_diff}"
    );
    assert!(
        bbappend_diff.contains("+SRC_URI += \"file://other.conf\""),
        "missing added line in bbappend diff:\n{bbappend_diff}"
    );

    let blob_diff = if let Some(start) = output.find("===BLOB_DIFF_START===\n") {
        let rest = &output[start + "===BLOB_DIFF_START===\n".len()..];
        if let Some(end) = rest.find("\n===BLOB_DIFF_END===") {
            &rest[..end]
        } else {
            ""
        }
    } else {
        ""
    };

    assert!(
        blob_diff.contains("Binary files a/blob.bin and b/blob.bin differ"),
        "missing binary files differ line in blob diff:\n{blob_diff}"
    );
}
