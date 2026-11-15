//! Browser tests for the Runs view (SPEC-headless-runs-dashboard FR-2811..FR-2814).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use filetime::{FileTime, set_file_mtime};
use serde_json::{Value, json};

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn find_chrome_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CHROME_BIN") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    for candidate in [
        "/opt/google/chrome/chrome",
        "google-chrome",
        "chromium",
        "chromium-browser",
        "chrome",
    ] {
        let p = Path::new(candidate);
        if p.is_absolute() {
            if p.exists() {
                return Some(p.to_path_buf());
            }
        } else if let Ok(output) = Command::new("which").arg(candidate).output()
            && output.status.success()
        {
            let found = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !found.is_empty() {
                return Some(PathBuf::from(found));
            }
        }
    }
    None
}

struct ServeFixture {
    _workspace: tempfile::TempDir,
    workspace: PathBuf,
    data_home: PathBuf,
    dashboard_url: String,
    _guard: ChildGuard,
}

impl ServeFixture {
    fn new() -> Self {
        let workspace = tempfile::tempdir().expect("temp workspace");
        let root = workspace.path();
        let data_home = root.join("data");
        let state_home = root.join("state");
        let cfg_home = root.join("cfg");
        let home = root.join("home");
        fs::create_dir_all(&data_home).expect("data home");
        fs::create_dir_all(&state_home).expect("state home");
        fs::create_dir_all(&cfg_home).expect("cfg home");
        fs::create_dir_all(&home).expect("home");
        let ws_path = root.to_path_buf();
        let dmuxai_dir = ws_path.join(".dmuxai");
        fs::create_dir_all(&dmuxai_dir).expect(".dmuxai");
        fs::write(dmuxai_dir.join("events.jsonl"), "").expect("events");

        let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .args(["serve", "--port", "0"])
            .current_dir(&ws_path)
            .env_remove("DMUXAI_WORKSPACE")
            .env("XDG_DATA_HOME", &data_home)
            .env("XDG_STATE_HOME", &state_home)
            .env("XDG_CONFIG_HOME", &cfg_home)
            .env("HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn dmuxai serve");

        let stdout = child.stdout.take().expect("stdout piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let res = reader.read_line(&mut line).map(|_| line);
            let _ = tx.send(res);
        });

        let first_line = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("recv first stdout line within 20s timeout")
            .expect("read dashboard url line");

        let url = first_line
            .trim()
            .strip_prefix("dmuxai dashboard: ")
            .unwrap_or_else(|| panic!("unexpected stdout: {first_line:?}"));

        Self {
            _workspace: workspace,
            workspace: ws_path,
            data_home,
            dashboard_url: url.to_string(),
            _guard: ChildGuard(Some(child)),
        }
    }

    fn runs_dir(&self) -> PathBuf {
        let key = dmuxai::state_path::repo_key(&self.workspace).expect("repo_key");
        self.data_home
            .join("dmuxai")
            .join("repos")
            .join(key)
            .join("runs")
    }
}

fn base_manifest(run_id: &str, created_at_ms: u64, status: &str) -> Value {
    json!({
        "schema_version": 1,
        "run_id": run_id,
        "origin": "cli_headless",
        "status": status,
        "created_at_ms": created_at_ms,
        "completed_at_ms": null,
        "duration_ms": null,
        "requested_runner": "codex",
        "resolved_runner": "codex",
        "requested_model": "composer-2.5",
        "resolved_model": null,
        "dmuxai_version": "0.0.0",
        "build_commit": null,
        "pid": null,
        "dmuxai_exit_code": null,
        "child_exit_code": null,
        "child_signal": null,
        "error_kind": null,
        "error_summary": null,
        "links": {
            "conversation_id": null,
            "turn_id": null,
            "graph_id": null,
            "node_id": null,
            "attempt_index": null
        },
        "diagnostics": { "stderr_bytes": 0, "tail_sample": null },
        "end_summary": null
    })
}

