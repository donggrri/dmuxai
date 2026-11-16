//! Browser tests for SPEC-dashboard-tab-trim (FR-2820–FR-2822, FR-2825, FR-2828).
//! Needs Chrome/Chromium, so the tests are `#[ignore]`d. Every wait is bounded.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
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
    fn new_with_config(config: &str) -> Self {
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
        if !config.is_empty() {
            fs::write(ws_path.join("dmuxai.toml"), config).expect("dmuxai.toml");
        }

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

    fn dispatch_mouse_click(&mut self, x: f64, y: f64) -> Result<(), String> {
        for event_type in ["mousePressed", "mouseReleased"] {
            self.msg_id += 1;
            let id = self.msg_id;
            let msg = serde_json::json!({
                "id": id,
                "method": "Input.dispatchMouseEvent",
                "params": {
                    "type": event_type,
                    "x": x,
                    "y": y,
                    "buttons": 1,
                    "button": "left",
                    "clickCount": 1
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
                        if text.contains(&target_str) {
                            break;
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
        }
        Ok(())
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

fn wait_for_truthy(session: &mut ChromeSession, expr: &str, what: &str, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(v) = session.cdp.eval(expr)
            && v.as_bool() == Some(true)
        {
            return;
        }
    }
    panic!("timed out waiting for {what}");
}

fn click_until_truthy(
    session: &mut ChromeSession,
    click_js: &str,
    cond_js: &str,
    what: &str,
    secs: u64,
) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let _ = session.cdp.eval(click_js);
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(v) = session.cdp.eval(cond_js)
            && v.as_bool() == Some(true)
        {
            return;
        }
    }
    panic!("timed out waiting for {what}");
}

// TC-2825 / FR-2825: Reset layout restores the snapshot accent and the sidebar
// width and open/closed defaults (the same as double-clicking each handle).
#[test]
#[ignore]
fn tc_2825_reset_layout() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2825_reset_layout: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new_with_config("[web]\naccent = \"#3366ff\"\n");
    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");

    wait_for_truthy(
        &mut session,
        "!!document.getElementById('accent-reset')",
        "top bar reset button",
        10,
    );
    // The configured accent arrives with the snapshot.
    wait_for_truthy(
        &mut session,
        "getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() === '#3366ff'",
        "snapshot accent applied",
        10,
    );

    // Change accent and sidebar layout away from defaults (persist width via
    // storage so reload picks it up; CDP synthetic drags are unreliable).
    session
        .cdp
        .eval("setAccentChoice('#ff0000')")
        .expect("set accent");
    session
        .cdp
        .eval(
            "(() => { localStorage.setItem('dmuxai.sidebar.width.left', '480'); location.reload(); })()",
        )
        .expect("persist wider left sidebar");
    wait_for_truthy(
        &mut session,
        "!!document.getElementById('accent-reset')",
        "dashboard after reload",
        15,
    );
    wait_for_truthy(
        &mut session,
        "document.body.classList.contains('left-sidebar-open')",
        "sessions sidebar open after reload",
        10,
    );
    click_until_truthy(
        &mut session,
        "(() => { if (document.body.classList.contains('left-sidebar-open')) document.getElementById('sessions-toggle').click(); })()",
        "!document.body.classList.contains('left-sidebar-open')",
        "sessions sidebar closed",
        20,
    );
    click_until_truthy(
        &mut session,
        "(() => { if (!document.body.classList.contains('sidebar-open')) document.getElementById('workspace-toggle').click(); })()",
        "document.body.classList.contains('sidebar-open')",
        "files sidebar open",
        20,
    );
    wait_for_truthy(
        &mut session,
        "(() => { const v = getComputedStyle(document.documentElement).getPropertyValue('--left-sidebar-width').trim(); \
           return v !== '320px' && !document.body.classList.contains('left-sidebar-open') && document.body.classList.contains('sidebar-open'); })()",
        "layout changed away from defaults",
        10,
    );
    wait_for_truthy(
        &mut session,
        "getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() === '#ff0000'",
        "override accent applied",
        5,
    );

    click_until_truthy(
        &mut session,
        "(() => { const b = document.getElementById('accent-reset'); if (b) b.click(); })()",
        "(() => localStorage.getItem('dmuxai.accent') === null && getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() === '#3366ff')()",
        "accent reset",
        20,
    );

    // Accent back to the configured (snapshot) accent...
    // ...sidebar widths and open/closed back to the defaults.
    wait_for_truthy(
        &mut session,
        "(() => { const l = getComputedStyle(document.documentElement).getPropertyValue('--left-sidebar-width').trim(); \
           const r = getComputedStyle(document.documentElement).getPropertyValue('--right-sidebar-width').trim(); \
           return l === '320px' && r === '320px' \
             && document.body.classList.contains('left-sidebar-open') \
             && !document.body.classList.contains('sidebar-open'); })()",
        "layout reset",
        10,
    );
    let stored = session
        .cdp
        .eval("localStorage.getItem('dmuxai.sidebar.width.left')")
        .expect("stored left width");
    assert!(
        stored.is_null(),
        "stored left width must be cleared after reset"
    );
}

const TC_2828_TAIL: &str = "__TC2828_SECRET_TAIL_BEYOND_PREVIEW__";

fn tc_2828_full_prompt() -> String {
    let mut full = "Current rules prefix for TC-2828.".to_string();
    while full.len() < 240 {
        full.push('x');
    }
    full.push_str(TC_2828_TAIL);
    full
}

// TC-2828 / FR-2828: configured prompt shows preview in placeholder, not value.
#[test]
#[ignore]
fn tc_2828_system_prompt_preview_placeholder() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2828_system_prompt_preview_placeholder: chrome binary not found");
        return;
    }

    let full_prompt = tc_2828_full_prompt();
    let preview_200: String = full_prompt.chars().take(200).collect();
    let escaped = full_prompt.replace('\\', "\\\\").replace('\"', "\\\"");
    let config = format!("[runners.codex]\nsystem_prompt = \"{escaped}\"\n");
    let fixture = ServeFixture::new_with_config(&config);
    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");

    click_until_truthy(
        &mut session,
        "(() => { const m = document.getElementById('more-menu'); if (m && m.hidden) document.getElementById('more-toggle').click(); const b = document.getElementById('agent-prompts-toggle'); if (b) b.click(); })()",
        "(() => { const p = document.getElementById('agent-prompts-popover'); return !!p && !p.hidden; })()",
        "system prompts popover",
        20,
    );
    wait_for_truthy(
        &mut session,
        "!!document.querySelector('.agent-prompt-row[data-runner=\"codex\"][data-set=\"true\"]')",
        "codex prompt row",
        15,
    );

    let value = session
        .cdp
        .eval("document.querySelector('.agent-prompt-row[data-runner=\"codex\"] textarea').value")
        .expect("textarea value");
    assert_eq!(value.as_str().unwrap_or(""), "", "textarea must stay empty");

    let placeholder = session
        .cdp
        .eval(
            "document.querySelector('.agent-prompt-row[data-runner=\"codex\"] textarea').placeholder",
        )
        .expect("textarea placeholder");
    let ph = placeholder.as_str().unwrap_or("");
    assert!(
        ph.contains("Current:"),
        "placeholder must show Current: prefix, got {ph:?}"
    );
    assert!(
        ph.contains(&preview_200),
        "placeholder must include API preview, got {ph:?}"
    );

    let preview_in_dom = session
        .cdp
        .eval(
            "document.querySelector('.agent-prompt-row[data-runner=\"codex\"] .agent-prompt-preview').textContent",
        )
        .expect("preview span");
    assert_eq!(
        preview_in_dom.as_str().unwrap_or(""),
        preview_200,
        "preview field matches truncated prompt"
    );
    let tail_in_dom = session
        .cdp
        .eval(&format!(
            "(() => {{ const row = document.querySelector('.agent-prompt-row[data-runner=\"codex\"]'); \
              const pop = document.getElementById('agent-prompts-popover'); \
              const hay = (row && row.innerText ? row.innerText : '') + (pop && pop.innerText ? pop.innerText : ''); \
              return hay.includes({tail:?}); }})()",
            tail = TC_2828_TAIL
        ))
        .expect("tail dom scan");
    assert_eq!(
        tail_in_dom.as_bool(),
        Some(false),
        "full prompt tail must not appear in the System prompts UI"
    );
}

