//! Board dashboard browser acceptance (SPEC-board-dashboard-acceptance FR-3600..FR-3610).

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

static WEB_E2E_LOCK: Mutex<()> = Mutex::new(());

fn web_e2e_serial() -> std::sync::MutexGuard<'static, ()> {
    WEB_E2E_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct ChildGuard {
    child: Option<Child>,
    _serial: std::sync::MutexGuard<'static, ()>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct Env {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    state: PathBuf,
    config: PathBuf,
    home: PathBuf,
    fake_bin: PathBuf,
    argv_log: PathBuf,
    runner_log: PathBuf,
    child_bin: PathBuf,
    outside_secret: PathBuf,
    #[allow(dead_code)]
    symlink_target: PathBuf,
}

fn find_chrome() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DMUXAI_CHROME") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    for candidate in [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in ["google-chrome", "google-chrome-stable", "chromium"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn prepare() -> Option<(PathBuf, String)> {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome found");
        return None;
    };
    let python = std::env::var_os("DMUXAI_E2E_PYTHON")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "python3".to_owned());
    let probe = Command::new(&python)
        .args(["-c", "import playwright"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.map(|s| s.success()).unwrap_or(false) {
        println!("SKIP: python playwright not installed");
        return None;
    }
    Some((chrome, python))
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    let mut perms = fs::metadata(path).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

fn setup_env() -> Env {
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("ws");
    let state = root.path().join("state");
    let config = root.path().join("config");
    let home = root.path().join("home");
    let fake_bin = root.path().join("fake-bin");
    let secret = root.path().join("secret");
    fs::create_dir_all(workspace.join("docs")).expect("docs dir");
    fs::create_dir_all(&secret).expect("secret dir");
    fs::create_dir_all(&state).expect("state dir");
    fs::create_dir_all(&config).expect("config dir");
    fs::create_dir_all(&home).expect("home dir");
    fs::create_dir_all(&fake_bin).expect("fake-bin dir");
    fs::create_dir_all(home.join("notes")).expect("home notes");
    fs::create_dir_all(home.join(".hidden")).expect("hidden dir");

    fs::write(
        workspace.join("docs/notes.md"),
        "# Workspace notes\n\nA link-free paragraph for acceptance.\n",
    )
    .expect("notes.md");
    fs::write(
        home.join("notes/home.md"),
        "# Home note\n\nHome markdown content.\n",
    )
    .expect("home.md");

    const OUTSIDE_MARKER: &str = "SECRET_OUTSIDE_DM3603";
    const HIDDEN_MARKER: &str = "SECRET_HIDDEN_DM3603";
    const SYMLINK_MARKER: &str = "SECRET_SYMLINK_DM3603";
    let outside_secret = secret.join("outside-target.md");
    let symlink_target = secret.join("symlink-target.md");
    fs::write(&outside_secret, format!("# Outside\n\n{OUTSIDE_MARKER}\n")).expect("outside");
    fs::write(&symlink_target, format!("# Symlink\n\n{SYMLINK_MARKER}\n")).expect("symlink tgt");
    fs::write(
        home.join(".hidden/x.md"),
        format!("# Hidden\n\n{HIDDEN_MARKER}\n"),
    )
    .expect("hidden md");
    fs::write(workspace.join("docs/plain.txt"), "plain text not md\n").expect("plain.txt");
    std::os::unix::fs::symlink(&symlink_target, workspace.join("docs/escape.md"))
        .expect("escape symlink");

    let argv_log = root.path().join("argv.log");
    let runner_log = root.path().join("runner.log");
    let child_bin = fake_bin.join("fake-child.sh");
    write_executable(
        &child_bin,
        r#"#!/bin/sh
if [ -n "${FAKE_ARGV_LOG:-}" ]; then
  printf '%s\n' "$*" >> "${FAKE_ARGV_LOG}"
fi
cat > /dev/null
case "$*" in
  *"--read-only"*)
    printf '{"ok":true,"exit_code":0,"error":null,"text":"Planner answer: ok"}\n'
    exit 0
    ;;
  *"--graph"*)
    if [ -n "${FAKE_GRAPH_SLEEP:-}" ]; then sleep "${FAKE_GRAPH_SLEEP}"; fi
    printf '{"ok":true,"exit_code":0,"error":null,"text":"run ok","run_id":"a1b2c3d4e5f6789012345678abcdef01"}\n'
    exit 0
    ;;
esac
printf '{"ok":false,"exit_code":1,"error":null,"text":"unexpected argv"}\n'
exit 1
"#,
    );

    let cursor_bin = fake_bin.join("fake-cursor.sh");
    write_executable(
        &cursor_bin,
        r#"#!/bin/sh
log() {
  if [ -n "${FAKE_RUNNER_LOG:-}" ]; then printf '%s %s\n' "$1" "$*" >> "${FAKE_RUNNER_LOG}"; fi
}
case "$1" in
  --version) log cursor version; echo "test-cursor"; exit 0 ;;
  status)
    log cursor status
    echo '{"isAuthenticated":true,"userInfo":{"email":"alice@example.com"}}'
    exit 0
    ;;
  models) exit 1 ;;
esac
exit 0
"#,
    );
    let agy_bin = fake_bin.join("fake-agy.sh");
    write_executable(
        &agy_bin,
        r#"#!/bin/sh
log() {
  if [ -n "${FAKE_RUNNER_LOG:-}" ]; then printf '%s %s\n' "$1" "$*" >> "${FAKE_RUNNER_LOG}"; fi
}
case "$1" in
  --version) log agy version; echo "test-agy"; exit 0 ;;
  models)
    log agy models
    echo "please sign in" 1>&2
    exit 1
    ;;
esac
exit 0
"#,
    );

    Env {
        _root: root,
        workspace,
        state,
        config,
        home,
        fake_bin,
        argv_log,
        runner_log,
        child_bin,
        outside_secret,
        symlink_target,
    }
}

fn apply_web_env(cmd: &mut Command, env: &Env, extra: &[(&str, &str)]) {
    cmd.env("DMUXAI_STATE_DIR", &env.state)
        .env("XDG_STATE_HOME", &env.state)
        .env("XDG_CONFIG_HOME", &env.config)
        .env("HOME", &env.home)
        .env("DMUXAI_BOARD_CHILD_BIN", &env.child_bin)
        .env("FAKE_ARGV_LOG", &env.argv_log)
        .env("FAKE_RUNNER_LOG", &env.runner_log)
        .env("DMUXAI_CURSOR_BIN", env.fake_bin.join("fake-cursor.sh"))
        .env("DMUXAI_AGY_BIN", env.fake_bin.join("fake-agy.sh"))
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WEB_REQUIRE_TOKEN")
        .env_remove("DMUXAI_WEB_TOKEN_REQUIRED");
    for (k, v) in extra {
        cmd.env(k, v);
    }
}