fn write_manifest(run_dir: &Path, manifest: &Value) {
    fs::create_dir_all(run_dir).expect("run dir");
    fs::write(
        run_dir.join("manifest.json"),
        serde_json::to_vec_pretty(manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
}

fn write_graph_fixture(workspace: &Path, graph_id: &str) {
    let graphs = workspace.join(".dmuxai").join("graphs");
    fs::create_dir_all(&graphs).expect("graphs dir");
    let run = json!({
        "plan": {
            "schema": "dmuxai.graph/v1",
            "id": "serve-run",
            "orchestrator_model": "gpt-4o",
            "user_prompt": "serve graph",
            "limits": { "max_parallel": 2, "max_retries": 2 },
            "nodes": [{
                "id": "a", "role": "plan", "job": "root task",
                "runner": "codex", "model_id": "m", "skill": null,
                "requires_gate": false, "status": "completed", "attempt": 1
            }],
            "edges": []
        },
        "conversation_id": "conv-1",
        "phase": "active",
        "waiting_gate_node": null
    });
    fs::write(
        graphs.join(format!("{graph_id}.json")),
        serde_json::to_vec_pretty(&run).expect("graph json"),
    )
    .expect("write graph");
}

fn http_get_json(authority: &str, path: &str) -> std::io::Result<serde_json::Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = TcpStream::connect(authority)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    let mut temp = [0u8; 4096];
    let mut content_length: Option<usize> = None;

    while Instant::now() < deadline {
        match stream.read(&mut temp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&temp[..n]);
                let text = String::from_utf8_lossy(&buf);
                if content_length.is_none()
                    && let Some(pos) = text.find("\r\n\r\n")
                {
                    let header_part = &text[..pos];
                    for line in header_part.lines() {
                        if let Some(val) = line.to_ascii_lowercase().strip_prefix("content-length:")
                            && let Ok(len) = val.trim().parse::<usize>()
                        {
                            content_length = Some(len);
                            break;
                        }
                    }
                }
                if let Some(len) = content_length
                    && let Some(pos) = text.find("\r\n\r\n")
                    && buf.len() >= pos + 4 + len
                {
                    let body = &text[pos + 4..pos + 4 + len];
                    return serde_json::from_str(body)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").trim();
    serde_json::from_str(body).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

struct CdpClient {
    stream: TcpStream,
    msg_id: u64,
}

impl CdpClient {
    fn connect(authority: &str, path: &str) -> std::io::Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = TcpStream::connect(authority)?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes())?;
        let mut response = [0u8; 1024];
        let mut n = 0;
        while Instant::now() < deadline {
            match stream.read(&mut response) {
                Ok(bytes) => {
                    n = bytes;
                    break;
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e),
            }
        }
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out waiting for WebSocket handshake",
            ));
        }
        let resp_str = String::from_utf8_lossy(&response[..n]);
        if !resp_str.contains("101") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                format!("WebSocket handshake failed: {resp_str}"),
            ));
        }
        Ok(Self { stream, msg_id: 0 })
    }

    fn send(&mut self, msg: &str) -> std::io::Result<()> {
        let payload = msg.as_bytes();
        let len = payload.len();
        let mut frame = Vec::new();
        frame.push(0x81);
        if len < 126 {
            frame.push(0x80 | (len as u8));
        } else if len <= 0xffff {
            frame.push(0x80 | 126);
            frame.push(((len >> 8) & 0xff) as u8);
            frame.push((len & 0xff) as u8);
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "payload too large",
            ));
        }
        frame.extend_from_slice(&[0, 0, 0, 0]);
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame)?;
        self.stream.flush()
    }

    fn eval(&mut self, expr: &str) -> Result<serde_json::Value, String> {
        self.msg_id += 1;
        let id = self.msg_id;
        let msg = serde_json::json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expr,
                "returnByValue": true,
                "awaitPromise": true
            }
        })
        .to_string();
        self.send(&msg).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        let mut temp = [0u8; 4096];
        let target_str = format!("\"id\":{id}");
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match self.stream.read(&mut temp) {
                Ok(0) => return Err("connection closed".to_string()),
                Ok(n) => {
                    buf.extend_from_slice(&temp[..n]);
                    let text = String::from_utf8_lossy(&buf);
                    if text.contains(&target_str)
                        && let Some(start) = text.find(&format!("{{\"id\":{id}"))
                        && let Some(end) = text[start..].rfind('}')
                    {
                        let slice = &text[start..start + end + 1];
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(slice)
                            && val.get("id").and_then(|v| v.as_u64()) == Some(id)
                        {
                            return Ok(val["result"]["result"]["value"].clone());
                        }
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Err(format!("timed out waiting for response to message {id}"))
    }
}

struct ChromeSession {
    _user_data_dir: tempfile::TempDir,
    _chrome: ChildGuard,
    cdp: CdpClient,
}

fn open_dashboard_in_chrome(url: &str) -> Option<ChromeSession> {
    let chrome_bin = find_chrome_binary()?;
    let cdp_port = {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        listener.local_addr().ok()?.port()
    };
    let user_data_dir = tempfile::tempdir().ok()?;
    let chrome_child = Command::new(&chrome_bin)
        .args([
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            &format!("--remote-debugging-port={cdp_port}"),
            &format!("--user-data-dir={}", user_data_dir.path().display()),
            "--window-size=1280,800",
            url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let mut page_ws_url = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(pages) = http_get_json(&format!("127.0.0.1:{cdp_port}"), "/json/list")
            && let Some(arr) = pages.as_array()
        {
            for page in arr {
                if page.get("type").and_then(|t| t.as_str()) == Some("page")
                    && let Some(ws) = page.get("webSocketDebuggerUrl").and_then(|w| w.as_str())
                {
                    page_ws_url = Some(ws.to_owned());
                    break;
                }
            }
        }
        if page_ws_url.is_some() {
            break;
        }
    }
    let page_ws_url = page_ws_url?;
    let ws_path = page_ws_url
        .split(&format!("{cdp_port}"))
        .nth(1)?
        .to_string();
    let cdp = CdpClient::connect(&format!("127.0.0.1:{cdp_port}"), &ws_path).ok()?;
    Some(ChromeSession {
        _user_data_dir: user_data_dir,
        _chrome: ChildGuard(Some(chrome_child)),
        cdp,
    })
}

fn runs_panel_open(session: &mut ChromeSession) -> bool {
    session
        .cdp
        .eval(
            "(() => { const p = document.querySelector('#runs'); return !!(p && !p.hidden && p.classList.contains('open')); })()",
        )
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn wait_for_runs_open(session: &mut ChromeSession) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if runs_panel_open(session) {
            return;
        }
    }
    panic!("timed out waiting for Runs view to open");
}

/// Opens Runs when a workspace graph blocks the serve default (FR-2811).
fn open_runs_view(session: &mut ChromeSession) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if runs_panel_open(session) {
            return;
        }
        let _ = session.cdp.eval(
            "(() => { const b = document.querySelector('#runs-toggle'); if (b) { b.click(); } })()",
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out opening Runs view");
}

// TC-2811 / FR-2811: under serve, Runs toggle exists and view is open by default.
#[test]
#[ignore]
fn tc_2811_runs_view_default_under_serve() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2811: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new();
    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");

    wait_for_runs_open(&mut session);

    let toggle = session
        .cdp
        .eval("!!document.querySelector('#runs-toggle')")
        .expect("eval toggle");
    assert_eq!(toggle.as_bool(), Some(true), "runs toggle must exist");

    let visible = session
        .cdp
        .eval(
            "(() => { const p = document.querySelector('#runs'); return p && !p.hidden && p.classList.contains('open'); })()",
        )
        .expect("eval runs panel");
    assert_eq!(
        visible.as_bool(),
        Some(true),
        "Runs panel must be visible without clicking"
    );
}

