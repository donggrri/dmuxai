//! TC-824 / FR-823, FR-824 (SPEC-running-indicators):
//! Headless Chrome renders the web dashboard with a running agent,
//! verifying the #run-status pill reads Running, the agent chip shows .spinner,
//! and the document title starts with ⏳.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use dmuxai::web::{
    AgentView, DashboardHub, DashboardServices, RunStateView, ServerConfig, SnapshotSource,
    start_server,
};

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

    let candidates = [
        "/opt/google/chrome/chrome",
        "/usr/bin/google-chrome",
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

fn base64_decode(input: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits = 0;
    for b in input.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' => continue,
            _ => continue,
        } as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
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
        frame.push(0x81);
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

    fn capture_screenshot(&mut self, path: &Path) {
        self.msg_id += 1;
        let id = self.msg_id;
        let msg = serde_json::json!({
            "id": id,
            "method": "Page.captureScreenshot",
            "params": {
                "format": "png"
            }
        })
        .to_string();

        if self.send(&msg).is_ok() {
            let mut buf = Vec::new();
            let mut temp = [0u8; 16384];
            let target_str = format!("\"id\":{id}");
            for _ in 0..100 {
                if let Ok(n) = self.stream.read(&mut temp)
                    && n > 0
                {
                    buf.extend_from_slice(&temp[..n]);
                    let text = String::from_utf8_lossy(&buf);
                    if text.contains(&target_str)
                        && let Some(data_pos) = text.find("\"data\":\"")
                    {
                        let after = &text[data_pos + 8..];
                        if let Some(quote) = after.find('"') {
                            let b64 = &after[..quote];
                            let bytes = base64_decode(b64);
                            if !bytes.is_empty() {
                                let _ = std::fs::write(path, bytes);
                                return;
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

#[tokio::test]
#[ignore = "needs Chrome/Chromium; run with --ignored"]
async fn running_indicator_renders_in_headless_chrome() {
    let chrome_bin = find_chrome_binary().expect(
        "Chrome or Chromium binary was not found on PATH or via CHROME_BIN; \
         install Google Chrome or Chromium to run this ignored e2e test",
    );

    let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    hub.publish(
        vec![AgentView {
            id: "codex".to_owned(),
            label: "Codex".to_owned(),
            busy: true,
            phase: Some("working".to_owned()),
            model: Some("gpt-4o".to_owned()),
            color: None,
            run: Some(RunStateView {
                state: "running".to_owned(),
                started_at_ms: Some(now_ms),
                ended_at_ms: None,
                duration_ms: None,
                node: None,
            }),
        }],
        None,
    );

    let token = "test-token-running".to_string();
    let config = ServerConfig {
        bind: "127.0.0.1".parse().unwrap(),
        port: 0,
        allow_remote: false,
        token: token.clone(),
        services: DashboardServices {
            events_path: None,
            todos: None,
            commands: None,
            mcp: None,
            web_attached: false,
            memory: None,
            docs: None,
        },
    };

    let server = start_server(config, hub).await.expect("start server");
    let url = server.url.clone();

    let user_data_dir = tempfile::tempdir().expect("temp chrome profile dir");

    let chrome_child = Command::new(&chrome_bin)
        .args([
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            &format!("--user-data-dir={}", user_data_dir.path().display()),
            "--window-size=1280,800",
            &url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn headless chrome");

    let _chrome_guard = ChildGuard(Some(chrome_child));

    let active_port_file = user_data_dir.path().join("DevToolsActivePort");
    let mut cdp_port = None;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(content) = std::fs::read_to_string(&active_port_file)
            && let Some(line) = content.lines().next()
            && let Ok(port) = line.trim().parse::<u16>()
        {
            cdp_port = Some(port);
            break;
        }
    }
    let cdp_port = cdp_port.expect("failed to obtain Chrome active CDP port");

    let mut page_ws_url = None;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
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
    let ws_path = {
        let without_scheme = page_ws_url
            .strip_prefix("ws://")
            .or_else(|| page_ws_url.strip_prefix("wss://"))
            .unwrap_or(&page_ws_url);
        let path_start = without_scheme
            .find('/')
            .expect("ws URL must contain a path component");
        &without_scheme[path_start..]
    };

    let mut cdp = CdpClient::connect(&format!("127.0.0.1:{cdp_port}"), ws_path)
        .expect("connect CDP websocket");

    // 1. Assert status pill #run-status reads Running
    let mut pill_text = String::new();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(val) = cdp.eval(
            "document.querySelector('#run-status') ? document.querySelector('#run-status').textContent : ''",
        ) && let Some(text) = val.as_str()
            && text.contains("Running")
        {
            pill_text = text.to_owned();
            break;
        }
    }
    assert!(
        pill_text.contains("Running"),
        "expected #run-status to contain 'Running', got: '{pill_text}'"
    );

    // 2. Assert agent chip has .spinner
    let mut has_chip_spinner = false;
    for _ in 0..20 {
        if let Ok(val) = cdp.eval("document.querySelector('#agents .chip .spinner') !== null")
            && val.as_bool() == Some(true)
        {
            has_chip_spinner = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        has_chip_spinner,
        "expected agent chip to contain a .spinner element"
    );

    // 3. Assert tab title starts with ⏳
    let title_val = cdp
        .eval("document.title")
        .expect("eval document.title")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        title_val.starts_with("⏳"),
        "expected document.title to start with '⏳', got: '{title_val}'"
    );

    // 4. Assert favicon href is set to running icon
    let favicon_href = cdp
        .eval("document.querySelector('#favicon') ? document.querySelector('#favicon').getAttribute('href') : ''")
        .expect("eval favicon href")
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        favicon_href.contains("favicon-running.svg"),
        "expected favicon href to contain favicon-running.svg, got: '{favicon_href}'"
    );

    // 5. Save a screenshot to target dir and assert it is created and non-empty
    let target_dir =
        PathBuf::from(std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_owned()));
    let screenshot_path = target_dir.join("running_indicator.png");
    cdp.capture_screenshot(&screenshot_path);
    assert!(
        screenshot_path.exists(),
        "expected screenshot to exist at {}",
        screenshot_path.display()
    );
    let meta = std::fs::metadata(&screenshot_path).expect("read screenshot metadata");
    assert!(
        meta.len() > 0,
        "expected screenshot file to be non-empty, got 0 bytes"
    );
}