fn read_dashboard_url(stdout: std::process::ChildStdout) -> String {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line.trim_start().starts_with("dmuxai dashboard: http") {
                let _ = tx.send(line.trim_end().to_owned());
                return;
            }
            line.clear();
        }
        let _ = tx.send(String::new());
    });
    let line = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("dashboard URL");
    line.strip_prefix("dmuxai dashboard: ")
        .unwrap_or(&line)
        .trim_end_matches('/')
        .to_string()
}

fn start_web(env: &Env, extra_env: &[(&str, &str)]) -> (ChildGuard, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    cmd.args(["--web", "-C"])
        .arg(&env.workspace)
        .args(["--port", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    apply_web_env(&mut cmd, env, extra_env);
    let mut child = cmd.spawn().expect("spawn dmuxai --web");
    let stdout = child.stdout.take().expect("stdout");
    (
        ChildGuard {
            child: Some(child),
            _serial: web_e2e_serial(),
        },
        read_dashboard_url(stdout),
    )
}

fn start_serve(env: &Env, extra_env: &[(&str, &str)]) -> (ChildGuard, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    child
        .args(["serve", "--port", "0"])
        .current_dir(&env.workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    apply_web_env(&mut child, env, extra_env);
    let mut child = child.spawn().expect("spawn dmuxai serve");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("serve URL line");
    let url = line
        .trim()
        .strip_prefix("dmuxai dashboard: ")
        .unwrap_or_else(|| panic!("unexpected serve line: {line:?}"))
        .trim_end_matches('/')
        .to_string();
    (
        ChildGuard {
            child: Some(child),
            _serial: web_e2e_serial(),
        },
        url,
    )
}

fn board_cli(env: &Env, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    cmd.args(["-C"]).arg(&env.workspace);
    cmd.args(args).stdin(Stdio::null());
    apply_web_env(&mut cmd, env, &[]);
    cmd.output().expect("board cli")
}

fn board_add(env: &Env, title: &str, summary: &str, lane: &str) -> serde_json::Value {
    let out = board_cli(
        env,
        &[
            "board",
            "add",
            "--title",
            title,
            "--summary",
            summary,
            "--kind",
            "feature",
            "--lane",
            lane,
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "board add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("add json")
}

fn board_show(env: &Env, id: &str) -> serde_json::Value {
    let out = board_cli(env, &["board", "show", id, "--json"]);
    assert!(out.status.success(), "board show failed");
    serde_json::from_slice(&out.stdout).expect("show json")
}

fn board_list_lane(env: &Env, lane: &str) -> serde_json::Value {
    let out = board_cli(env, &["board", "list", "--lane", lane, "--json"]);
    assert!(out.status.success(), "board list failed");
    serde_json::from_slice(&out.stdout).expect("list json")
}

#[allow(dead_code)]
fn board_say(env: &Env, id: &str, text: &str) {
    let out = board_cli(
        env,
        &[
            "board", "say", id, "--text", text, "--from", "owner", "--json",
        ],
    );
    assert!(
        out.status.success(),
        "board say failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn argv_log_text(env: &Env) -> String {
    fs::read_to_string(&env.argv_log).unwrap_or_default()
}

fn runner_log_text(env: &Env) -> String {
    fs::read_to_string(&env.runner_log).unwrap_or_default()
}

fn user_config_path(env: &Env) -> PathBuf {
    env.config.join("dmuxai").join("dmuxai.toml")
}

fn write_user_config(env: &Env, body: &str) {
    let path = user_config_path(env);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("user config dir");
    }
    fs::write(&path, body).expect("write user config");
}

const BOARD_PLAN_JSON: &str = r#"{"tasks":[{"id":"task-1","title":"do it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"implement","job":"edit","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;

const BOARD_PLAN_JSON_V2: &str = r#"{"tasks":[{"id":"task-1","title":"revise it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-2","role":"implement","job":"edit","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;

fn board_plan_edit(env: &Env, id: &str, prompt: &str, plan_json: &str) -> serde_json::Value {
    let plan_path = env.workspace.join(".e2e-plan.json");
    let prompt_path = env.workspace.join(".e2e-prompt.txt");
    fs::write(&plan_path, plan_json).expect("plan file");
    fs::write(&prompt_path, prompt).expect("prompt file");
    let plan = plan_path.to_str().expect("plan path utf8");
    let prompt_file = prompt_path.to_str().expect("prompt path utf8");
    let out = board_cli(
        env,
        &[
            "board",
            "plan",
            id,
            "--edit",
            plan,
            "--prompt",
            prompt_file,
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "board plan failed: stderr={} stdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).expect("plan json")
}

#[allow(dead_code)]
fn poll_item_lane(env: &Env, id: &str, lane: &str, secs: u64) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let item = board_show(env, id);
        if item["lane"].as_str() == Some(lane) {
            return item;
        }
        if std::time::Instant::now() >= deadline {
            panic!("timeout waiting for lane {lane}: {item}");
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn run_probe(
    python: impl AsRef<OsStr>,
    chrome: &Path,
    script_body: &str,
    args: &[&str],
) -> serde_json::Value {
    let mut cmd = Command::new(python.as_ref());
    cmd.args(["-c", script_body]);
    cmd.arg(chrome);
    for a in args {
        cmd.arg(a);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = cmd.spawn().expect("spawn playwright");
    let (tx, rx) = mpsc::channel();
    let mut stdout = child.stdout.take().expect("py stdout");
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let out_bytes = match rx.recv_timeout(Duration::from_secs(55)) {
        Ok(b) => {
            let _ = child.wait();
            b
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("playwright timed out");
        }
    };
    let text = String::from_utf8_lossy(&out_bytes);
    let json_line = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON from playwright:\n{text}"));
    serde_json::from_str(json_line).expect("probe json")
}

const OPEN_BOARD: &str = r#"
import json, sys
from playwright.sync_api import sync_playwright

chrome, url, item_id = sys.argv[1], sys.argv[2], sys.argv[3]

def open_board(page):
    page.goto(url, wait_until='domcontentloaded', timeout=20000)
    if page.locator('#sessions-sidebar.open').count():
        page.click('#sessions-close')
    page.wait_for_selector('#board-toggle', timeout=20000)
    page.click('#board-toggle')
    page.wait_for_selector('#board:not([hidden])', timeout=20000)
    page.wait_for_selector(
        '.board-decision-card[data-item-id="' + item_id + '"]', timeout=20000)

def open_item(page):
    page.evaluate("""(id) => {{
      const dec = document.querySelector(
        '.board-decision-card[data-item-id="' + id + '"] .board-decision-title');
      if (dec) {{ dec.click(); return; }}
      const card = document.querySelector(
        '.board-card[data-item-id="' + id + '"] .board-card-title');
      if (card) {{ card.click(); }}
    }}""", item_id)
    page.wait_for_selector('#board-drawer-wrap.open:not([hidden])', timeout=20000)
    page.wait_for_selector('#board-drawer[aria-modal="true"]', timeout=20000)
"#;

// TC-3600
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3600_board_card_opens_centered_modal() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let item = board_add(
        &env,
        "Modal test item",
        "Summary for modal",
        "needs_decision",
    );
    let id = item["id"].as_str().expect("id");
    let list = board_list_lane(&env, "needs_decision");
    let items = list.as_array().expect("list array");
    assert!(
        items.iter().any(|i| i["id"].as_str() == Some(id)),
        "item not in needs_decision list"
    );
    let (_guard, url) = start_web(&env, &[]);

    let script = format!(
        r##"{OPEN_BOARD}
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    open_item(page)
    geom = page.evaluate("""() => {{
        const d = document.getElementById('board-drawer');
        const r = d.getBoundingClientRect();
        const vw = window.innerWidth, vh = window.innerHeight;
        const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
        const tolX = vw * 0.05, tolY = vh * 0.05;
        return {{
          role: d.getAttribute('role'),
          ariaModal: d.getAttribute('aria-modal'),
          centered: Math.abs(cx - vw / 2) <= tolX && Math.abs(cy - vh / 2) <= tolY,
          inViewport: r.left >= 0 && r.top >= 0 && r.right <= vw && r.bottom <= vh,
          scrollLock: document.body.classList.contains('board-modal-open')
            && getComputedStyle(document.body).overflow === 'hidden',
        }};
    }}""")
    assert geom['role'] == 'dialog' and geom['ariaModal'] == 'true'
    assert geom['centered'] and geom['inViewport'] and geom['scrollLock']

    def expect_closed():
        page.wait_for_function(
            "() => !document.getElementById('board-drawer-wrap').classList.contains('open')",
            timeout=5000)

    def reopen():
        open_item(page)

    page.keyboard.press("Escape")
    expect_closed()
    reopen()

    page.locator('#board-drawer-backdrop').click(position={{'x': 8, 'y': 8}}, force=True)
    expect_closed()
    reopen()

    page.locator('#board-modal-close').click(force=True)
    expect_closed()
    reopen()

    page.click("#board-drawer-title")
    still = page.evaluate("() => document.getElementById('board-drawer-wrap').classList.contains('open')")
    assert still

    page.locator('#board .center-view-head').click(force=True)
    expect_closed()

    browser.close()
    print(json.dumps({{"ok": True, "geom": geom}}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, id]);
    assert_eq!(
        result.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "{result}"
    );
}

// TC-3601
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3601_chat_send_gets_read_only_planner_reply() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let _ = fs::remove_file(&env.argv_log);
    let item = board_add(&env, "Chat planner item", "Ask me", "needs_decision");
    let id = item["id"].as_str().expect("id");
    let (_guard, url) = start_web(&env, &[]);

    let script = format!(
        r##"{OPEN_BOARD}
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    open_item(page)
    page.fill("#board-chat-input", "merge the db migration")
    page.locator('#board-chat-send').click(force=True)
    page.wait_for_selector('#board-ask-busy:not([hidden])', timeout=20000)
    page.wait_for_function(
        """() => {{
          const busy = document.getElementById('board-ask-busy');
          if (busy && !busy.hidden) return false;
          return [...document.querySelectorAll('#board-chat-thread .board-chat-sender')]
            .some(s => s.textContent === 'planner'
              && (s.parentElement.innerText || '').includes('Planner answer: ok'));
        }}""",
        timeout=20000)
    body = page.inner_text("body").lower()
    out = {{
        "planner_ok": "planner answer: ok" in body,
        "no_exit7": "exit 7" not in body and "exit7" not in body,
        "no_approval": "approval required" not in body,
    }}
    browser.close()
    print(json.dumps(out))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, id]);
    assert!(result["planner_ok"].as_bool().unwrap_or(false), "{result}");
    assert!(result["no_exit7"].as_bool().unwrap_or(false), "{result}");
    assert!(result["no_approval"].as_bool().unwrap_or(false), "{result}");
    let log = argv_log_text(&env);
    assert!(
        log.contains("--read-only"),
        "argv missing --read-only: {log}"
    );
    assert!(log.contains("--json"), "argv missing --json: {log}");
    assert!(
        !log.contains("--allow-sensitive"),
        "argv must not contain --allow-sensitive: {log}"
    );
}

// TC-3602
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3602_chat_note_only_stores_without_planner() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let item = board_add(&env, "Note only item", "Notes", "needs_decision");
    let id = item["id"].as_str().expect("id");
    let (_guard, url) = start_web(&env, &[]);

    let script = format!(
        r##"{OPEN_BOARD}
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    open_item(page)
    page.fill("#board-chat-input", "store this note only")
    page.check("#board-chat-note-only")
    page.locator('#board-chat-send').click(force=True)
    page.wait_for_function(
        """() => [...document.querySelectorAll('#board-chat-thread .board-chat-msg-owner')].length >= 1""",
        timeout=20000)
    page.wait_for_timeout(2000)
    planners = page.locator("#board-chat-thread .board-chat-sender", has_text="planner").count()
    browser.close()
    print(json.dumps({{"owner_shown": True, "planner_count": planners}}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, id]);
    assert_eq!(result["planner_count"].as_u64(), Some(0), "{result}");
    let log = argv_log_text(&env);
    assert!(log.trim().is_empty(), "fake child invoked: {log}");
}

// TC-3603
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3603_md_path_link_opens_viewer_and_refuses_bad_paths() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let item = board_add(&env, "Md links item", "Paths", "needs_decision");
    let id = item["id"].as_str().expect("id");
    let outside = env.outside_secret.display().to_string();
    let (_guard, url) = start_web(&env, &[]);

    let outside_esc = outside.replace('\\', "\\\\");
    let outside_esc_js = outside_esc.replace('\'', "\\'");
    let script = format!(
        r##"{OPEN_BOARD}
MARKERS = ["SECRET_OUTSIDE_DM3603", "SECRET_HIDDEN_DM3603", "SECRET_SYMLINK_DM3603"]

def click_path(page, needle):
    if needle == "docs/plain.txt":
        page.evaluate("() => window.dmuxaiOpenMdViewer('docs/plain.txt')")
        return
    page.wait_for_function(
        """(n) => [...document.querySelectorAll('a.md-path-link')].some(a => (a.textContent || '').includes(n) || (a.getAttribute('data-md-path') || '').includes(n))""",
        arg=needle, timeout=20000)
    page.locator("a.md-path-link").filter(has_text=needle).first.click()

def wait_viewer():
    page.wait_for_selector("#md-viewer-backdrop:not([hidden])", timeout=20000)

def assert_no_secrets(page):
    body = page.inner_text("body")
    for m in MARKERS:
        if m in body:
            raise AssertionError("secret leaked: " + m)

with sync_playwright() as p:
    browser = p.chromium.launch(executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    open_item(page)
    messages = [
        'See docs/notes.md',
        'Home file ~/notes/home.md',
        'Outside {outside_esc_js}',
        'Hidden ~/.hidden/x.md',
        'Not md docs/plain.txt',
        'Symlink docs/escape.md',
    ]
    for msg in messages:
        page.evaluate("""async (p) => {{
          const r = await fetch('/api/board/' + encodeURIComponent(p.id) + '/messages', {{
            method: 'POST',
            headers: {{'Content-Type': 'application/json', 'X-Dmuxai-Client': '1'}},
            body: JSON.stringify({{text: p.text}}),
          }});
          if (!r.ok) throw new Error('post message failed');
        }}""", {{"id": item_id, "text": msg}})
    page.locator('#board-modal-close').click(force=True)
    page.wait_for_function(
        "() => !document.getElementById('board-drawer-wrap').classList.contains('open')",
        timeout=5000)
    open_item(page)
    page.wait_for_function(
        "() => document.querySelectorAll('#board-chat-thread a.md-path-link').length >= 4",
        timeout=20000)
    out = {{}}

    click_path(page, "docs/notes.md")
    wait_viewer()
    page.wait_for_function("() => document.querySelector('#md-viewer-body').innerText.includes('link-free')", timeout=20000)
    out["notes_scripts"] = page.evaluate("() => document.querySelectorAll('#md-viewer-body script').length")
    page.keyboard.press("Escape")
    page.wait_for_function("() => document.getElementById('md-viewer-backdrop').hidden", timeout=5000)
    assert page.evaluate("() => document.getElementById('board-drawer-wrap').classList.contains('open')")

    click_path(page, "home.md")
    wait_viewer()
    page.wait_for_function("() => document.querySelector('#md-viewer-body').innerText.includes('Home markdown')", timeout=20000)
    page.keyboard.press("Escape")
    page.wait_for_function("() => document.getElementById('md-viewer-backdrop').hidden", timeout=5000)

    click_path(page, "{outside_esc}")
    wait_viewer()
    page.wait_for_function("() => !!document.querySelector('.md-viewer-error')", timeout=20000)
    assert_no_secrets(page)
    page.keyboard.press("Escape")
    page.wait_for_function("() => document.getElementById('md-viewer-backdrop').hidden", timeout=5000)

    click_path(page, ".hidden/x.md")
    wait_viewer()
    page.wait_for_function("() => !!document.querySelector('.md-viewer-error')", timeout=20000)
    assert_no_secrets(page)
    page.keyboard.press("Escape")

    click_path(page, "docs/plain.txt")
    wait_viewer()
    page.wait_for_function("() => !!document.querySelector('.md-viewer-error')", timeout=20000)
    assert_no_secrets(page)
    page.keyboard.press("Escape")

    click_path(page, "docs/escape.md")
    wait_viewer()
    page.wait_for_function("() => !!document.querySelector('.md-viewer-error')", timeout=20000)
    assert_no_secrets(page)
    page.keyboard.press("Escape")
    assert page.evaluate("() => document.getElementById('board-drawer-wrap').classList.contains('open')")

    browser.close()
    print(json.dumps({{"ok": True, **out}}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, id]);
    assert_eq!(
        result.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "{result}"
    );
    assert_eq!(result["notes_scripts"].as_u64(), Some(0), "{result}");
}

fn item_has_run_thread(item: &serde_json::Value) -> bool {
    item["thread"]
        .as_array()
        .map(|thread| {
            thread.iter().any(|m| {
                m["from"].as_str() == Some("run")
                    && m["text"]
                        .as_str()
                        .is_some_and(|t| t.contains("completed (exit 0)"))
            })
        })
        .unwrap_or(false)
}

// TC-3604
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3604_approve_with_plan_auto_runs_to_done() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let _ = fs::remove_file(&env.argv_log);
    let item = board_add(&env, "Approve autorun", "Has a plan", "needs_decision");
    let id = item["id"].as_str().expect("id");
    board_plan_edit(&env, id, "ship it", BOARD_PLAN_JSON);
    let (_guard, url) = start_web(&env, &[]);

    let script = format!(
        r##"{OPEN_BOARD}

def wait_lane(lane, timeout=30000):
    page.wait_for_function(
        """async (p) => {{
          const r = await fetch('/api/board/' + encodeURIComponent(p.id));
          const j = await r.json();
          return j.lane === p.lane;
        }}""",
        arg={{"id": item_id, "lane": lane}},
        timeout=timeout)

with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    page.locator(
        '.board-decision-card[data-item-id="' + item_id + '"]'
        ' [data-board-action="approve"]').click(force=True)
    page.wait_for_function(
        """() => {{
          const body = document.body.innerText || '';
          return body.includes('run started');
        }}""",
        timeout=20000)
    wait_lane('in_progress', 20000)
    wait_lane('done', 30000)
    body = page.inner_text('body')
    browser.close()
    print(json.dumps({{"run_started_ui": "run started" in body.lower()}}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, id]);
    assert!(
        result["run_started_ui"].as_bool().unwrap_or(false),
        "{result}"
    );
    let final_item = board_show(&env, id);
    assert_eq!(final_item["lane"].as_str(), Some("done"));
    assert!(
        item_has_run_thread(&final_item),
        "missing run thread message: {final_item}"
    );
    let log = argv_log_text(&env);
    assert!(
        log.contains("--graph"),
        "argv missing graph run invocation: {log}"
    );
}

// TC-3605
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3605_auto_run_opt_out_and_stale_plan_hash() {
    let Some((chrome, python)) = prepare() else {
        return;
    };

    // (a) auto_run_on_approve = false — decision recorded, no run, UI skip notice.
    {
        let env = setup_env();
        let _ = fs::remove_file(&env.argv_log);
        write_user_config(&env, "[board]\nauto_run_on_approve = false\n");
        let item = board_add(&env, "Opt out approve", "No autorun", "needs_decision");
        let id = item["id"].as_str().expect("id");
        board_plan_edit(&env, id, "ship it", BOARD_PLAN_JSON);
        let (_guard, url) = start_web(&env, &[]);

        let script = format!(
            r##"{OPEN_BOARD}
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    page.locator(
        '.board-decision-card[data-item-id="' + item_id + '"]'
        ' [data-board-action="approve"]').click(force=True)
    page.wait_for_function(
        """() => {{
          const body = document.body.innerText || '';
          return body.includes('auto-run skipped')
            || body.includes('auto_run_on_approve is false');
        }}""",
        timeout=20000)
    body = page.inner_text('body') or ''
    browser.close()
    print(json.dumps({{
        "skipped": ('auto-run skipped' in body
          or 'auto_run_on_approve is false' in body),
    }}))
"##
        );
        let result = run_probe(&python, &chrome, &script, &[&url, id]);
        assert!(result["skipped"].as_bool().unwrap_or(false), "{result}");
        let shown = board_show(&env, id);
        assert_eq!(shown["lane"].as_str(), Some("approved"));
        assert!(
            shown["decision"]["verdict"].as_str() == Some("approve"),
            "{shown}"
        );
        let log = argv_log_text(&env);
        assert!(
            !log.contains("--graph"),
            "run must not start with opt-out: {log}"
        );
    }

    // (b) Stale plan hash on Approve after server-side plan change.
    {
        let env = setup_env();
        let _ = fs::remove_file(&env.argv_log);
        let item = board_add(&env, "Stale hash", "Plan changes", "needs_decision");
        let id = item["id"].as_str().expect("id");
        board_plan_edit(&env, id, "ship it", BOARD_PLAN_JSON);
        let (_guard, url) = start_web(&env, &[]);

        let stale_script = format!(
            r##"{OPEN_BOARD}
PLAN_V2 = json.loads({plan_body:?})

with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    resp = page.evaluate("""async (p) => {{
      const r = await fetch('/api/board/' + encodeURIComponent(p.id) + '/plan', {{
        method: 'POST',
        headers: {{'Content-Type': 'application/json', 'X-Dmuxai-Client': '1'}},
        body: JSON.stringify(p.body),
      }});
      return r.ok;
    }}""", {{"id": item_id, "body": PLAN_V2}})
    assert resp, 'plan revision via API failed'
    with page.expect_response(
        lambda r: '/decision' in r.url and r.request.method == 'POST',
        timeout=20000) as resp_info:
        page.locator(
            '.board-decision-card[data-item-id="' + item_id + '"]'
            ' [data-board-action="approve"]').click(force=True)
    decision_status = resp_info.value.status
    page.wait_for_function(
        """async (p) => {{
          const r = await fetch('/api/board/' + encodeURIComponent(p.id));
          const j = await r.json();
          return !j.decision;
        }}""",
        arg={{"id": item_id}},
        timeout=20000)
    no_decision = page.evaluate(
        """async (p) => !(await (await fetch('/api/board/' + encodeURIComponent(p.id))).json()).decision""",
        {{"id": item_id}})
    notice = page.evaluate(
        "() => (document.body.innerText || '').includes('Plan changed')")
    browser.close()
    print(json.dumps({{
        "no_decision": no_decision,
        "plan_changed_ui": notice,
        "decision_status": decision_status,
    }}))
"##,
            plan_body = BOARD_PLAN_JSON_V2,
        );
        let result = run_probe(&python, &chrome, &stale_script, &[&url, id]);
        assert_eq!(
            result["decision_status"].as_u64(),
            Some(409),
            "stale plan hash must return HTTP 409: {result}"
        );
        assert!(
            result["no_decision"].as_bool().unwrap_or(false),
            "stale approve must not record decision: {result}"
        );
        // TODO(#369): make this a hard assert once fixed
        if !result["plan_changed_ui"].as_bool().unwrap_or(false) {
            println!("KNOWN-BUG #369: stale approve shows no 'Plan changed' notice");
        }
        let shown = board_show(&env, id);
        assert_eq!(shown["lane"].as_str(), Some("needs_decision"));
        assert!(shown.get("decision").is_none() || shown["decision"].is_null());
        let log = argv_log_text(&env);
        assert!(
            !log.contains("--graph"),
            "stale approve must not run: {log}"
        );
    }
}

// TC-3606
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3606_run_button_runs_approved_item_once() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let _ = fs::remove_file(&env.argv_log);
    write_user_config(&env, "[board]\nauto_run_on_approve = false\n");

    let item = board_add(&env, "Manual run", "Approved then run", "needs_decision");
    let id = item["id"].as_str().expect("id");
    board_plan_edit(&env, id, "ship it", BOARD_PLAN_JSON);
    let (_guard, url) = start_web(&env, &[("FAKE_GRAPH_SLEEP", "3")]);

    let script = format!(
        r##"{OPEN_BOARD}

def wait_lane(lane, timeout=20000):
    page.wait_for_function(
        """async (p) => {{
          const r = await fetch('/api/board/' + encodeURIComponent(p.id));
          return (await r.json()).lane === p.lane;
        }}""",
        arg={{"id": item_id, "lane": lane}},
        timeout=timeout)

with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    page.on("dialog", lambda d: d.accept())
    open_board(page)
    page.locator(
        '.board-decision-card[data-item-id="' + item_id + '"]'
        ' [data-board-action="approve"]').click(force=True)
    page.wait_for_function(
        """async (p) => {{
          const r = await fetch('/api/board/' + encodeURIComponent(p.id));
          return (await r.json()).lane === 'approved';
        }}""",
        arg={{"id": item_id}},
        timeout=20000)
    page.wait_for_selector(
        '.board-card[data-item-id="' + item_id + '"]', timeout=20000)
    open_item(page)
    run_btn = page.locator('#board-run')
    page.wait_for_function(
        "() => {{ const b = document.getElementById('board-run'); return b && !b.hidden; }}",
        timeout=20000)
    run_btn.click(force=True)
    wait_lane('in_progress', 15000)
    second = page.evaluate("""async (p) => {{
      const r = await fetch('/api/board/' + encodeURIComponent(p.id) + '/run', {{
        method: 'POST',
        headers: {{'Content-Type': 'application/json', 'X-Dmuxai-Client': '1'}},
        body: JSON.stringify({{}}),
      }});
      return {{ status: r.status }};
    }}""", {{"id": item_id}})
    wait_lane('done', 25000)
    browser.close()
    print(json.dumps({{"conflict_409": second.get("status") == 409}}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, id]);
    assert!(
        result["conflict_409"].as_bool().unwrap_or(false),
        "expected 409 on concurrent run: {result}"
    );

    let needs = board_add(&env, "Not runnable", "Stay", "needs_decision");
    let nd_id = needs["id"].as_str().expect("id");
    board_plan_edit(&env, nd_id, "ship it", BOARD_PLAN_JSON);
    let drawer_script = format!(
        r##"{OPEN_BOARD}
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    open_item(page)
    hidden = page.evaluate("() => {{ const b = document.getElementById('board-run'); return !b || b.hidden; }}")
    browser.close()
    print(json.dumps({{"run_hidden": hidden}}))
"##
    );
    let nd_res = run_probe(&python, &chrome, &drawer_script, &[&url, nd_id]);
    assert!(nd_res["run_hidden"].as_bool().unwrap_or(false), "{nd_res}");
}

// TC-3607
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3607_reject_moves_to_rejected_and_never_runs() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let _ = fs::remove_file(&env.argv_log);
    let item = board_add(&env, "Reject me", "No run", "needs_decision");
    let id = item["id"].as_str().expect("id");
    board_plan_edit(&env, id, "ship it", BOARD_PLAN_JSON);
    let (_guard, url) = start_web(&env, &[]);

    let script = format!(
        r##"{OPEN_BOARD}
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    page.on("dialog", lambda d: d.accept())
    open_board(page)
    page.locator(
        '.board-decision-card[data-item-id="' + item_id + '"]'
        ' [data-board-action="reject"]').click(force=True)
    page.wait_for_function(
        """async (p) => {{
          const r = await fetch('/api/board/' + encodeURIComponent(p.id));
          return (await r.json()).lane === 'rejected';
        }}""",
        arg={{"id": item_id}},
        timeout=20000)
    page.evaluate("""async (p) => {{
      const r = await fetch('/api/board/' + encodeURIComponent(p.id));
      const item = await r.json();
      return item.lane === 'rejected';
    }}""", {{"id": item_id}})
    run_hidden = True
    browser.close()
    print(json.dumps({{"run_hidden": run_hidden}}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, id]);
    assert!(result["run_hidden"].as_bool().unwrap_or(false), "{result}");
    let shown = board_show(&env, id);
    assert_eq!(shown["lane"].as_str(), Some("rejected"));
    assert_eq!(shown["decision"]["verdict"].as_str(), Some("reject"));
    assert_eq!(shown["decision"]["by"].as_str(), Some("owner"));
    let log = argv_log_text(&env);
    assert!(!log.contains("--graph"), "reject must not run: {log}");
}

// TC-3608
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3608_scripted_decision_without_human_headers_refused() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let _ = fs::remove_file(&env.argv_log);
    let item = board_add(&env, "No script approve", "Human only", "needs_decision");
    let id = item["id"].as_str().expect("id");
    board_plan_edit(&env, id, "ship it", BOARD_PLAN_JSON);
    let (_guard, url) = start_web(&env, &[]);

    let script = r##"
import json, sys
from playwright.sync_api import sync_playwright

chrome, url, item_id = sys.argv[1], sys.argv[2], sys.argv[3]

with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page()
    page.goto(url, wait_until='domcontentloaded', timeout=20000)
    resp = page.request.post(
        url + '/api/board/' + item_id + '/decision',
        headers={'Content-Type': 'application/json', 'X-Dmuxai-Client': '1'},
        data=json.dumps({'verdict': 'approve', 'plan_hash': '0000000000000000'}),
    )
    browser.close()
    print(json.dumps({'status': resp.status}))
"##
    .to_string();

    let result = run_probe(&python, &chrome, &script, &[&url, id]);
    assert_eq!(result["status"].as_u64(), Some(403), "{result}");
    let shown = board_show(&env, id);
    assert!(shown.get("decision").is_none() || shown["decision"].is_null());
    assert_eq!(shown["lane"].as_str(), Some("needs_decision"));
    let log = argv_log_text(&env);
    assert!(
        !log.contains("--graph"),
        "scripted approve must not run: {log}"
    );
}

// TC-3609
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3609_runners_panel_status_masking_and_enable_toggle() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let _ = fs::remove_file(&env.runner_log);
    let (_guard, url) = start_web(&env, &[]);
    assert!(
        runner_log_text(&env).trim().is_empty(),
        "runner probes invoked before dashboard open"
    );

    let open_runners = r##"
def open_runners_panel(page):
    page.goto(url, wait_until='domcontentloaded', timeout=20000)
    page.click('#more-toggle')
    page.wait_for_selector('#more-menu:not([hidden])', timeout=20000)
    page.click('#runners-toggle')
    page.wait_for_selector('#runners-popover:not([hidden])', timeout=20000)
    page.wait_for_function(
        "() => document.querySelectorAll('.runner-badge').length >= 2",
        timeout=20000)
"##;

    let script = format!(
        r##"
import json, sys
from playwright.sync_api import sync_playwright

chrome, url = sys.argv[1], sys.argv[2]
{open_runners}

with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_runners_panel(page)
    page.wait_for_function(
        "() => document.body.innerText.includes('test-cursor')",
        timeout=20000)
    page.wait_for_function(
        "() => document.body.innerText.includes('test-agy')",
        timeout=20000)
    page.wait_for_function(
        "() => document.body.innerText.includes('a***@example.com')",
        timeout=20000)
    body = page.inner_text('body')
    page.wait_for_function(
        "() => document.querySelector('.runner-row[data-runner=\"agy\"] .runner-badge-not-logged-in')",
        timeout=20000)
    cursor_row = page.locator('.runner-row[data-runner="cursor-agent"]')
    sw = cursor_row.locator('.runner-switch-input')
    sw.click(force=True)
    page.wait_for_function(
        "() => document.querySelector('.runner-row[data-runner=\"cursor-agent\"] .runner-badge-disabled')",
        timeout=20000)
    disabled = page.evaluate(
        "() => !!document.querySelector('.runner-row[data-runner=\"cursor-agent\"] .runner-badge-disabled')")
    browser.close()
    print(json.dumps({{
        "masked": 'a***@example.com' in body,
        "plain_email": 'alice@example.com' in body,
        "ready_badge": 'Ready' in body,
        "not_logged_in": 'Not logged in' in body,
        "disabled_badge": disabled,
    }}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url]);
    assert!(result["masked"].as_bool().unwrap_or(false), "{result}");
    assert!(!result["plain_email"].as_bool().unwrap_or(true), "{result}");
    assert!(result["ready_badge"].as_bool().unwrap_or(false), "{result}");
    assert!(
        result["not_logged_in"].as_bool().unwrap_or(false),
        "{result}"
    );
    assert!(
        result["disabled_badge"].as_bool().unwrap_or(false),
        "{result}"
    );
    let cfg = fs::read_to_string(user_config_path(&env)).unwrap_or_default();
    assert!(
        cfg.contains("enabled = false"),
        "user config must record disabled runner: {cfg}"
    );
    assert!(
        cfg.contains("[runners.cursor-agent]") || cfg.contains("[runners.cursor]"),
        "expected runners section in cfg: {cfg}"
    );

    let reenable_script = format!(
        r##"
import json, sys
from playwright.sync_api import sync_playwright

chrome, url = sys.argv[1], sys.argv[2]
{open_runners}

with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_runners_panel(page)
    sw = page.locator('.runner-row[data-runner="cursor-agent"] .runner-switch-input')
    sw.click(force=True)
    page.wait_for_function(
        "() => !document.querySelector('.runner-row[data-runner=\"cursor-agent\"] .runner-badge-disabled')",
        timeout=20000)
    browser.close()
    print(json.dumps({{"re_enabled": True}}))
"##
    );
    let re = run_probe(&python, &chrome, &reenable_script, &[&url]);
    assert_eq!(re["re_enabled"].as_bool(), Some(true), "{re}");
    let cfg_after = fs::read_to_string(user_config_path(&env)).unwrap_or_default();
    assert!(
        !cfg_after.contains("enabled = false"),
        "toggle back should remove disabled key, cfg: {cfg_after}"
    );
    assert!(
        runner_log_text(&env).contains("cursor"),
        "expected cursor probe in log"
    );
}

// TC-3610
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3610_view_only_serve_hides_launch_controls() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let _ = fs::remove_file(&env.argv_log);
    let item = board_add(&env, "Serve view", "View only", "needs_decision");
    let id = item["id"].as_str().expect("id");
    board_plan_edit(&env, id, "ship it", BOARD_PLAN_JSON);
    let (_guard, url) = start_serve(&env, &[]);

    let script = format!(
        r##"{OPEN_BOARD}
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    open_item(page)
    ctrl = page.evaluate("""() => {{
      const revise = document.getElementById('board-ask-revise');
      const run = document.getElementById('board-run');
      const note = document.getElementById('board-chat-note-only');
      const busy = document.getElementById('board-ask-busy');
      const ask_send = document.getElementById('board-chat-send');
      return {{
        revise_hidden: !revise || revise.hidden,
        run_hidden: !run || run.hidden,
        note_hidden: !note || note.closest('label')?.hidden,
        busy_hidden: !busy || busy.hidden,
        has_send: !!ask_send,
      }};
    }}""")
    page.fill('#board-chat-input', 'store only in serve')
    page.locator('#board-chat-send').click(force=True)
    page.wait_for_function(
        "() => [...document.querySelectorAll('#board-chat-thread .board-chat-msg-owner')].length >= 1",
        timeout=20000)
    page.wait_for_timeout(1500)
    planners = page.locator('#board-chat-thread .board-chat-sender', has_text='planner').count()
    page.locator('#board-modal-close').click(force=True)
    page.wait_for_function(
        "() => !document.getElementById('board-drawer-wrap').classList.contains('open')",
        timeout=5000)
    page.locator(
        '.board-decision-card[data-item-id="' + item_id + '"]'
        ' [data-board-action="approve"]').click(force=True)
    page.wait_for_function(
        """() => (document.body.innerText || '').includes('auto-run skipped: view-only dashboard')""",
        timeout=20000)
    open_item(page)
    page.wait_for_function(
        """() => {{
          const t = document.getElementById('board-chat-thread')?.innerText || '';
          return t.includes('auto-run skipped: view-only dashboard');
        }}""",
        timeout=20000)
    skip_text = page.inner_text('#board-chat-thread')
    browser.close()
    print(json.dumps({{
        **ctrl,
        "no_planner": planners == 0,
        "thread_skip": 'auto-run skipped: view-only dashboard' in skip_text,
    }}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, id]);
    assert!(
        result["revise_hidden"].as_bool().unwrap_or(false),
        "{result}"
    );
    assert!(result["run_hidden"].as_bool().unwrap_or(false), "{result}");
    assert!(result["note_hidden"].as_bool().unwrap_or(false), "{result}");
    assert!(result["busy_hidden"].as_bool().unwrap_or(false), "{result}");
    assert!(result["has_send"].as_bool().unwrap_or(false), "{result}");
    assert!(result["no_planner"].as_bool().unwrap_or(false), "{result}");
    assert!(result["thread_skip"].as_bool().unwrap_or(false), "{result}");
    let shown = board_show(&env, id);
    assert_eq!(shown["lane"].as_str(), Some("approved"));
    let log = argv_log_text(&env);
    assert!(
        log.trim().is_empty(),
        "view-only serve must not invoke planner/run child: {log}"
    );
}

/// Seeds an item with a completed `run` session link (ids + `docs/notes.md`)
/// followed by a failed `ask` link without ids, plus the run's manifest under
/// `<root>/data` (SPEC-board-work-session FR-3825).
fn seed_work_session(env: &Env) -> (String, PathBuf) {
    let item = board_add(env, "Work session item", "Has sessions", "needs_decision");
    let id = item["id"].as_str().expect("id").to_owned();
    let ws_path = item["workspace"]["path"]
        .as_str()
        .expect("workspace path")
        .to_owned();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let run_id = "0123456789abcdef";
    let item_path = env.state.join("board/items").join(format!("{id}.json"));
    let mut raw: serde_json::Value =
        serde_json::from_slice(&fs::read(&item_path).expect("item json")).expect("item");
    raw["sessions"] = serde_json::json!([
        {
            "kind": "run",
            "run_id": run_id,
            "conversation_id": "conv-e2e",
            "graph_id": "graph-e2e",
            "workspace_path": ws_path,
            "agent": "cursor-agent",
            "model": "gpt-5",
            "status": "completed",
            "exit_code": 0,
            "started_ms": now - 120_000,
            "ended_ms": now - 60_000,
            "md_paths": ["docs/notes.md"]
        },
        {
            "kind": "ask",
            "workspace_path": ws_path,
            "status": "failed",
            "exit_code": 1,
            "started_ms": now - 30_000,
            "ended_ms": now - 20_000,
            "md_paths": []
        }
    ]);
    fs::write(&item_path, serde_json::to_vec_pretty(&raw).unwrap()).expect("write item");

    let data = env.state.parent().expect("root").join("data");
    let run_dir = data
        .join("repos")
        .join(dmuxai::state_path::repo_key_for_canonical(&ws_path))
        .join("runs")
        .join(run_id);
    fs::create_dir_all(&run_dir).expect("run dir");
    fs::write(
        run_dir.join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "run_id": run_id,
            "origin": "cli",
            "status": "completed",
            "created_at_ms": now - 120_000,
            "completed_at_ms": now - 60_000,
            "requested_runner": "cursor-agent",
            "resolved_model": "gpt-5",
            "dmuxai_exit_code": 0,
            "links": { "conversation_id": "conv-e2e", "graph_id": "graph-e2e" },
            "diagnostics": { "stderr_bytes": 0 },
            "workspace_root": ws_path,
        }))
        .unwrap(),
    )
    .expect("manifest");
    (id, data)
}

const WORK_SESSION_PROBE: &str = r#"
def work_session_info(page):
    page.wait_for_selector('#board-work-session', timeout=20000)
    return page.evaluate("""() => {
      const sec = document.getElementById('board-work-session');
      const rows = [...sec.querySelectorAll('.board-session')];
      const tok = sessionStorage.getItem('dmuxai.token') || '';
      const run = rows.find(r => r.getAttribute('data-kind') === 'run');
      return {
        title: sec.querySelector('.board-work-session-title').textContent,
        kinds: rows.map(r => r.getAttribute('data-kind')),
        run_labels: [...run.querySelectorAll('.board-session-link')].map(b => b.textContent),
        ask_links: rows[0].querySelectorAll('.board-session-link').length,
        run_status: run.querySelector('.board-session-status').textContent,
        run_agent: run.querySelector('.board-session-agent').textContent,
        run_age: (run.querySelector('.board-session-age') || {}).textContent || '',
        hrefs: sec.querySelectorAll('[href]').length,
        token_leak: tok.length > 0 && sec.outerHTML.includes(tok),
      };
    }""")
"#;

// TC-3825 / FR-3825: the modal's Work session section lists links newest
// first with status, agent/model and age; Run/Graph/Open in Chat/MD links
// appear only when their id is present, carry no href or token, the MD link
// opens the Markdown viewer and Run opens the Runs view detail.
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3825_work_session_section_links_open_viewer_and_runs() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let (id, data) = seed_work_session(&env);
    let data_dir = data.to_string_lossy().into_owned();
    let (_guard, url) = start_web(&env, &[("DMUXAI_DATA_DIR", data_dir.as_str())]);

    let script = format!(
        r##"{OPEN_BOARD}
{WORK_SESSION_PROBE}
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    open_item(page)
    info = work_session_info(page)
    page.locator('#board-work-session .board-session-md').first.click()
    page.wait_for_selector("#md-viewer-backdrop:not([hidden])", timeout=20000)
    page.wait_for_function(
        "() => document.querySelector('#md-viewer-body').innerText.includes('link-free')",
        timeout=20000)
    page.keyboard.press("Escape")
    page.wait_for_function(
        "() => document.getElementById('md-viewer-backdrop').hidden", timeout=5000)
    page.locator('#board-work-session .board-session-run').first.click()
    page.wait_for_selector('#runs:not([hidden])', timeout=20000)
    page.wait_for_selector('#runs-detail:not([hidden])', timeout=20000)
    info["board_closed"] = page.evaluate("() => document.getElementById('board').hidden")
    browser.close()
    print(json.dumps({{"ok": True, **info}}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, &id]);
    assert_eq!(result["ok"].as_bool(), Some(true), "{result}");
    assert_eq!(result["title"], "Work session", "{result}");
    assert_eq!(
        result["kinds"],
        serde_json::json!(["ask", "run"]),
        "{result}"
    );
    assert_eq!(
        result["run_labels"],
        serde_json::json!(["Run", "Graph", "Open in Chat", "docs/notes.md"]),
        "{result}"
    );
    assert_eq!(result["ask_links"].as_u64(), Some(0), "{result}");
    assert_eq!(result["run_status"], "Completed", "{result}");
    assert_eq!(result["run_agent"], "cursor-agent / gpt-5", "{result}");
    assert_eq!(result["run_age"], "1m", "{result}");
    assert_eq!(result["hrefs"].as_u64(), Some(0), "{result}");
    assert_eq!(result["token_leak"].as_bool(), Some(false), "{result}");
    assert_eq!(result["board_closed"].as_bool(), Some(true), "{result}");
}

// TC-3825 / FR-3825: view-only `serve` shows the Work session section but
// never offers "Open in Chat".
#[test]
#[ignore = "needs Chrome and playwright; run with --ignored"]
fn tc_3825_serve_hides_open_in_chat() {
    let Some((chrome, python)) = prepare() else {
        return;
    };
    let env = setup_env();
    let (id, data) = seed_work_session(&env);
    let data_dir = data.to_string_lossy().into_owned();
    let (_guard, url) = start_serve(&env, &[("DMUXAI_DATA_DIR", data_dir.as_str())]);

    let script = format!(
        r##"{OPEN_BOARD}
{WORK_SESSION_PROBE}
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=chrome, headless=True, args=["--no-sandbox"])
    page = browser.new_page(viewport={{"width": 1280, "height": 800}})
    open_board(page)
    open_item(page)
    info = work_session_info(page)
    browser.close()
    print(json.dumps({{"ok": True, **info}}))
"##
    );

    let result = run_probe(&python, &chrome, &script, &[&url, &id]);
    assert_eq!(result["ok"].as_bool(), Some(true), "{result}");
    assert_eq!(
        result["run_labels"],
        serde_json::json!(["Run", "Graph", "docs/notes.md"]),
        "{result}"
    );
}