// TC-2811 / FR-2811: soft default Runs view yields when a conversation snapshot arrives.
#[test]
#[ignore]
fn tc_2811_runs_default_yields_to_conversation() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2811_runs_default_yields_to_conversation: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new();
    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");
    wait_for_runs_open(&mut session);

    let snap = json!({
        "source": "tui",
        "workspace": fixture.workspace.display().to_string(),
        "agents": [],
        "session": {
            "conversation_id": "c1",
            "conversation_title": "t",
            "selected_agent": "demo",
            "details_expanded": false,
            "agents": [{
                "id": "demo",
                "label": "demo",
                "allowed": true,
                "busy": false,
                "modes": ["ask"],
                "mode": "ask",
                "models": [],
                "model": null,
                "effort": null,
                "models_loading": false
            }],
            "conversations": [],
            "transcript_omitted": 0,
            "transcript": [{
                "id": "c1:0",
                "role": "user",
                "agent": null,
                "content": "hello",
                "streaming": false
            }]
        }
    });
    let snap_js = serde_json::to_string(&snap).expect("snap json");
    session
        .cdp
        .eval(&format!(
            "document.dispatchEvent(new CustomEvent('dmuxai:snapshot', {{detail: {snap_js}}}))"
        ))
        .expect("dispatch conversation snapshot");

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(val) = session.cdp.eval(
            "(() => { const p = document.querySelector('#runs'); return !(p && !p.hidden && p.classList.contains('open')); })()",
        ) && val.as_bool() == Some(true)
        {
            return;
        }
    }
    panic!("Runs view still open after conversation snapshot");
}