// TC-2820 / FR-2820: top bar shows exactly Runs, Diff, Chat, Workspace, More.
#[test]
#[ignore]
fn tc_2820_top_bar_controls() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2820_top_bar_controls: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new_with_config("");
    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");

    wait_for_truthy(
        &mut session,
        "!!document.getElementById('runs-toggle')",
        "runs toggle",
        15,
    );
    let labels = session
        .cdp
        .eval(
            "(() => [...document.querySelectorAll('nav.top-views > button, nav.top-views > .more-menu-wrap > button')]
              .map(b => b.textContent.replace(/\\s+/g, ' ').trim().replace(/\\s\\d+$/, '')))()",
        )
        .expect("top bar labels");
    let arr = labels.as_array().expect("array");
    assert_eq!(arr.len(), 5, "expected five top-level controls");
    assert_eq!(arr[0].as_str(), Some("Runs"));
    assert_eq!(arr[1].as_str(), Some("Diff"));
    assert_eq!(arr[2].as_str(), Some("Chat"));
    assert_eq!(arr[3].as_str(), Some("Workspace"));
    assert_eq!(arr[4].as_str(), Some("More"));

    let stray = session
        .cdp
        .eval(
            "(() => !document.querySelector('header.top > #sessions-toggle') \
              && !document.querySelector('header.top > #memory-toggle') \
              && !document.querySelector('header.top > #theme-select'))()",
        )
        .expect("no stray controls");
    assert_eq!(
        stray.as_bool(),
        Some(true),
        "secondary controls not in top bar"
    );
}

