//! TC-705 / UX-702..UX-703 and TC-881/TC-882 (SPEC-graph-swimlane): headless
//! Chrome renders the swimlane timeline,
//! allows node inspection in the side panel, and persists the view mode choice
//! across reloads.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Kills and reaps child processes on all return and panic paths.
struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Locates a viable Chrome or Chromium binary on the system.
fn find_chrome_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CHROME_BIN") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    let candidates = [
        "/opt/google/chrome/chrome",
        "google-chrome",
        "chromium",
        "chromium-browser",
        "chrome",
    ];

    for candidate in candidates {
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

/// Writes a demo diamond graph plan (a -> b, a -> c, b -> d, c -> d)
/// for TC-705 / UX-700..UX-703.
fn write_diamond_fixture(workspace: &Path) {
    let graphs = workspace.join(".dmuxai").join("graphs");
    std::fs::create_dir_all(&graphs).expect("create graphs dir");
    let run = serde_json::json!({
        "plan": {
            "schema": "dmuxai.graph/v1",
            "id": "diamond-run",
            "orchestrator_model": "composer-2.5",
            "user_prompt": "build diamond pipeline",
            "limits": { "max_parallel": 2, "max_retries": 2 },
            "nodes": [
                {
                    "id": "a", "role": "plan", "job": "decompose work",
                    "runner": "codex", "model_id": "m", "skill": null,
                    "requires_gate": false, "status": "completed", "attempt": 1
                },
                {
                    "id": "b", "role": "implement", "job": "write frontend",
                    "runner": "agy", "model_id": "m", "skill": null,
                    "requires_gate": false, "status": "running", "attempt": 1
                },
                {
                    "id": "c", "role": "review", "job": "verify contracts",
                    "runner": "cursor", "model_id": "m", "skill": null,
                    "requires_gate": false, "status": "pending", "attempt": 0
                },
                {
                    "id": "d", "role": "implement", "job": "final integration",
                    "runner": "codex", "model_id": "m", "skill": null,
                    "requires_gate": false, "status": "pending", "attempt": 0
                }
            ],
            "edges": [
                { "from": "a", "to": "b" },
                { "from": "a", "to": "c" },
                { "from": "b", "to": "d" },
                { "from": "c", "to": "d" }
            ]
        },
        "conversation_id": "conv-diamond",
        "phase": "active",
        "waiting_gate_node": null
    });
    std::fs::write(
        graphs.join("diamond.json"),
        serde_json::to_vec_pretty(&run).expect("serialize diamond fixture"),
    )
    .expect("write diamond fixture");
}

fn http_get_json(authority: &str, path: &str) -> std::io::Result<serde_json::Value> {
    let mut stream = TcpStream::connect(authority)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    let mut temp = [0u8; 4096];
    let mut content_length: Option<usize> = None;

    for _ in 0..50 {
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
        let mut stream = TcpStream::connect(authority)?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;

        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes())?;

        let mut response = [0u8; 1024];
        let n = stream.read(&mut response)?;
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
        frame.push(0x81); // FIN + text opcode
        if len < 126 {
            frame.push(0x80 | (len as u8));
        } else if len <= 0xffff {
            frame.push(0x80 | 126);
            frame.push(((len >> 8) & 0xff) as u8);
            frame.push((len & 0xff) as u8);
        } else {
            frame.push(0x80 | 127);
            for i in (0..8).rev() {
                frame.push(((len >> (i * 8)) & 0xff) as u8);
            }
        }
        // Mask with 4 zeros: payload bytes are unchanged
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

        for _ in 0..100 {
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

// TC-705 / UX-700..UX-703: headless Chrome renders the SVG subway map,
// allows node inspection, and persists view mode across reloads.
#[test]
#[ignore]
fn graph_view_renders_in_headless_chrome() {
    let chrome_bin = match find_chrome_binary() {
        Some(bin) => bin,
        None => {
            println!("skipping graph_view_renders_in_headless_chrome: chrome binary not found");
            return;
        }
    };

    let workspace = tempfile::tempdir().expect("temp workspace");
    write_diamond_fixture(workspace.path());

    // 1. Spawn dmuxai serve --port 0
    let mut serve_child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace.path())
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dmuxai serve");

    let stdout = serve_child.stdout.take().expect("stdout is piped");
    let _serve_guard = ChildGuard(Some(serve_child));

    let (line_tx, line_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let result = reader.read_line(&mut line);
        let _ = line_tx.send(result.map(|_| line));
    });

    let first_line = match line_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(line)) => line,
        Ok(Err(error)) => panic!("failed to read stdout: {error}"),
        Err(error) => panic!("timed out waiting for the dashboard URL: {error}"),
    };

    let url = first_line
        .trim()
        .strip_prefix("dmuxai dashboard: ")
        .unwrap_or_else(|| panic!("unexpected first stdout line: {first_line:?}"));

    // Find a free TCP port for Chrome remote debugging
    let cdp_port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind free port for CDP");
        listener.local_addr().expect("local addr").port()
    };

    let user_data_dir = tempfile::tempdir().expect("temp chrome profile dir");

    // 2. Spawn headless Chrome pointing to the dashboard URL
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
        .expect("spawn headless chrome");

    let _chrome_guard = ChildGuard(Some(chrome_child));

    // Wait for Chrome remote debugging port to become responsive
    let mut page_ws_url = None;
    for _ in 0..50 {
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

    let page_ws_url = page_ws_url.expect("failed to obtain Chrome WebSocket debugger URL");
    let ws_path = page_ws_url
        .split(&format!("{cdp_port}"))
        .nth(1)
        .expect("valid ws path");

    let mut cdp = CdpClient::connect(&format!("127.0.0.1:{cdp_port}"), ws_path)
        .expect("connect CDP websocket");

    // 3. Wait for the graph SVG to render (nodes loaded from /api/state or SSE)
    let mut nodes_count = 0;
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(val) = cdp.eval("document.querySelectorAll('svg g.node').length")
            && let Some(n) = val.as_u64()
            && n == 4
        {
            nodes_count = n;
            break;
        }
    }

    assert_eq!(
        nodes_count, 4,
        "expected 4 svg g.node elements in diamond graph"
    );

    let edges_count = cdp
        .eval("document.querySelectorAll('svg path.edge').length")
        .expect("eval edge count")
        .as_u64()
        .unwrap_or(0);
    assert_eq!(
        edges_count, 4,
        "expected 4 svg path.edge elements for 4 dependencies"
    );

    // 4. Clicking a node opens the side panel (UX-701)
    let click_res = cdp.eval(
        "document.querySelector(\"svg g.node[data-id='b']\").dispatchEvent(new MouseEvent('click', { bubbles: true }))"
    );
    assert!(click_res.is_ok(), "failed to dispatch click on node b");

    let detail_hidden = cdp
        .eval("document.querySelector('#graph-detail').hidden")
        .expect("eval detail hidden")
        .as_bool()
        .unwrap_or(true);
    assert!(
        !detail_hidden,
        "expected #graph-detail to be visible after clicking node"
    );

    let detail_text = cdp
        .eval("document.querySelector('#graph-detail').innerText")
        .expect("eval detail text")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        detail_text.contains('b'),
        "expected detail text to reference node b: {detail_text}"
    );
    assert!(
        detail_text.contains("AGY") || detail_text.contains("agy"),
        "expected detail text to show runner AGY: {detail_text}"
    );

    let esc_res = cdp.eval(
        "document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))",
    );
    assert!(esc_res.is_ok(), "failed to dispatch Escape");
    let detail_hidden_after_esc = cdp
        .eval("document.querySelector('#graph-detail').hidden")
        .expect("eval detail hidden after Esc")
        .as_bool()
        .unwrap_or(false);
    assert!(
        detail_hidden_after_esc,
        "expected #graph-detail to be hidden after Escape"
    );

    // 5. List/Graph toggle switches view mode and persists across reloads (UX-702)
    let toggle_click = cdp.eval("document.querySelector('#view-list').click()");
    assert!(toggle_click.is_ok(), "failed to click view-list button");

    let stored_pref = cdp
        .eval("window.localStorage.getItem('dmuxai.graphView')")
        .expect("eval localStorage")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert_eq!(
        stored_pref, "list",
        "expected localStorage to persist 'list' preference"
    );

    // Reload the page and verify persisted mode is restored
    let reload_res = cdp.eval("location.reload()");
    assert!(reload_res.is_ok(), "failed to trigger location.reload()");

    let mut restored = false;
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        let list_on = cdp
            .eval("document.querySelector('#view-list') && document.querySelector('#view-list').classList.contains('on')")
            .unwrap_or(serde_json::Value::Bool(false))
            .as_bool()
            .unwrap_or(false);
        let stage_hidden = cdp
            .eval("document.querySelector('#graph-stage') && document.querySelector('#graph-stage').hidden")
            .unwrap_or(serde_json::Value::Bool(false))
            .as_bool()
            .unwrap_or(false);

        if list_on && stage_hidden {
            restored = true;
            break;
        }
    }

    assert!(
        restored,
        "expected list view preference to persist and remain active after reload"
    );
}