// TC-2812 / FR-2812: list rows with status, origin, runner/model, stale flag.
#[test]
#[ignore]
fn tc_2812_runs_list_rows() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2812: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new();
    let runs = fixture.runs_dir();
    fs::create_dir_all(&runs).expect("runs root");

    let completed = "cccccccccccccccc";
    let failed = "dddddddddddddddd";
    let active = "eeeeeeeeeeeeeeee";

    let mut m_done = base_manifest(completed, 3_000, "completed");
    m_done["completed_at_ms"] = json!(3_100);
    m_done["duration_ms"] = json!(100);
    write_manifest(&runs.join(completed), &m_done);

    let mut m_fail = base_manifest(failed, 2_000, "failed");
    m_fail["completed_at_ms"] = json!(2_100);
    m_fail["duration_ms"] = json!(100);
    m_fail["dmuxai_exit_code"] = json!(1);
    write_manifest(&runs.join(failed), &m_fail);

    let stalled_id = "ffffffffffffffff";
    let mut m_active = base_manifest(active, 4_000, "active");
    m_active["pid"] = json!(999_999);
    write_manifest(&runs.join(active), &m_active);

    let mut m_stalled = base_manifest(stalled_id, 6_000, "active");
    m_stalled["pid"] = json!(std::process::id());
    write_manifest(&runs.join(stalled_id), &m_stalled);
    let stalled_progress = runs.join(stalled_id).join("progress.jsonl");
    fs::write(
        &stalled_progress,
        "{\"type\":\"progress\",\"ts_ms\":6000,\"kind\":\"started\",\"summary\":\"old\"}\n",
    )
    .expect("stalled progress");
    let old = FileTime::from_unix_time(
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs() as i64)
            - 120,
        0,
    );
    set_file_mtime(&stalled_progress, old).expect("stale progress mtime");

    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");
    wait_for_runs_open(&mut session);

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(n) = session
            .cdp
            .eval("document.querySelectorAll('.runs-row').length")
            && n.as_u64() == Some(4)
        {
            break;
        }
    }
    let row_count = session
        .cdp
        .eval("document.querySelectorAll('.runs-row').length")
        .expect("row count")
        .as_u64()
        .unwrap_or(0);
    assert_eq!(row_count, 4, "expected 4 run rows");

    let stale = session
        .cdp
        .eval("!!document.querySelector('[data-run-id=\"eeeeeeeeeeeeeeee\"] .runs-flag-stale')")
        .expect("stale badge");
    #[cfg(target_os = "linux")]
    assert_eq!(stale.as_bool(), Some(true), "stale badge on dead pid row");

    let classes = session
        .cdp
        .eval(
            "Array.from(document.querySelectorAll('.run-status')).map(el => el.className).join('|')",
        )
        .expect("status classes")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(classes.contains("run-status-completed"), "{classes}");
    assert!(classes.contains("run-status-failed"), "{classes}");
    assert!(classes.contains("run-status-active"), "{classes}");

    let runner_text = session
        .cdp
        .eval("document.querySelector('.runs-col-runner') && document.querySelector('.runs-col-runner').innerText")
        .expect("runner col")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        runner_text.contains("codex"),
        "expected runner text: {runner_text}"
    );

    let origins = session
        .cdp
        .eval(
            "Array.from(document.querySelectorAll('.runs-col-origin')).map(el => el.innerText).join('|')",
        )
        .expect("origins")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        origins.split('|').all(|o| o == "cli"),
        "expected cli origin labels: {origins}"
    );

    let stalled = session
        .cdp
        .eval(&format!(
            "!!document.querySelector('[data-run-id=\"{stalled_id}\"] .runs-flag-stalled')"
        ))
        .expect("stalled badge");
    assert_eq!(stalled.as_bool(), Some(true), "stalled badge on quiet run");

    let elapsed_before = session
        .cdp
        .eval(&format!(
            "document.querySelector('[data-run-id=\"{active}\"] .runs-col-elapsed') && document.querySelector('[data-run-id=\"{active}\"] .runs-col-elapsed').innerText"
        ))
        .expect("elapsed before")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        elapsed_before.contains(':'),
        "elapsed column must show M:SS: {elapsed_before}"
    );

    let tick_deadline = Instant::now() + Duration::from_secs(3);
    let mut elapsed_after = elapsed_before.clone();
    while Instant::now() < tick_deadline {
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(text) = session.cdp.eval(&format!(
            "document.querySelector('[data-run-id=\"{active}\"] .runs-col-elapsed').innerText"
        )) {
            let t = text.as_str().unwrap_or("").to_owned();
            if t != elapsed_before {
                elapsed_after = t;
                break;
            }
        }
    }
    assert_ne!(
        elapsed_before, elapsed_after,
        "active run elapsed should tick"
    );

    let late_id = "1111111111111111";
    let mut m_late = base_manifest(late_id, 7_000, "completed");
    m_late["completed_at_ms"] = json!(7_100);
    m_late["duration_ms"] = json!(100);
    write_manifest(&runs.join(late_id), &m_late);

    let poll_deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_fifth = false;
    while Instant::now() < poll_deadline {
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(n) = session
            .cdp
            .eval("document.querySelectorAll('.runs-row').length")
            && n.as_u64() == Some(5)
        {
            saw_fifth = true;
            break;
        }
    }
    assert!(
        saw_fifth,
        "poll should pick up a new run (2 s poll, 15 s deadline)"
    );
}