// TC-2821 / FR-2821: More menu stacks above an open center view and accepts pointer hits.
#[test]
#[ignore]
fn tc_2821_more_menu_above_center_view() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2821_more_menu_above_center_view: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new_with_config("");
    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");

    wait_for_truthy(
        &mut session,
        "!!document.getElementById('diff-toggle')",
        "diff toggle",
        15,
    );
    click_until_truthy(
        &mut session,
        "(() => { const d = document.getElementById('docs'); if (d && !d.classList.contains('open')) document.getElementById('diff-toggle').click(); })()",
        "document.getElementById('docs').classList.contains('open')",
        "diff center open",
        20,
    );

    click_until_truthy(
        &mut session,
        "(() => { const m = document.getElementById('more-menu'); if (m && m.hidden) document.getElementById('more-toggle').click(); })()",
        "!document.getElementById('more-menu').hidden",
        "more menu visible",
        20,
    );
    wait_for_truthy(
        &mut session,
        "(() => { const h = document.querySelector('header.top'); return h && h.classList.contains('more-open') && getComputedStyle(h).zIndex === '30'; })()",
        "header more-open z-index 30",
        5,
    );

    let hits = session
        .cdp
        .eval(
            r##"(() => {
  const menu = document.getElementById('more-menu');
  function inMenu(x, y) {
    let el = document.elementFromPoint(x, y);
    while (el) {
      if (el === menu) return true;
      el = el.parentElement;
    }
    return false;
  }
  const r = menu.getBoundingClientRect();
  const probes = [];
  probes.push({ label: 'menu-top+60', ok: inMenu(r.left + r.width / 2, r.top + 60) });
  for (const el of menu.querySelectorAll('button[role="menuitem"], #theme-select, #accent-picker')) {
    const b = el.getBoundingClientRect();
    const x = b.left + b.width / 2;
    const y = b.top + b.height / 2;
    probes.push({ id: el.id, x, y, ok: inMenu(x, y) });
  }
  const skills = document.getElementById('skills-toggle');
  const sr = skills.getBoundingClientRect();
  return {
    probes,
    skillsX: sr.left + sr.width / 2,
    skillsY: sr.top + sr.height / 2
  };
})()"##,
        )
        .expect("hit probes");
    let probes = hits
        .get("probes")
        .and_then(|v| v.as_array())
        .expect("probes array");
    for probe in probes {
        assert_eq!(
            probe.get("ok").and_then(|v| v.as_bool()),
            Some(true),
            "elementFromPoint must hit #more-menu: {probe:?}"
        );
    }

    let skills_x = hits
        .get("skillsX")
        .and_then(|v| v.as_f64())
        .expect("skillsX");
    let skills_y = hits
        .get("skillsY")
        .and_then(|v| v.as_f64())
        .expect("skillsY");
    session
        .cdp
        .dispatch_mouse_click(skills_x, skills_y)
        .expect("click skills via pointer");
    wait_for_truthy(
        &mut session,
        "!document.getElementById('skills-popover').hidden",
        "skills popover open after pointer click",
        10,
    );
    wait_for_truthy(
        &mut session,
        "(() => { const h = document.querySelector('header.top'); return h && !h.classList.contains('more-open') && getComputedStyle(h).zIndex === '2'; })()",
        "header z-index 2 after menu closed",
        5,
    );
}