/// A running `dmuxai serve` plus headless Chrome on its dashboard.
struct Dashboard {
    cdp: CdpClient,
    _serve: ChildGuard,
    _chrome: ChildGuard,
    _profile: tempfile::TempDir,
}

fn start_dashboard(chrome_bin: &Path, workspace: &Path, width: u32, height: u32) -> Dashboard {
    let mut serve_child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace)
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dmuxai serve");
    let stdout = serve_child.stdout.take().expect("stdout is piped");
    let serve = ChildGuard(Some(serve_child));
    let (line_tx, line_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let result = reader.read_line(&mut line);
        let _ = line_tx.send(result.map(|_| line));
    });
    let first_line = line_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("dashboard URL line")
        .expect("read stdout");
    let url = first_line
        .trim()
        .strip_prefix("dmuxai dashboard: ")
        .unwrap_or_else(|| panic!("unexpected first stdout line: {first_line:?}"))
        .to_owned();

    let cdp_port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind free port for CDP");
        listener.local_addr().expect("local addr").port()
    };
    let profile = tempfile::tempdir().expect("temp chrome profile dir");
    let chrome_child = Command::new(chrome_bin)
        .args([
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            &format!("--remote-debugging-port={cdp_port}"),
            &format!("--user-data-dir={}", profile.path().display()),
            &format!("--window-size={width},{height}"),
            &url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn headless chrome");
    let chrome = ChildGuard(Some(chrome_child));

    let mut page_ws_url = None;
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(pages) = http_get_json(&format!("127.0.0.1:{cdp_port}"), "/json/list")
            && let Some(arr) = pages.as_array()
        {
            page_ws_url = arr.iter().find_map(|page| {
                (page.get("type").and_then(|t| t.as_str()) == Some("page"))
                    .then(|| page.get("webSocketDebuggerUrl").and_then(|w| w.as_str()))
                    .flatten()
                    .map(str::to_owned)
            });
        }
        if page_ws_url.is_some() {
            break;
        }
    }
    let page_ws_url = page_ws_url.expect("Chrome WebSocket debugger URL");
    let ws_path = page_ws_url
        .split(&format!("{cdp_port}"))
        .nth(1)
        .expect("valid ws path")
        .to_owned();
    let cdp = CdpClient::connect(&format!("127.0.0.1:{cdp_port}"), &ws_path)
        .expect("connect CDP websocket");
    Dashboard {
        cdp,
        _serve: serve,
        _chrome: chrome,
        _profile: profile,
    }
}