// TC-2813 / FR-2813: live stream, approval banner, graph nodes, exit code.
#[test]
#[ignore]
fn tc_2813_run_detail_live_and_approval() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2813: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new();
    write_graph_fixture(&fixture.workspace, "g1");
    let runs = fixture.runs_dir();
    let run_id = "aaaaaaaaaaaaaaaa";
    let mut manifest = base_manifest(run_id, 5_000, "active");
    manifest["pid"] = json!(std::process::id());
    write_manifest(&runs.join(run_id), &manifest);
    let progress_path = runs.join(run_id).join("progress.jsonl");
    fs::write(
        &progress_path,
        "{\"type\":\"progress\",\"ts_ms\":5000,\"kind\":\"started\",\"summary\":\"boot\"}\n",
    )
    .expect("progress");

    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");
    open_runs_view(&mut session);

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(n) = session
            .cdp
            .eval("document.querySelectorAll('.runs-row').length")
            && n.as_u64().unwrap_or(0) >= 1
        {
            break;
        }
    }

    session
        .cdp
        .eval(&format!(
            "document.querySelector('[data-run-id=\"{run_id}\"]').click()"
        ))
        .expect("select row");

    let boot_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < boot_deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(text) = session.cdp.eval(
            "document.querySelector('#runs-stream') && document.querySelector('#runs-stream').innerText",
        ) && text.as_str().unwrap_or("").contains("boot")
        {
            break;
        }
    }

    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&progress_path)
        .expect("open progress")
        .write_all(
            br#"{"type":"progress","ts_ms":6000,"kind":"tool","summary":"live line"}
"#,
        )
        .expect("append progress");

    let mut saw_live = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(text) = session.cdp.eval(
            "document.querySelector('#runs-stream') && document.querySelector('#runs-stream').innerText",
        ) {
            let t = text.as_str().unwrap_or("");
            if t.contains("live line") {
                saw_live = true;
                break;
            }
        }
    }
    assert!(saw_live, "expected live progress line in stream");

    let mut manifest = base_manifest(run_id, 5_000, "awaiting_approval");
    manifest["links"]["graph_id"] = json!("g1");
    manifest["completed_at_ms"] = json!(5_500);
    manifest["duration_ms"] = json!(500);
    manifest["dmuxai_exit_code"] = json!(7);
    manifest["end_summary"] = json!({ "plan_hash": "deadbeefdeadbeef" });
    write_manifest(&runs.join(run_id), &manifest);

    let mut saw_approval = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        if let Ok(text) = session.cdp.eval(
            "document.querySelector('#runs-approval-banner') && !document.querySelector('#runs-approval-banner').hidden && document.querySelector('#runs-approval-banner').innerText",
        ) {
            let t = text.as_str().unwrap_or("");
            if t.contains("--approve-plan-id") {
                saw_approval = true;
                break;
            }
        }
    }
    assert!(saw_approval, "approval banner with hint");

    let nodes = session
        .cdp
        .eval("document.querySelectorAll('#runs-graph-nodes li').length")
        .expect("graph nodes")
        .as_u64()
        .unwrap_or(0);
    assert!(nodes >= 1, "expected graph nodes listed");

    let exit = session
        .cdp
        .eval("document.querySelector('#runs-detail-header').innerText")
        .expect("header")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        exit.contains("Exit code: 7"),
        "expected exit code in header: {exit}"
    );
}