// TC-2821 / FR-2821: More menu lists secondary items with correct aria and Esc.
#[test]
#[ignore]
fn tc_2821_more_menu_items() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2821_more_menu_items: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new_with_config("");
    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");

    wait_for_truthy(
        &mut session,
        "!!document.getElementById('more-toggle')",
        "more toggle",
        15,
    );
    let popup = session
        .cdp
        .eval("document.getElementById('more-toggle').getAttribute('aria-haspopup')")
        .expect("aria-haspopup");
    assert_eq!(popup.as_str(), Some("menu"));

    click_until_truthy(
        &mut session,
        "(() => { const m = document.getElementById('more-menu'); if (m && m.hidden) document.getElementById('more-toggle').click(); })()",
        "(() => { const m = document.getElementById('more-menu'); const t = document.getElementById('more-toggle'); \
           return !!m && !m.hidden && t && t.getAttribute('aria-expanded') === 'true'; })()",
        "more menu open",
        20,
    );

    let items = session
        .cdp
        .eval(
            "(() => [...document.querySelectorAll('#more-menu button[role=\"menuitem\"], #more-menu #theme-select, #more-menu #accent-picker')]
              .map(el => (el.tagName === 'SELECT' ? 'Theme' : (el.id === 'accent-picker' ? 'Accent' : el.textContent.trim().replace(/\\s+(Off|On)$/, '').split('\\n')[0]))))()",
        )
        .expect("menu items");
    let names: Vec<String> = items
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        names.windows(2).any(|w| w == ["Sessions", "Memory"]),
        "Sessions then Memory: {names:?}"
    );
    assert!(names.iter().any(|n| n.contains("Skills")), "{names:?}");
    assert!(
        names.iter().any(|n| n.contains("System prompts")),
        "{names:?}"
    );
    assert!(names.iter().any(|n| n == "Full graph"), "{names:?}");
    assert!(names.iter().any(|n| n == "Theme"), "{names:?}");
    assert!(names.iter().any(|n| n == "Accent"), "{names:?}");
    assert!(
        names.iter().any(|n| n.contains("Reset layout")),
        "{names:?}"
    );

    click_until_truthy(
        &mut session,
        "(() => { const p = document.getElementById('memory-popover'); if (p && p.hidden) document.getElementById('memory-toggle').click(); })()",
        "(() => { const m = document.getElementById('more-menu'); const p = document.getElementById('memory-popover'); \
           return !!m && m.hidden && p && !p.hidden; })()",
        "memory popover open after pick",
        20,
    );

    session
        .cdp
        .eval("document.getElementById('more-toggle').click()")
        .expect("reopen more");
    session
        .cdp
        .eval(
            "document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}))",
        )
        .expect("esc");
    wait_for_truthy(
        &mut session,
        "document.getElementById('more-menu').hidden",
        "menu closed on esc",
        5,
    );

    session
        .cdp
        .eval("document.getElementById('more-toggle').click()")
        .expect("open more again");
    wait_for_truthy(
        &mut session,
        "!document.getElementById('more-menu').hidden",
        "menu visible for outside click",
        5,
    );
    session
        .cdp
        .eval("document.getElementById('runs-toggle').click()")
        .expect("click outside menu");
    wait_for_truthy(
        &mut session,
        "document.getElementById('more-menu').hidden",
        "menu closed on outside click",
        5,
    );

    session
        .cdp
        .eval("document.getElementById('more-toggle').click()")
        .expect("open more for tab");
    session
        .cdp
        .eval("document.getElementById('theme-select').focus()")
        .expect("focus theme");
    let tab_reachable = session
        .cdp
        .eval("document.activeElement && document.activeElement.id === 'theme-select'")
        .expect("theme focused");
    assert_eq!(
        tab_reachable.as_bool(),
        Some(true),
        "Theme select must be focusable while More is open"
    );
    session
        .cdp
        .eval("document.getElementById('accent-picker').focus()")
        .expect("focus accent");
    let accent_focused = session
        .cdp
        .eval("document.activeElement && document.activeElement.id === 'accent-picker'")
        .expect("accent focused");
    assert_eq!(
        accent_focused.as_bool(),
        Some(true),
        "Accent picker must be focusable while More is open"
    );

    session
        .cdp
        .eval("document.getElementById('diff-toggle').click()")
        .expect("open diff center");
    wait_for_truthy(
        &mut session,
        "document.getElementById('docs').classList.contains('open')",
        "diff center open",
        10,
    );
    session
        .cdp
        .eval("document.getElementById('more-toggle').click()")
        .expect("open more over center");
    wait_for_truthy(
        &mut session,
        "!document.getElementById('more-menu').hidden",
        "menu over center",
        5,
    );
    session
        .cdp
        .eval(
            "document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}))",
        )
        .expect("esc with center");
    wait_for_truthy(
        &mut session,
        "document.getElementById('more-menu').hidden",
        "menu closed while center stays",
        5,
    );
    let diff_still_open = session
        .cdp
        .eval("document.getElementById('docs').classList.contains('open')")
        .expect("diff still open");
    assert_eq!(
        diff_still_open.as_bool(),
        Some(true),
        "Esc must close More without stealing center-view close"
    );

    session
        .cdp
        .eval("document.getElementById('more-toggle').click()")
        .expect("open more for keyboard");
    session
        .cdp
        .eval("document.getElementById('sessions-toggle').focus()")
        .expect("focus first menu item");
    session
        .cdp
        .eval(
            "document.dispatchEvent(new KeyboardEvent('keydown', {key: 'ArrowDown', bubbles: true}))",
        )
        .expect("arrow between menu items");
    let moved = session
        .cdp
        .eval("document.activeElement && document.activeElement.id !== 'sessions-toggle'")
        .expect("moved focus");
    assert_eq!(
        moved.as_bool(),
        Some(true),
        "ArrowDown must move between More menu buttons"
    );

    session
        .cdp
        .eval("document.getElementById('theme-select').focus()")
        .expect("focus theme for arrows");
    session
        .cdp
        .eval(
            "document.dispatchEvent(new KeyboardEvent('keydown', {key: 'ArrowDown', bubbles: true}))",
        )
        .expect("arrow on select");
    let select_still_focused = session
        .cdp
        .eval("document.activeElement && document.activeElement.id === 'theme-select'")
        .expect("theme still focused");
    assert_eq!(
        select_still_focused.as_bool(),
        Some(true),
        "ArrowDown on Theme must not hijack focus away from the native select"
    );
    let menu_open = session
        .cdp
        .eval("!document.getElementById('more-menu').hidden")
        .expect("menu still open");
    assert_eq!(
        menu_open.as_bool(),
        Some(true),
        "Arrow keys on Theme select must not close the More menu"
    );
}

