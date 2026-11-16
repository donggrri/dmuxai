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
            dashboard_url: url.to_string(),
            _guard: ChildGuard(Some(child)),
        }
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
    session
        .cdp
        .eval("document.getElementById('sessions-toggle').click()")
        .expect("close sessions sidebar");
    session
        .cdp
        .eval("document.getElementById('workspace-toggle').click()")
        .expect("open files sidebar");
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

    session
        .cdp
        .eval("document.getElementById('accent-reset').click()")
        .expect("click reset layout");

    // Accent back to the configured (snapshot) accent...
    wait_for_truthy(
        &mut session,
        "(() => localStorage.getItem('dmuxai.accent') === null && getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() === '#3366ff')()",
        "accent reset",
        10,
    );
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

const TC_2828_PREVIEW: &str = "Follow these rules for TC-2828.";

// TC-2828 / FR-2828: configured prompt shows preview in placeholder, not value.
#[test]
#[ignore]
fn tc_2828_system_prompt_preview_placeholder() {
    let chrome_bin = find_chrome_binary();
    if chrome_bin.is_none() {
        println!("skipping tc_2828_system_prompt_preview_placeholder: chrome binary not found");
        return;
    }

    let config = format!("[runners.codex]\nsystem_prompt = \"{TC_2828_PREVIEW}\"\n");
    let fixture = ServeFixture::new_with_config(&config);
    let mut session = open_dashboard_in_chrome(&fixture.dashboard_url).expect("open chrome");

    wait_for_truthy(
        &mut session,
        "!!document.getElementById('more-toggle')",
        "more menu toggle",
        15,
    );
    session
        .cdp
        .eval("(() => { const m = document.getElementById('more-menu'); if (m.hidden) document.getElementById('more-toggle').click(); document.getElementById('agent-prompts-toggle').click(); })()")
        .expect("open system prompts");
    wait_for_truthy(
        &mut session,
        "!document.getElementById('agent-prompts-popover').hidden",
        "system prompts popover",
        10,
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
        ph.contains(TC_2828_PREVIEW),
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
        TC_2828_PREVIEW,
        "preview field matches configured prompt"
    );
    assert!(
        session
            .cdp
            .eval("document.querySelector('.agent-prompt-row[data-runner=\"codex\"]').innerText.includes('Follow these rules')")
            .ok()
            .and_then(|v| v.as_bool())
            == Some(true),
        "preview text visible in row"
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

    session
        .cdp
        .eval("document.getElementById('more-toggle').click()")
        .expect("open more");
    wait_for_truthy(
        &mut session,
        "document.getElementById('more-toggle').getAttribute('aria-expanded') === 'true'",
        "menu expanded",
        5,
    );
    wait_for_truthy(
        &mut session,
        "!document.getElementById('more-menu').hidden",
        "menu visible",
        5,
    );

    let items = session
        .cdp
        .eval(
            "(() => [...document.querySelectorAll('#more-menu [role=\"menuitem\"]')]
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

    session
        .cdp
        .eval("document.getElementById('memory-toggle').click()")
        .expect("pick memory");
    wait_for_truthy(
        &mut session,
        "document.getElementById('more-menu').hidden",
        "menu closed after pick",
        5,
    );
    wait_for_truthy(
        &mut session,
        "!document.getElementById('memory-popover').hidden",
        "memory popover open",
        10,
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

    session
        .cdp
        .eval("document.getElementById('workspace-toggle').click()")
        .expect("workspace");
    wait_for_truthy(
        &mut session,
        "document.getElementById('sidebar').classList.contains('open')",
        "sidebar open",
        10,
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
