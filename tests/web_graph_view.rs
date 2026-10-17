//! TC-705 / UX-700..UX-703: headless Chrome renders the SVG subway map,
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
