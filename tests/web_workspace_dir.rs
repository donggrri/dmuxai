//! TC-874, TC-876 (SPEC-workspace-dir FR-871, FR-872, FR-874):
//! - `workspace_dialog_renders_in_headless_chrome` (TC-874): header workspace
//!   button shows abbreviated path with full path in `title`; clicking opens the
//!   dialog listing recent paths and subdirectories (no files, no dotdirs);
//!   entering an invalid path (`/`) shows the validation reason inline and
//!   disables Switch; captures a screenshot.
//! - `workspace_switch_reexecs_serve` (TC-876): `POST /api/workspace/switch`
//!   re-executes `dmuxai --web` on the same port and token in the new workspace
//!   within 15s.

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

// TC-874 (FR-871, FR-872)
#[test]
#[ignore = "needs Chrome/Chromium and python playwright; run with --ignored"]
fn workspace_dialog_renders_in_headless_chrome() {
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

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
    let home = root.join("home");
    let state_dir = root.join("state");
    let ws_a = home.join("proj-alpha");
    let ws_b = home.join("proj-beta");
    std::fs::create_dir_all(ws_a.join("sub-one")).unwrap();
    std::fs::create_dir_all(ws_a.join("sub-two")).unwrap();
    std::fs::create_dir_all(ws_a.join(".hidden-sub")).unwrap();
    std::fs::write(ws_a.join("notes.txt"), "hello world").unwrap();
    std::fs::create_dir_all(&ws_b).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();

    // Seed recent-workspaces.json with ws_b so recent list is non-empty.
    std::fs::write(
        state_dir.join("recent-workspaces.json"),
        serde_json::to_vec(&vec![ws_b.to_string_lossy().into_owned()]).unwrap(),
    )
    .unwrap();

    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind free port");
        listener.local_addr().expect("local addr").port()
    };
    let token = "0123456789abcdef0123456789abcdef01234567";
    let screenshot_file = root.join("workspace_dialog.png");
    let bin = env!("CARGO_BIN_EXE_dmuxai");

    let script = r##"
import sys, os, pty, time, json, signal, struct, fcntl, termios, threading, urllib.request
from playwright.sync_api import sync_playwright

chrome_bin = sys.argv[1]
dmuxai_bin = sys.argv[2]
workspace_dir = sys.argv[3]
home_dir = sys.argv[4]
state_dir = sys.argv[5]
port = int(sys.argv[6])
token = sys.argv[7]
screenshot_path = sys.argv[8]

pid, master_fd = pty.fork()
if pid == 0:
    os.chdir(workspace_dir)
    os.environ["HOME"] = home_dir
    os.environ["DMUXAI_STATE_DIR"] = state_dir
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
ws_url = f"{base_url}/api/workspace"
dash_url = f"{base_url}/?token={token}"

def wait_for_server():
    deadline = time.time() + 20.0
    while time.time() < deadline:
        try:
            req = urllib.request.Request(ws_url, headers={"Authorization": f"Bearer {token}"})
            with urllib.request.urlopen(req, timeout=1.0) as resp:
                body = json.loads(resp.read().decode("utf-8"))
                if body.get("current") == workspace_dir:
                    return
        except Exception:
            pass
        time.sleep(0.1)
    raise RuntimeError("timed out waiting for dmuxai --web server; pty=" + "".join(pty_chunks))

out = {}
try:
    wait_for_server()
    with sync_playwright() as p:
        b = p.chromium.launch(executable_path=chrome_bin, args=["--no-sandbox"])
        pg = b.new_page(viewport={"width": 1280, "height": 800}, bypass_csp=True)
        pg.goto(dash_url)
        pg.wait_for_function(
            "document.getElementById('workspace-button')?.getAttribute('title') === " + json.dumps(workspace_dir),
            timeout=15000,
        )
        out["header_title"] = pg.get_attribute("#workspace-button", "title")
        out["header_text"] = pg.text_content("#workspace-button")

        pg.click("#workspace-button")
        pg.wait_for_function(
            "!document.getElementById('workspace-dialog').hidden && document.querySelectorAll('#workspace-browser .workspace-dir-btn').length >= 2",
            timeout=15000,
        )
        out["dirs"] = pg.eval_on_selector_all(
            "#workspace-browser .workspace-dir-btn",
            "els => els.map(e => e.textContent)"
        )
        out["recent"] = pg.eval_on_selector_all(
            "#workspace-recent .workspace-recent-btn",
            "els => els.map(e => e.getAttribute('title'))"
        )

        pg.fill("#workspace-path", "/")
        pg.press("#workspace-path", "Enter")
        pg.wait_for_function(
            "!document.getElementById('workspace-reason').hidden && document.getElementById('workspace-reason').textContent.includes('denied: filesystem root')",
            timeout=15000,
        )
        out["invalid_reason"] = pg.text_content("#workspace-reason")
        out["switch_disabled"] = pg.eval_on_selector("#workspace-switch", "e => e.disabled")

        pg.screenshot(path=screenshot_path)
        b.close()