// TC-2814 / FR-2814: payload text is not interpreted as HTML.
#[test]
#[ignore]
fn tc_2814_run_text_is_not_html() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2814: chrome binary not found");
        return;
    }

    let xss = "<img src=x onerror=window.__pwned=1>";
    let fixture = ServeFixture::new();
    let runs = fixture.runs_dir();
    let run_id = "bbbbbbbbbbbbbbbb";
    let mut manifest = base_manifest(run_id, 1_000, "failed");
    manifest["completed_at_ms"] = json!(1_100);
    manifest["duration_ms"] = json!(100);
    manifest["dmuxai_exit_code"] = json!(1);
    manifest["error_summary"] = json!(xss);
    write_manifest(&runs.join(run_id), &manifest);
    let progress_path = runs.join(run_id).join("progress.jsonl");
    fs::create_dir_all(runs.join(run_id)).expect("run dir");
    fs::write(
        progress_path,
        format!(
            "{{\"type\":\"progress\",\"ts_ms\":1000,\"kind\":\"text\",\"summary\":{}}}\n",
            serde_json::to_string(xss).unwrap()
        ),
    )
    .expect("progress");

    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");
    wait_for_runs_open(&mut session);

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(n) = session
            .cdp
            .eval("document.querySelectorAll('.runs-row').length")
            && n.as_u64().unwrap_or(0) >= 1
        {
            break;
        }
    }

    session
        .cdp
        .eval(&format!(
            "document.querySelector('[data-run-id=\"{run_id}\"]').click()"
        ))
        .expect("select row");

    let text_deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_payload = false;
    while Instant::now() < text_deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(text) = session.cdp.eval(
            "document.querySelector('#runs-stream') && document.querySelector('#runs-stream').innerText",
        ) && text.as_str().unwrap_or("").contains(xss)
        {
            saw_payload = true;
            break;
        }
    }
    assert!(saw_payload, "stream must show literal payload text");

    let img = session
        .cdp
        .eval("document.querySelector('#runs img')")
        .expect("img query");
    assert!(img.is_null(), "must not inject img elements");

    let pwned = session.cdp.eval("window.__pwned").expect("pwned flag");
    assert!(pwned.is_null(), "onerror must not run");

    let body_text = session
        .cdp
        .eval("document.querySelector('#runs').innerText")
        .expect("runs text")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        body_text.contains(xss),
        "literal payload text must be visible"
    );
}