// TC-2822 / FR-2822: Workspace opens the sidebar with Files/To-dos tabs and badge.
#[test]
#[ignore]
fn tc_2822_workspace_button() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2822_workspace_button: chrome binary not found");
        return;
    }

    let workspace = tempfile::tempdir().expect("temp workspace");
    let ws = workspace.path();
    let data_home = ws.join("data");
    let state_home = ws.join("state");
    let cfg_home = ws.join("cfg");
    let home = ws.join("home");
    for dir in [&data_home, &state_home, &cfg_home, &home] {
        fs::create_dir_all(dir).expect("dir");
    }
    fs::create_dir_all(ws.join(".dmuxai")).expect(".dmuxai");
    fs::write(ws.join(".dmuxai/events.jsonl"), "").expect("events");
    for title in ["Alpha todo", "Beta todo"] {
        let status = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .args(["todo", "add", title])
            .current_dir(ws)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &data_home)
            .env("XDG_STATE_HOME", &state_home)
            .env("XDG_CONFIG_HOME", &cfg_home)
            .env_remove("DMUXAI_WORKSPACE")
            .stdout(std::process::Stdio::null())
            .status()
            .expect("todo add");
        assert!(status.success());
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(ws)
        .env_remove("DMUXAI_WORKSPACE")
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_STATE_HOME", &state_home)
        .env("XDG_CONFIG_HOME", &cfg_home)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let first_line = rx.recv_timeout(Duration::from_secs(20)).expect("url line");
    let dashboard_url = first_line
        .trim()
        .strip_prefix("dmuxai dashboard: ")
        .unwrap()
        .to_string();
    let _serve_guard = ChildGuard(Some(child));

    let mut session = open_dashboard_in_chrome(&dashboard_url).expect("open chrome");
    wait_for_truthy(
        &mut session,
        "document.getElementById('todos-count').textContent === '2'",
        "todo badge",
        20,
    );

    click_until_truthy(
        &mut session,
        "(() => { const s = document.getElementById('sidebar'); if (s && !s.classList.contains('open')) document.getElementById('workspace-toggle').click(); })()",
        "document.getElementById('sidebar').classList.contains('open')",
        "sidebar open",
        20,
    );
    wait_for_truthy(
        &mut session,
        "!!document.getElementById('tab-files') && !!document.getElementById('tab-todos')",
        "sidebar tabs",
        5,
    );
    let label = session
        .cdp
        .eval("document.getElementById('workspace-toggle').getAttribute('aria-label')")
        .expect("label");
    assert_eq!(
        label.as_str(),
        Some("Workspace, 2 open to-dos"),
        "badge in accessible name"
    );
}