/// Evaluates `expr` (which must return a JSON string) and parses it.
fn eval_json(cdp: &mut CdpClient, expr: &str) -> serde_json::Value {
    let raw = cdp.eval(expr).expect("CDP eval");
    let text = raw
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON string from {expr}: {raw}"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("bad JSON {text}: {e}"))
}

fn wait_for(cdp: &mut CdpClient, expr: &str) -> bool {
    for _ in 0..80 {
        std::thread::sleep(Duration::from_millis(100));
        if cdp.eval(expr).ok().and_then(|v| v.as_bool()) == Some(true) {
            return true;
        }
    }
    false
}

/// The SPEC-graph-swimlane demo run: 8 nodes on 4 runners, 9 minutes in, one
/// running, one failed, three pending (one blocked by the failure).
fn write_swimlane_fixture(workspace: &Path) {
    let graphs = workspace.join(".dmuxai").join("graphs");
    std::fs::create_dir_all(&graphs).expect("create graphs dir");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let at = |min_since_start: f64| now - ((9.0 - min_since_start) * 60_000.0) as u64;
    let node = |id: &str,
                role: &str,
                runner: &str,
                model: &str,
                status: &str,
                attempt: u32,
                times: Option<(f64, Option<f64>)>| {
        let mut n = serde_json::json!({
            "id": id, "role": role, "job": format!("{id} step"), "runner": runner,
            "model_id": model, "skill": null, "requires_gate": false,
            "status": status, "attempt": attempt
        });
        if let Some((start, end)) = times {
            n["started_at_ms"] = at(start).into();
            if let Some(end) = end {
                n["finished_at_ms"] = at(end).into();
            }
        }
        n
    };
    let nodes = vec![
        node(
            "plan",
            "plan",
            "agy",
            "gemini-3.8-flash-high",
            "completed",
            1,
            Some((0.0, Some(2.0))),
        ),
        node(
            "spec",
            "draft",
            "opencode-haiku",
            "claude-haiku-5.5",
            "completed",
            1,
            Some((2.0, Some(4.6))),
        ),
        node(
            "impl-core",
            "implement",
            "agy",
            "gemini-3.8-flash-high",
            "running",
            1,
            Some((4.6, None)),
        ),
        node(
            "impl-api",
            "implement",
            "opencode-deepseek",
            "deepseek-v4.1-flash",
            "completed",
            1,
            Some((4.6, Some(8.0))),
        ),
        node(
            "impl-ui",
            "implement",
            "cursor",
            "composer-2.5",
            "failed",
            2,
            Some((4.6, Some(6.4))),
        ),
        node(
            "review",
            "review",
            "opencode-haiku",
            "claude-haiku-5.5",
            "pending",
            0,
            None,
        ),
        node(
            "test",
            "test",
            "opencode-deepseek",
            "deepseek-v4.1-flash",
            "pending",
            0,
            None,
        ),
        node(
            "wrapup",
            "draft",
            "agy",
            "gemini-3.8-flash-high",
            "pending",
            0,
            None,
        ),
    ];
    let edges: Vec<serde_json::Value> = [
        ("plan", "spec"),
        ("spec", "impl-core"),
        ("spec", "impl-api"),
        ("spec", "impl-ui"),
        ("impl-core", "review"),
        ("impl-api", "review"),
        ("impl-ui", "review"),
        ("review", "test"),
        ("test", "wrapup"),
    ]
    .iter()
    .map(|(from, to)| serde_json::json!({ "from": from, "to": to }))
    .collect();
    let run = serde_json::json!({
        "plan": {
            "schema": "dmuxai.graph/v1",
            "id": "demo-csv-export",
            "orchestrator_model": "gemini-3.8-flash-high",
            "user_prompt": "Add CSV export to the reports page",
            "limits": { "max_parallel": 3, "max_retries": 2 },
            "nodes": nodes,
            "edges": edges
        },
        "conversation_id": "conv-demo",
        "phase": "active",
        "waiting_gate_node": null
    });
    std::fs::write(
        graphs.join("demo-csv-export.json"),
        serde_json::to_vec_pretty(&run).expect("serialize swimlane fixture"),
    )
    .expect("write swimlane fixture");
}

