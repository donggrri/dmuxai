//! TC-863 (SPEC-session-sidebar FR-861..FR-863, UX-860): the left `Sessions`
//! sidebar lists conversations with last agent and relative time, marks the
//! active conversation first with `aria-current="true"`, switches conversations
//! on row click, creates a new conversation via `New`, and persists its
//! collapsed state across reloads. Needs Chrome/Chromium and python Playwright,
//! so it is `#[ignore]`d; run with
//! `cargo test --test web_session_sidebar -- --ignored`.

#![cfg(unix)]

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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

// TC-863: session_sidebar_renders_in_headless_chrome (FR-861..FR-863, UX-860)
#[test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
fn session_sidebar_renders_in_headless_chrome() {
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
    let config = r#"[graph]
enabled = false

[routing]
default_agent = "fake"

[[agents.custom]]
name = "fake"
label = "demo-agent"
command = ["sh", "-c", "echo done", "sh", "{prompt}"]
prompt_via = "arg"
"#;
    std::fs::write(workspace.path().join("dmuxai.toml"), config).expect("write dmuxai.toml");

    let bin = env!("CARGO_BIN_EXE_dmuxai");
    for prompt in ["First seeded session", "Second seeded session"] {
        let status = Command::new(bin)
            .args(["run", prompt, "--agent", "fake"])
            .current_dir(workspace.path())
            .env("HOME", home.path())
            .env_remove("DMUXAI_WORKSPACE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("dmuxai run seed");
        assert!(status.success(), "dmuxai run failed for {prompt}");
        std::thread::sleep(Duration::from_millis(20));
    }

    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind free port");
        listener.local_addr().expect("local addr").port()
    };
    let token = "0123456789abcdef0123456789abcdef01234567";
    let screenshot_file = workspace.path().join("session_sidebar.png");

    let script = r##"
import sys, os, pty, time, json, signal, struct, fcntl, termios, threading, urllib.request
from playwright.sync_api import sync_playwright

chrome_bin = sys.argv[1]
dmuxai_bin = sys.argv[2]
workspace_dir = sys.argv[3]
home_dir = sys.argv[4]
port = int(sys.argv[5])
token = sys.argv[6]
screenshot_path = sys.argv[7]

pid, master_fd = pty.fork()
if pid == 0:
    os.chdir(workspace_dir)
    os.environ["HOME"] = home_dir
    os.environ["TERM"] = "xterm-256color"
    os.environ["DMUXAI_WEB_TOKEN"] = token
    os.environ.pop("DMUXAI_WORKSPACE", None)
    winsize = struct.pack("HHHH", 40, 120, 0, 0)
    try:
        fcntl.ioctl(sys.stdout.fileno(), termios.TIOCSWINSZ, winsize)
    except Exception:
        pass
    os.execv(dmuxai_bin, [dmuxai_bin, "--web", "--port", str(port)])

winsize = struct.pack("HHHH", 40, 120, 0, 0)
try:
    fcntl.ioctl(master_fd, termios.TIOCSWINSZ, winsize)
except Exception:
    pass

pty_chunks = []
def drain_pty(fd):
    try:
        while True:
            data = os.read(fd, 4096)
            if not data:
                break
            if b"\x1b[6n" in data:
                try:
                    os.write(fd, b"\x1b[1;1R")
                except OSError:
                    pass
            if len(pty_chunks) < 50:
                pty_chunks.append(data.decode("utf-8", errors="replace"))
    except OSError:
        pass

t = threading.Thread(target=drain_pty, args=(master_fd,), daemon=True)
t.start()

base_url = f"http://127.0.0.1:{port}"
state_url = f"{base_url}/api/state"
dash_url = f"{base_url}/?token={token}"

def wait_for_server():
    deadline = time.time() + 20.0
    while time.time() < deadline:
        try:
            req = urllib.request.Request(state_url, headers={"Authorization": f"Bearer {token}"})
            with urllib.request.urlopen(req, timeout=1.0) as resp:
                body = json.loads(resp.read().decode("utf-8"))
                if body.get("session") and len(body["session"].get("conversations", [])) >= 2:
                    return
        except Exception:
            pass
        time.sleep(0.1)
    raise RuntimeError("timed out waiting for dmuxai --web server; pty=" + "".join(pty_chunks))

def rows_snapshot(pg):
    return pg.eval_on_selector_all(
        "#sessions-list .sessions-row",
        """els => els.map(e => ({
            id: e.getAttribute('data-id'),
            ariaCurrent: e.getAttribute('aria-current'),
            title: e.querySelector('.sessions-row-title')?.textContent || '',
            agent: e.querySelector('.sessions-row-agent')?.textContent || '',
            time: e.querySelector('.sessions-row-time')?.textContent || '',
            timeTitle: e.querySelector('.sessions-row-time')?.getAttribute('title') || ''
        }))"""
    )

out = {}
try:
    wait_for_server()
    with sync_playwright() as p:
        b = p.chromium.launch(executable_path=chrome_bin, args=["--no-sandbox"])
        pg = b.new_page(viewport={"width": 1400, "height": 900}, bypass_csp=True)
        pg.goto(dash_url)
        pg.wait_for_function(
            "document.querySelectorAll('#sessions-list .sessions-row').length >= 2",
            timeout=15000,
        )

        out["initial_open"] = pg.eval_on_selector("#sessions-sidebar", "e => e.classList.contains('open')")
        out["initial_body_dock"] = pg.eval_on_selector("body", "e => e.classList.contains('left-sidebar-open')")
        out["initial_expanded"] = pg.get_attribute("#sessions-toggle", "aria-expanded")
        initial_rows = rows_snapshot(pg)
        out["initial_rows"] = initial_rows

        second_id = initial_rows[1]["id"]
        pg.locator("#sessions-list .sessions-row").nth(1).click()
        try:
            pg.wait_for_function(
                """expectedId => {
                    const first = document.querySelector('#sessions-list .sessions-row');
                    return first && first.getAttribute('data-id') === expectedId && first.getAttribute('aria-current') === 'true';
                }""",
                arg=second_id,
                timeout=15000,
            )
        except Exception as err:
            notice = pg.eval_on_selector(".session-notice", "e => e.textContent")
            rows_now = rows_snapshot(pg)
            raise RuntimeError(f"switch failed: notice={notice!r} rows={rows_now!r} pty={''.join(pty_chunks)!r}") from err
        out["after_switch_rows"] = rows_snapshot(pg)

        prev_count = len(out["after_switch_rows"])
        pg.click("#sessions-new")
        pg.wait_for_function(
            f"document.querySelectorAll('#sessions-list .sessions-row').length === {prev_count + 1}",
            timeout=15000,
        )
        out["after_new_rows"] = rows_snapshot(pg)

        pg.screenshot(path=screenshot_path)

        pg.click("#sessions-close")
        pg.wait_for_function(
            "!document.querySelector('#sessions-sidebar').classList.contains('open')",
            timeout=15000,
        )
        out["closed_before_reload"] = pg.eval_on_selector("#sessions-sidebar", "e => e.classList.contains('open')")

        pg.reload()
        pg.wait_for_function(
            f"document.querySelectorAll('#sessions-list .sessions-row').length >= {prev_count + 1}",
            timeout=15000,
        )
        out["open_after_reload"] = pg.eval_on_selector("#sessions-sidebar", "e => e.classList.contains('open')")
        out["expanded_after_reload"] = pg.get_attribute("#sessions-toggle", "aria-expanded")
        b.close()
finally:
    try:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    except Exception:
        pass
    try:
        os.close(master_fd)
    except Exception:
        pass

print(json.dumps(out))
"##;

    let mut py_child = Command::new(python)
        .args(["-c", script])
        .arg(&chrome)
        .arg(bin)
        .arg(workspace.path())
        .arg(home.path())
        .arg(port.to_string())
        .arg(token)
        .arg(&screenshot_file)
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

    assert_eq!(v["initial_open"], true, "{v}");
    assert_eq!(v["initial_body_dock"], true, "{v}");
    assert_eq!(v["initial_expanded"], "true", "{v}");

    let initial_rows = v["initial_rows"].as_array().expect("initial_rows array");
    assert!(initial_rows.len() >= 2, "{v}");
    assert_eq!(initial_rows[0]["ariaCurrent"], "true", "{v}");
    assert!(initial_rows[1]["ariaCurrent"].is_null(), "{v}");
    let seeded_rows: Vec<_> = initial_rows
        .iter()
        .filter(|row| row["agent"] == "demo-agent")
        .collect();
    assert_eq!(seeded_rows.len(), 2, "{v}");
    for row in initial_rows {
        let time_str = row["time"].as_str().unwrap_or("");
        assert!(time_str.contains("ago"), "expected relative time in {row}");
        let time_title = row["timeTitle"].as_str().unwrap_or("");
        assert!(
            time_title.contains('T'),
            "expected ISO timestamp title in {row}"
        );
    }

    let second_id = initial_rows[1]["id"].as_str().expect("second row id");
    let after_switch = v["after_switch_rows"]
        .as_array()
        .expect("after_switch_rows array");
    assert_eq!(after_switch[0]["id"], second_id, "{v}");
    assert_eq!(after_switch[0]["ariaCurrent"], "true", "{v}");
    assert!(after_switch[1]["ariaCurrent"].is_null(), "{v}");

    let after_new = v["after_new_rows"]
        .as_array()
        .expect("after_new_rows array");
    assert_eq!(after_new.len(), after_switch.len() + 1, "{v}");
    assert_eq!(after_new[0]["ariaCurrent"], "true", "{v}");
    assert_ne!(after_new[0]["id"], second_id, "{v}");

    assert_eq!(v["closed_before_reload"], false, "{v}");
    assert_eq!(v["open_after_reload"], false, "{v}");
    assert_eq!(v["expanded_after_reload"], "false", "{v}");
    assert!(screenshot_file.exists(), "screenshot was not captured");
}