// TC-2817 / FR-2817: live graph metadata while active; approved continuation link.
#[test]
#[ignore]
fn tc_2817_live_graph_and_approved_link() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2817: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new();
    let runs = fixture.runs_dir();
    let active_id = "cccccccccccccccc";
    let continue_id = "dddddddddddddddd";
    let approved_id = "eeeeeeeeeeeeeeee";

    let mut manifest = base_manifest(active_id, 10_000, "active");
    manifest["pid"] = json!(std::process::id());
    manifest["resolved_runner"] = json!(null);
    manifest["resolved_model"] = json!(null);
    write_manifest(&runs.join(active_id), &manifest);

    let mut cont = base_manifest(continue_id, 12_000, "active");
    cont["pid"] = json!(std::process::id());
    write_manifest(&runs.join(continue_id), &cont);

    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");
    open_runs_view(&mut session);

    let list_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < list_deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(n) = session
            .cdp
            .eval("document.querySelectorAll('.runs-row').length")
            && n.as_u64().unwrap_or(0) >= 2
        {
            break;
        }
    }

    session
        .cdp
        .eval(&format!(
            "document.querySelector('[data-run-id=\"{active_id}\"]').click()"
        ))
        .expect("select active run");

    let stream_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < stream_deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(n) = session
            .cdp
            .eval("document.querySelectorAll('#runs-stream li').length")
            && n.as_u64().unwrap_or(0) >= 1
        {
            break;
        }
    }

    write_graph_fixture(&fixture.workspace, "g-live");
    let mut updated = base_manifest(active_id, 10_000, "active");
    updated["pid"] = json!(std::process::id());
    updated["resolved_runner"] = json!("cursor");
    updated["resolved_model"] = json!("composer-2.5");
    updated["links"]["graph_id"] = json!("g-live");
    write_manifest(&runs.join(active_id), &updated);

    let mut saw_graph = false;
    let graph_deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < graph_deadline {
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(n) = session
            .cdp
            .eval("document.querySelectorAll('#runs-graph-nodes li').length")
            && n.as_u64().unwrap_or(0) >= 1
        {
            saw_graph = true;
            break;
        }
    }
    assert!(saw_graph, "graph nodes should appear without reselecting");

    let header = session
        .cdp
        .eval("document.querySelector('#runs-detail-header').innerText")
        .expect("detail header")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        header.contains("cursor") && header.contains("composer-2.5"),
        "resolved runner/model in header: {header}"
    );

    let selected_val = session
        .cdp
        .eval("document.querySelector('.runs-row-selected')?.getAttribute('data-run-id')")
        .expect("selected row");
    let selected = selected_val.as_str().unwrap_or("");
    assert_eq!(selected, active_id, "selection must stay on active run");

    let mut approved = base_manifest(approved_id, 11_000, "approved");
    approved["completed_at_ms"] = json!(11_500);
    approved["duration_ms"] = json!(500);
    approved["dmuxai_exit_code"] = json!(7);
    approved["end_summary"] = json!({
        "plan_hash": "cafebabecafebabe",
        "approved_by_run_id": continue_id
    });
    write_manifest(&runs.join(approved_id), &approved);

    let mut saw_approved_row = false;
    let poll_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < poll_deadline {
        std::thread::sleep(Duration::from_millis(200));
        if let Ok(text) = session.cdp.eval(&format!(
            "document.querySelector('[data-run-id=\"{approved_id}\"] .run-status')?.textContent"
        )) && text.as_str().unwrap_or("") == "approved"
        {
            saw_approved_row = true;
            break;
        }
    }
    assert!(saw_approved_row, "approved status pill in list");

    session
        .cdp
        .eval(&format!(
            "document.querySelector('[data-run-id=\"{approved_id}\"]').click()"
        ))
        .expect("select approved run");

    let mut saw_continue = false;
    let approve_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < approve_deadline {
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(hidden) = session
            .cdp
            .eval("document.querySelector('#runs-approval-banner')?.hidden")
            && hidden.as_bool() == Some(false)
            && let Ok(text) = session
                .cdp
                .eval("document.querySelector('#runs-approval-banner').innerText")
        {
            let t = text.as_str().unwrap_or("");
            if t.contains("continued in run")
                && !t.contains("--approve-plan")
                && !t.contains("Waiting for approval")
            {
                saw_continue = true;
                break;
            }
        }
    }
    assert!(
        saw_continue,
        "approved continuation text, not approval hint"
    );

    let short = &continue_id[..8];
    session
        .cdp
        .eval("document.querySelector('.runs-approved-link')?.click()")
        .expect("click continuation link");

    let mut linked = false;
    let link_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < link_deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(sel) = session
            .cdp
            .eval("document.querySelector('.runs-row-selected')?.getAttribute('data-run-id')")
            && sel.as_str() == Some(continue_id)
        {
            linked = true;
            break;
        }
    }
    assert!(linked, "continuation link selects run {short}");
}
