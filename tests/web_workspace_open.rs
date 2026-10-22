//! TC-1471 (SPEC-workspace-open-headless FR-1470): a `--web` dashboard spawned
//! for "Open in new window" runs without a TTY, publishes `web.json`, serves
//! the API, and exits on the dashboard `quit` command. Without the headless
//! env it still refuses to run without a TTY.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn http(port: u16, method: &str, path: &str, token: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

fn base_command(workspace: &std::path::Path, state: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    cmd.args(["--web", "-C"])
        .arg(workspace)
        .args(["--port", "0", "--bind", "127.0.0.1"])
        .env("DMUXAI_STATE_DIR", state)
        .env("HOME", state)
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WORKSPACE")
        .env_remove("DMUXAI_HEADLESS_DASHBOARD")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    cmd
}

// TC-1471: open_in_new_window_starts_headless_dashboard (FR-1470)
#[test]
fn tc_1471_open_in_new_window_starts_headless_dashboard() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&state).unwrap();

    // Without the env var a TTY is still required.
    let out = base_command(&workspace, &state).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("interactive terminal"));

    let child = base_command(&workspace, &state)
        .env("DMUXAI_HEADLESS_DASHBOARD", "1")
        .spawn()
        .unwrap();
    let mut guard = ChildGuard(Some(child));
    let discovery_path = workspace.join(".dmuxai").join("web.json");
    let deadline = Instant::now() + Duration::from_secs(20);
    let discovery: serde_json::Value = loop {
        if let Ok(text) = std::fs::read_to_string(&discovery_path)
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
        {
            break value;
        }
        if let Some(status) = guard.0.as_mut().unwrap().try_wait().unwrap() {
            panic!("headless dashboard exited early: {status}");
        }
        assert!(Instant::now() < deadline, "no web.json within 20 s");
        std::thread::sleep(Duration::from_millis(100));
    };
    let port = discovery["port"].as_u64().unwrap() as u16;
    let token = discovery["token"].as_str().unwrap().to_owned();

    let state_resp = http(port, "GET", "/api/state", &token, "");
    assert!(state_resp.starts_with("HTTP/1.1 200"), "{state_resp}");
    assert!(state_resp.contains("\"workspace\""), "{state_resp}");

    let quit = http(
        port,
        "POST",
        "/api/command",
        &token,
        r#"{"command":"quit"}"#,
    );
    assert!(quit.starts_with("HTTP/1.1 200"), "{quit}");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut child = guard.0.take().unwrap();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "quit exits cleanly: {status}");
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("headless dashboard did not exit after quit");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