// TC-2824 / FR-2824 / OQ-2820: Chat "Open in Graph" and plan-awaiting-approval
// auto-open target Runs (selecting the matching run) when the graph belongs to
// a headless run in Runs, and open the large Graph view otherwise (and always
// under --web for plan approvals).
#[test]
#[ignore]
fn tc_2824_open_in_graph_retarget() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2824_open_in_graph_retarget: chrome binary not found");
        return;
    }

    let fixture = ServeFixture::new_with_config("");
    let runs = fixture.runs_dir();
    let run_id = "cccccccccccccccc";
    let headless_graph_id = "headless-g-2824";
    let plain_graph_id = "chat-g-2824";

    // Write a headless run manifest linked to `headless_graph_id` and its graph file.
    fs::create_dir_all(runs.join(run_id)).expect("run dir");
    let manifest = serde_json::json!({
        "schema_version": 1,
        "run_id": run_id,
        "origin": "cli_headless",
        "status": "completed",
        "created_at_ms": 1_700_000_000_000u64,
        "completed_at_ms": 1_700_000_001_000u64,
        "duration_ms": 1_000u64,
        "requested_runner": "codex",
        "resolved_runner": "codex",
        "requested_model": "composer-2.5",
        "resolved_model": null,
        "dmuxai_version": "0.0.0",
        "build_commit": null,
        "pid": null,
        "dmuxai_exit_code": 0,
        "child_exit_code": null,
        "child_signal": null,
        "error_kind": null,
        "error_summary": null,
        "links": {
            "conversation_id": null,
            "turn_id": null,
            "graph_id": headless_graph_id,
            "node_id": null,
            "attempt_index": null
        },
        "diagnostics": { "stderr_bytes": 0, "tail_sample": null },
        "end_summary": null
    });
    fs::write(
        runs.join(run_id).join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest"),
    )
    .expect("write manifest");

    let graphs = fixture.workspace.join(".dmuxai").join("graphs");
    fs::create_dir_all(&graphs).expect("graphs dir");
    let graph_file = serde_json::json!({
        "plan": {
            "schema": "dmuxai.graph/v1",
            "id": headless_graph_id,
            "orchestrator_model": "gpt-4o",
            "user_prompt": "headless 2824",
            "limits": { "max_parallel": 2, "max_retries": 2 },
            "nodes": [{
                "id": "step1", "role": "plan", "job": "plan task",
                "runner": "codex", "model_id": "m", "skill": null,
                "requires_gate": false, "status": "completed", "attempt": 1
            }],
            "edges": []
        },
        "conversation_id": "conv-2824",
        "phase": "completed",
        "waiting_gate_node": null
    });
    fs::write(
        graphs.join(format!("{headless_graph_id}.json")),
        serde_json::to_vec_pretty(&graph_file).expect("graph file"),
    )
    .expect("write graph file");

    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");
    wait_for_truthy(
        &mut session,
        "typeof applySnapshot === 'function' && snapshot !== null && document.getElementById('runs').classList.contains('open')",
        "dashboard ready with initial runs default",
        15,
    );

    let make_snap =
        |source: &str, graph_id: &str, phase: &str, rev: u64, approval_rev: Option<u64>| {
            let approval = approval_rev.map(|r| {
                serde_json::json!({
                    "rev": r,
                    "prompt": "approve?",
                    "plan_hash": "cafebabe"
                })
            });
            serde_json::json!({
                "schema": "dmuxai.dashboard/v1",
                "revision": rev,
                "generated_at_ms": 1_700_000_002_000u64,
                "source": source,
                "workspace": fixture.workspace.display().to_string(),
                "agents": [],
                "graph": {
                    "id": graph_id,
                    "phase": phase,
                    "orchestrator_model": "gpt-4o",
                    "user_prompt": "test prompt",
                    "approval": approval,
                    "nodes": [{
                        "id": "step1", "role": "plan", "job": "plan task",
                        "runner": "codex", "model_id": "m", "status": "completed", "attempt": 1
                    }],
                    "edges": []
                },
                "session": {
                    "conversation_id": "c-2824",
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
                        "id": "c-2824:0",
                        "role": "user",
                        "agent": null,
                        "content": "hello",
                        "streaming": false
                    }]
                }
            })
        };

    // 1. Non-headless graph "Open in Graph" opens the large Graph view.
    click_until_truthy(
        &mut session,
        "(() => { const s = document.getElementById('session'); if (s && s.hidden) document.getElementById('chat-toggle').click(); })()",
        "(() => { const s = document.getElementById('session'); return !!s && !s.hidden; })()",
        "chat view open",
        20,
    );
    let snap1 = serde_json::to_string(&make_snap("files", plain_graph_id, "completed", 10, None))
        .expect("snap1");
    session
        .cdp
        .eval(&format!("applySnapshot({snap1})"))
        .expect("apply snap1");
    click_until_truthy(
        &mut session,
        &format!(
            "(() => {{ const b = document.querySelector('.chat-aux-open-run[data-run-id=\"{plain_graph_id}\"]'); if (b) b.click(); }})()"
        ),
        "(() => { const g = document.getElementById('graph'); return !!(g && !g.hidden && g.classList.contains('open')); })()",
        "large Graph view opens for non-headless graph",
        20,
    );

    // 2. Headless-run graph "Open in Graph" opens Runs and selects that run.
    session
        .cdp
        .eval("document.getElementById('chat-toggle').click()")
        .expect("back to chat");
    let snap2 = serde_json::to_string(&make_snap(
        "files",
        headless_graph_id,
        "completed",
        11,
        None,
    ))
    .expect("snap2");
    session
        .cdp
        .eval(&format!("applySnapshot({snap2})"))
        .expect("apply snap2");
    click_until_truthy(
        &mut session,
        &format!(
            "(() => {{ const b = document.querySelector('.chat-aux-open-run[data-run-id=\"{headless_graph_id}\"]'); if (b) b.click(); }})()"
        ),
        &format!(
            "(() => {{ \
                const r = document.getElementById('runs'); \
                const sel = document.querySelector('.runs-row-selected'); \
                const btn = document.getElementById('runs-full-graph'); \
                return !!(r && !r.hidden && r.classList.contains('open') \
                    && sel && sel.getAttribute('data-run-id') === '{run_id}' \
                    && btn && !btn.hidden); \
            }})()"
        ),
        "Runs opens and selects headless run with Full graph button",
        20,
    );

    // 3. OQ-2820: Under --web (source !== 'files'), pending plan auto-open still
    // opens the large Graph view with approval controls even if graph_id matches a run.
    session
        .cdp
        .eval("document.getElementById('chat-toggle').click()")
        .expect("back to chat for web approval");
    let snap3 = serde_json::to_string(&make_snap(
        "tui",
        headless_graph_id,
        "awaiting_approval",
        12,
        Some(1),
    ))
    .expect("snap3");
    session
        .cdp
        .eval(&format!("applySnapshot({snap3})"))
        .expect("apply snap3");
    wait_for_truthy(
        &mut session,
        "(() => { \
            const g = document.getElementById('graph'); \
            const ap = document.getElementById('graph-approval'); \
            return !!(g && !g.hidden && g.classList.contains('open') && ap && !ap.hidden); \
        })()",
        "large Graph view with approval controls opens under --web",
        10,
    );

    // 4. Under serve (source === 'files'), pending plan auto-open for a headless
    // run's graph selects that run in Runs instead.
    session
        .cdp
        .eval("document.getElementById('chat-toggle').click()")
        .expect("back to chat for serve approval");
    let snap4 = serde_json::to_string(&make_snap(
        "files",
        headless_graph_id,
        "awaiting_approval",
        13,
        Some(2),
    ))
    .expect("snap4");
    session
        .cdp
        .eval(&format!("applySnapshot({snap4})"))
        .expect("apply snap4");
    wait_for_truthy(
        &mut session,
        &format!(
            "(() => {{ \
                const r = document.getElementById('runs'); \
                const sel = document.querySelector('.runs-row-selected'); \
                return !!(r && !r.hidden && r.classList.contains('open') \
                    && sel && sel.getAttribute('data-run-id') === '{run_id}'); \
            }})()"
        ),
        "Runs opens and selects headless run on serve approval auto-open",
        10,
    );
}