finally:
    try:
        os.killpg(pid, signal.SIGKILL)
    except Exception:
        try:
            os.kill(pid, signal.SIGKILL)
        except Exception:
            pass
    try:
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
        .arg(&ws_a)
        .arg(&home)
        .arg(&state_dir)
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
            panic!("playwright timed out after 60s");
        }
    };
    let stdout_str = String::from_utf8_lossy(&out_bytes);
    let last_line = stdout_str
        .lines()
        .rfind(|l| l.starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON output from playwright: {stdout_str}"));
    let parsed: serde_json::Value = serde_json::from_str(last_line).expect("parse JSON");

    assert_eq!(
        parsed["header_title"].as_str(),
        Some(ws_a.to_string_lossy().as_ref())
    );
    assert_eq!(parsed["header_text"].as_str(), Some("~/proj-alpha"));
    let dirs: Vec<&str> = parsed["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(dirs, vec!["sub-one", "sub-two"]);
    let recent: Vec<&str> = parsed["recent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(recent.contains(&ws_b.to_string_lossy().as_ref()));
    assert_eq!(
        parsed["invalid_reason"].as_str(),
        Some("denied: filesystem root")
    );
    assert_eq!(parsed["switch_disabled"].as_bool(), Some(true));

    let png_bytes = std::fs::read(&screenshot_file).expect("read screenshot");
    assert!(png_bytes.len() > 1000, "screenshot too small");
    assert_eq!(&png_bytes[..4], b"\x89PNG");
}

// TC-876 (FR-874)
#[test]
#[ignore = "spawns interactive dmuxai --web under pty and re-execs; run with --ignored"]
fn workspace_switch_reexecs_serve() {
    let python = "python3";
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
    let home = root.join("home");
    let state_dir = root.join("state");
    let ws_a = home.join("ws-first");
    let ws_b = home.join("ws-second");
    std::fs::create_dir_all(&ws_a).unwrap();
    std::fs::create_dir_all(&ws_b).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();

    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind free port");
        listener.local_addr().expect("local addr").port()
    };
    let token = "fedcba9876543210fedcba9876543210fedcba98";
    let bin = env!("CARGO_BIN_EXE_dmuxai");

    let script = r##"
import sys, os, pty, time, json, signal, struct, fcntl, termios, threading, urllib.request

dmuxai_bin = sys.argv[1]
ws_a = sys.argv[2]
ws_b = sys.argv[3]
home_dir = sys.argv[4]
state_dir = sys.argv[5]
port = int(sys.argv[6])
token = sys.argv[7]

pid, master_fd = pty.fork()
if pid == 0:
    os.chdir(ws_a)
    os.environ["HOME"] = home_dir
    os.environ["DMUXAI_STATE_DIR"] = state_dir
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
            if len(pty_chunks) < 100:
                pty_chunks.append(data.decode("utf-8", errors="replace"))
    except OSError:
        pass

t = threading.Thread(target=drain_pty, args=(master_fd,), daemon=True)
t.start()

base_url = f"http://127.0.0.1:{port}"
ws_url = f"{base_url}/api/workspace"
state_url = f"{base_url}/api/state"
switch_url = f"{base_url}/api/workspace/switch"
headers = {"Authorization": f"Bearer {token}", "Content-Type": "application/json"}

out = {}
try:
    deadline = time.time() + 15.0
    while time.time() < deadline:
        try:
            req = urllib.request.Request(ws_url, headers=headers)
            with urllib.request.urlopen(req, timeout=1.0) as resp:
                body = json.loads(resp.read().decode("utf-8"))
                if body.get("current") == ws_a:
                    out["initial_current"] = body["current"]
                    break
        except Exception:
            pass
        time.sleep(0.1)
    else:
        raise RuntimeError("timed out waiting for initial workspace; pty=" + "".join(pty_chunks))

    switch_req = urllib.request.Request(
        switch_url,
        data=json.dumps({"path": ws_b}).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    with urllib.request.urlopen(switch_req, timeout=5.0) as resp:
        out["switch_status"] = resp.status
        out["switch_body"] = json.loads(resp.read().decode("utf-8"))

    deadline = time.time() + 15.0
    while time.time() < deadline:
        try:
            req = urllib.request.Request(state_url, headers=headers)
            with urllib.request.urlopen(req, timeout=1.0) as resp:
                body = json.loads(resp.read().decode("utf-8"))
                if body.get("workspace") == ws_b:
                    out["switched_workspace"] = body["workspace"]
                    break
        except Exception:
            pass
        time.sleep(0.15)
    else:
        raise RuntimeError("timed out waiting for re-exec into ws_b; pty=" + "".join(pty_chunks))
finally:
    try:
        os.killpg(pid, signal.SIGKILL)
    except Exception:
        try:
            os.kill(pid, signal.SIGKILL)
        except Exception:
            pass
    try:
        os.waitpid(pid, 0)
    except Exception:
        pass
    try:
        os.close(master_fd)
    except Exception:
        pass

print(json.dumps(out))
"##;

    let output = Command::new(python)
        .args(["-c", script])
        .arg(bin)
        .arg(&ws_a)
        .arg(&ws_b)
        .arg(&home)
        .arg(&state_dir)
        .arg(port.to_string())
        .arg(token)
        .output()
        .expect("run python pty switch test");
    assert!(
        output.status.success(),
        "python pty test failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let last_line = stdout_str
        .lines()
        .rfind(|l| l.starts_with('{'))
        .expect("JSON line");
    let parsed: serde_json::Value = serde_json::from_str(last_line).expect("parse JSON");
    assert_eq!(
        parsed["initial_current"].as_str(),
        Some(ws_a.to_string_lossy().as_ref())
    );
    assert_eq!(parsed["switch_status"].as_u64(), Some(202));
    assert_eq!(
        parsed["switched_workspace"].as_str(),
        Some(ws_b.to_string_lossy().as_ref())
    );
}