// TC-881 (SPEC-graph-swimlane FR-882, FR-884, UX-880): estimate, tick-step,
// arrow-routing and light-theme contrast math, run in the real page.
#[test]
#[ignore]
fn graph_swimlane_math_in_headless_chrome() {
    let Some(chrome_bin) = find_chrome_binary() else {
        println!("skipping graph_swimlane_math_in_headless_chrome: chrome binary not found");
        return;
    };
    let workspace = tempfile::tempdir().expect("temp workspace");
    write_swimlane_fixture(workspace.path());
    let mut dash = start_dashboard(&chrome_bin, workspace.path(), 1280, 900);
    assert!(
        wait_for(
            &mut dash.cdp,
            "!!(window.dmuxaiGraph && document.querySelector('.graph-svg g.node'))"
        ),
        "swimlane did not render"
    );

    let result = eval_json(
        &mut dash.cdp,
        r#"(() => {
  const G = window.dmuxaiGraph;
  const now = 100000;
  const graph = { phase: 'active', edges: [], nodes: [
    { id: 'a', role: 'plan', runner: 'agy', status: 'completed', started_at_ms: 0, finished_at_ms: 60000, deps: [] },
    { id: 'b', role: 'implement', runner: 'agy', status: 'running', started_at_ms: 60000, deps: ['a'] },
    { id: 'c', role: 'review', runner: 'cursor', status: 'pending', deps: ['b'] },
    { id: 'd', role: 'test', runner: 'cursor', status: 'pending', deps: [] },
    { id: 'e', role: 'test', runner: 'codex', status: 'failed', started_at_ms: 0, finished_at_ms: 30000, deps: [] },
    { id: 'f', role: 'review', runner: 'codex', status: 'pending', deps: ['e'] }
  ] };
  const est = G.estimate(graph, now);
  const s = { x: 200, y: 54, w: 200, h: 56 };
  const t = { x: 380, y: 158, w: 160, h: 56 };
  const short = G.arrowPath(s, t);
  const pts = (short.match(/-?[0-9.]+,-?[0-9.]+/g) || []).map(p => p.split(',').map(Number));
  const inside = pts.slice(1).some(([x, y]) => x > s.x + 1 && x < s.x + s.w - 1 && y > s.y + 1 && y < s.y + s.h - 1);
  const wide = G.arrowPath(s, { x: 460, y: 158, w: 160, h: 56 });
  document.querySelector('#theme-select').value = 'light';
  document.querySelector('#theme-select').dispatchEvent(new Event('change', { bubbles: true }));
  const css = getComputedStyle(document.documentElement);
  const v = name => css.getPropertyValue(name).trim();
  const bgs = ['--graph-canvas', '--graph-node-fill', '--bg-chip'].map(v);
  const pending = Math.min(...bgs.map(b => G.contrast(v('--graph-pending'), b)));
  const text = Math.min(...['--text', '--muted', '--faint'].flatMap(f => bgs.map(b => G.contrast(v(f), b))));
  return JSON.stringify({
    theme: document.documentElement.dataset.theme,
    dStart: est.d.start, dEstimated: est.d.estimated,
    bPlanEnd: est.b.planEnd, bEnd: est.b.end,
    cStart: est.c.start, cDur: est.c.estDurMs,
    fBlocked: est.f.blocked, fStart: est.f.start, cBlocked: est.c.blocked,
    step14h: G.tickStep(14 * 3600000), ticks14h: Math.floor(14 * 3600000 / G.tickStep(14 * 3600000)) + 1,
    step2m: G.tickStep(120000),
    shortPath: short, shortHasCurve: short.indexOf('C') !== -1, shortInside: inside,
    widePath: wide,
    pending: pending, text: text
  });
})()"#,
    );
    assert_eq!(result["theme"], "light");
    // A dependency-less pending node starts at now.
    assert_eq!(result["dStart"], 100000);
    assert_eq!(result["dEstimated"], true);
    // Running b: the card ends at now; dependents wait for start + estimate
    // (median of all finished durations, 60s and 30s -> 45s).
    assert_eq!(result["bEnd"], 100000);
    assert_eq!(result["bPlanEnd"], 105000);
    assert_eq!(result["cStart"], 105000);
    assert_eq!(result["cDur"], 45000);
    // After a failed dependency: blocked, still estimated from now.
    assert_eq!(result["fBlocked"], true);
    assert_eq!(result["fStart"], 100000);
    assert_eq!(result["cBlocked"], false);
    // Tick steps: <= 10 ticks for a 14h run, 30s steps for short runs.
    assert!(result["ticks14h"].as_u64().unwrap() <= 11, "{result}");
    assert_eq!(result["step14h"], 7_200_000);
    assert_eq!(result["step2m"], 30_000);
    // Short gap: orthogonal route that never re-enters the source card.
    assert_eq!(result["shortHasCurve"], false, "{result}");
    assert_eq!(result["shortInside"], false, "{result}");
    assert!(
        result["widePath"].as_str().unwrap().contains('C'),
        "{result}"
    );
    // UX-880: light-theme contrast.
    assert!(result["pending"].as_f64().unwrap() >= 3.0, "{result}");
    assert!(result["text"].as_f64().unwrap() >= 4.5, "{result}");
}

// TC-882 (SPEC-graph-swimlane FR-880..FR-885, UX-881): the demo run renders as
// four lanes with a now line, a running tail, estimates on pending cards and
// arrows that never pass back over their source; the drawer overlays the
// timeline; Fit at 900px keeps every text at 11px or more.
#[test]
#[ignore]
fn graph_swimlane_renders_in_headless_chrome() {
    let Some(chrome_bin) = find_chrome_binary() else {
        println!("skipping graph_swimlane_renders_in_headless_chrome: chrome binary not found");
        return;
    };
    let workspace = tempfile::tempdir().expect("temp workspace");
    write_swimlane_fixture(workspace.path());
    let mut dash = start_dashboard(&chrome_bin, workspace.path(), 900, 1000);
    assert!(
        wait_for(
            &mut dash.cdp,
            "document.querySelectorAll('.graph-svg g.node').length === 8"
        ),
        "expected 8 cards"
    );

    let layout = eval_json(
        &mut dash.cdp,
        r#"(() => {
  const lanes = [...document.querySelectorAll('.graph-svg g.lane')].map(l => l.getAttribute('data-runner'));
  const cards = [...document.querySelectorAll('.graph-svg g.node')].map(g => {
    const m = g.getAttribute('transform').match(/translate\(([-0-9.]+),([-0-9.]+)\)/);
    return { id: g.getAttribute('data-id'), runner: g.getAttribute('data-runner'),
      lane: Number(g.getAttribute('data-lane')), x: Number(m[1]), y: Number(m[2]),
      w: Number(g.querySelector('.card-box').getAttribute('width')), h: 56,
      sub: (g.querySelector('.card-sub') || {}).textContent || '',
      tail: !!g.querySelector('.card-tail') };
  });
  const byId = Object.fromEntries(cards.map(c => [c.id, c]));
  const loopBacks = [...document.querySelectorAll('.graph-svg path.edge')].filter(p => {
    const s = byId[p.getAttribute('data-from')];
    const pts = (p.getAttribute('d').match(/-?[0-9.]+,-?[0-9.]+/g) || []).map(q => q.split(',').map(Number));
    return pts.slice(1).some(([x, y]) => x > s.x + 1 && x < s.x + s.w - 1 && y > s.y + 1 && y < s.y + s.h - 1);
  }).map(p => p.getAttribute('data-from') + '->' + p.getAttribute('data-to'));
  const now = document.querySelector('.graph-svg .now-line .now-text');
  return JSON.stringify({ lanes, cards, loopBacks,
    edges: document.querySelectorAll('.graph-svg path.edge').length,
    now: now ? now.textContent : '' });
})()"#,
    );
    let lanes: Vec<String> = serde_json::from_value(layout["lanes"].clone()).unwrap();
    assert_eq!(
        lanes,
        ["agy", "opencode-haiku", "opencode-deepseek", "cursor"],
        "lanes in first-use order"
    );
    let cards = layout["cards"].as_array().unwrap();
    let card = |id: &str| cards.iter().find(|c| c["id"] == id).unwrap().clone();
    for c in cards {
        let lane = c["lane"].as_u64().unwrap() as usize;
        assert_eq!(
            lanes[lane],
            c["runner"].as_str().unwrap(),
            "card in its lane: {c}"
        );
    }
    // Time order inside the AGY lane.
    let x = |id: &str| card(id)["x"].as_f64().unwrap();
    assert!(x("plan") < x("impl-core") && x("impl-core") < x("wrapup"));
    assert!(card("impl-core")["tail"].as_bool().unwrap(), "running tail");
    for id in ["review", "test", "wrapup"] {
        assert!(
            card(id)["sub"]
                .as_str()
                .unwrap()
                .starts_with("est. start +"),
            "{id} shows its estimated start"
        );
    }
    assert_eq!(layout["edges"], 9);
    assert_eq!(layout["loopBacks"], serde_json::json!([]), "{layout}");
    assert!(
        layout["now"].as_str().unwrap().starts_with("now "),
        "{layout}"
    );

    // FR-885: the drawer overlays the timeline without changing its width.
    let before = dash
        .cdp
        .eval("document.querySelector('.graph-svg').getBoundingClientRect().width")
        .unwrap()
        .as_f64()
        .unwrap();
    dash.cdp
        .eval("document.querySelector(\".graph-svg g.node[data-id='impl-core']\").dispatchEvent(new MouseEvent('click', { bubbles: true }))")
        .unwrap();
    assert!(wait_for(
        &mut dash.cdp,
        "!document.querySelector('#graph-detail').hidden"
    ));
    let after = dash
        .cdp
        .eval("document.querySelector('.graph-svg').getBoundingClientRect().width")
        .unwrap()
        .as_f64()
        .unwrap();
    assert!(
        (before - after).abs() < 1.0,
        "svg width {before} -> {after}"
    );
    let overlay = dash
        .cdp
        .eval("getComputedStyle(document.querySelector('#graph-detail')).position")
        .unwrap();
    assert_eq!(overlay, "absolute");
    dash.cdp
        .eval("document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))")
        .unwrap();
    assert!(wait_for(
        &mut dash.cdp,
        "document.querySelector('#graph-detail').hidden"
    ));

    // UX-881: after Fit at 900px every graph text renders at >= 11px.
    dash.cdp
        .eval("document.querySelector('#graph-fit').click()")
        .unwrap();
    let smallest = dash
        .cdp
        .eval("Math.min(...[...document.querySelectorAll('.graph-svg text')].map(t => parseFloat(getComputedStyle(t).fontSize) * t.getScreenCTM().a))")
        .unwrap()
        .as_f64()
        .unwrap();
    assert!(smallest >= 10.99, "smallest rendered text {smallest}px");
}
