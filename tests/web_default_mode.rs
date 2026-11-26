//! End-to-end integration tests for SPEC-web-default-mode (TC-2972, TC-2974, TC-2979).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn http_get_json(url: &str) -> String {
    let rest = url.strip_prefix("http://").unwrap();
    let (host_port, path) = rest.split_once('/').unwrap();
    let mut stream = TcpStream::connect(host_port).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let req = format!("GET /{path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (_, body) = raw.split_once("\r\n\r\n").unwrap();
    body.to_owned()
}

fn base_cmd(cwd: &Path, state: &Path, cfg: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    cmd.current_dir(cwd)
        .env("DMUXAI_STATE_DIR", state)
        .env("XDG_STATE_HOME", state)
        .env("XDG_CONFIG_HOME", cfg)
        .env_remove("DMUXAI_UI")
        .env_remove("DMUXAI_CONFIG")
        .env_remove("DMUXAI_WEB_REQUIRE_TOKEN")
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WORKSPACE")
        .env_remove("DMUXAI_TRUST_WORKSPACE_CONFIG")
        .stdin(Stdio::null());
    cmd
}

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_and_read_url(
    args: &[&str],
    cwd: &Path,
    state: &Path,
    cfg: &Path,
    extra_env: &[(&str, &str)],
) -> (ChildGuard, String) {
    let mut cmd = base_cmd(cwd, state, cfg);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn dmuxai");
    let stdout = child.stdout.take().unwrap();
    if let Some(mut stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut discarded = Vec::new();
            let _ = stderr.read_to_end(&mut discarded);
        });
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if let Some(url) = line.strip_prefix("dmuxai dashboard: ") {
                let _ = tx.send(url.trim().to_owned());
                break;
            }
            line.clear();
        }
    });
    let url = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("dmuxai must print dashboard URL");
    (ChildGuard(Some(child)), url)
}

// TC-2972 / FR-2972: Second `dmuxai` in another workspace opens it in the live dashboard
// without changing focus, prints `?ws=<id>`, updates discovery `workspaces`, and exits 0.
#[test]
fn tc_2972_second_start_opens_workspace_in_live_dashboard() {
    let tmp = tempfile::tempdir().unwrap();
    let ws1 = tmp.path().join("ws1");
    let ws2 = tmp.path().join("ws2");
    let state = tmp.path().join("state");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();

    let (_guard1, url1) = spawn_and_read_url(
        &["--port", "0"],
        &ws1,
        &state,
        &cfg,
        &[("DMUXAI_TRUST_WORKSPACE_CONFIG", "1")],
    );
    let base1 = url1.trim_end_matches('/');

    // Wait briefly for user discovery file to be written.
    let disc_path = state.join("web.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !disc_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(disc_path.exists(), "user web.json must exist");

    // Second start in ws2 attaches in-process and exits 0.
    let out2 = base_cmd(&ws2, &state, &cfg)
        .output()
        .expect("run second dmuxai");
    assert!(
        out2.status.success(),
        "second dmuxai must exit 0, stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    let expected_prefix = format!("dmuxai dashboard: {base1}/?ws=");
    assert!(
        stdout2.contains(&expected_prefix),
        "expected `{expected_prefix}<id>`, got: {stdout2:?}"
    );

    // Focus must still be on default (ws1), not ws2.
    let body = http_get_json(&format!("{base1}/api/workspaces"));
    let resp: serde_json::Value = serde_json::from_str(&body).unwrap();
    let list = resp["workspaces"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], "default");
    assert_eq!(list[0]["focused"], true);
    assert_eq!(list[1]["focused"], false);

    // User discovery `workspaces` list includes both workspaces.
    let disc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&disc_path).unwrap()).unwrap();
    assert_eq!(disc["workspaces"].as_array().unwrap().len(), 2);

    // Third start in ws2 (already open) prints the same URL and exits 0.
    let out3 = base_cmd(&ws2, &state, &cfg)
        .output()
        .expect("run third dmuxai");
    assert!(out3.status.success());
    assert_eq!(String::from_utf8_lossy(&out3.stdout), stdout2);
}

// TC-2974 / FR-2974: `--new` skips reuse, starts a separate dashboard without replacing
// the user-level discovery file, and `open_dashboard_args` passes `--new`.
#[test]
fn tc_2974_new_flag_skips_reuse_and_spawn_dashboard_passes_new() {
    let args = dmuxai::workspace::open_dashboard_args(Path::new("/tmp/w"));
    assert!(
        args.iter().any(|a| a == "--new"),
        "open_dashboard_args must include --new: {args:?}"
    );

    let tmp = tempfile::tempdir().unwrap();
    let ws1 = tmp.path().join("ws1");
    let ws2 = tmp.path().join("ws2");
    let state = tmp.path().join("state");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();

    let (_guard1, url1) = spawn_and_read_url(
        &["--port", "0"],
        &ws1,
        &state,
        &cfg,
        &[("DMUXAI_TRUST_WORKSPACE_CONFIG", "1")],
    );
    let disc_path = state.join("web.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !disc_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    let disc_before = std::fs::read_to_string(&disc_path).unwrap();

    let (_guard2, url2) = spawn_and_read_url(&["--new", "--port", "0"], &ws2, &state, &cfg, &[]);
    assert_ne!(url1, url2, "--new must start a separate dashboard");
    let disc_after = std::fs::read_to_string(&disc_path).unwrap();
    assert_eq!(
        disc_before, disc_after,
        "--new must not replace the live web dashboard in user web.json"
    );
}

// TC-2979 / FR-2978: When repo defines `[[agents.custom]]`, reuse is refused and `dmuxai`
// prints one line explaining why and starts its own dashboard.
#[test]
fn tc_2979_repo_custom_agents_fall_back_to_own_dashboard() {
    let tmp = tempfile::tempdir().unwrap();
    let ws1 = tmp.path().join("ws1");
    let ws2 = tmp.path().join("ws2");
    let state = tmp.path().join("state");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();

    std::fs::write(
        ws2.join("dmuxai.toml"),
        "[[agents.custom]]\nname = \"my-agent\"\ncommand = [\"echo\"]\n",
    )
    .unwrap();

    let (_guard1, url1) = spawn_and_read_url(&["--port", "0"], &ws1, &state, &cfg, &[]);
    let disc_path = state.join("web.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !disc_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }

    let mut child2 = base_cmd(&ws2, &state, &cfg)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dmuxai in custom-agents ws");

    let stdout2 = child2.stdout.take().unwrap();
    let stderr2 = child2.stderr.take().unwrap();
    let _guard2 = ChildGuard(Some(child2));
    let (tx_out, rx_out) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout2);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if let Some(url) = line.strip_prefix("dmuxai dashboard: ") {
                let _ = tx_out.send(url.trim().to_owned());
                break;
            }
            line.clear();
        }
    });
    let (tx_err, rx_err) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr2);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line.contains("cannot host this workspace") {
                let _ = tx_err.send(line);
                break;
            }
            line.clear();
        }
    });

    let url2 = rx_out
        .recv_timeout(Duration::from_secs(20))
        .expect("must start own dashboard");
    let err_line = rx_err
        .recv_timeout(Duration::from_secs(5))
        .expect("must print notice on stderr");

    assert_ne!(url1, url2);
    assert!(err_line.contains("cannot host this workspace"));
}

// TC-2972 / FR-2972: Token-required reuse sends Bearer token and prints `?ws=<id>&token=<tok>`.
#[test]
fn tc_2972_token_required_reuse() {
    let tmp = tempfile::tempdir().unwrap();
    let ws1 = tmp.path().join("ws1");
    let ws2 = tmp.path().join("ws2");
    let state = tmp.path().join("state");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();

    let token = "test_reuse_token_12345678901234567890";
    let (_guard1, url1) = spawn_and_read_url(
        &["--port", "0"],
        &ws1,
        &state,
        &cfg,
        &[
            ("DMUXAI_WEB_REQUIRE_TOKEN", "1"),
            ("DMUXAI_WEB_TOKEN", token),
        ],
    );
    let base1 = url1.split("/?token=").next().unwrap().trim_end_matches('/');

    let disc_path = state.join("web.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !disc_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }

    let out2 = base_cmd(&ws2, &state, &cfg)
        .output()
        .expect("run second dmuxai");
    assert!(
        out2.status.success(),
        "expected exit 0, stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    let expected_prefix = format!("dmuxai dashboard: {base1}/?ws=");
    assert!(
        stdout2.contains(&expected_prefix) && stdout2.contains(&format!("&token={token}")),
        "expected `{expected_prefix}<id>&token={token}`, got: {stdout2:?}"
    );
}

// TC-2973 / FR-2973: Full dashboard exits 4; stale/serve discovery file is taken over.
#[test]
fn tc_2973_cli_full_exits_4_and_stale_serve_takeover() {
    let tmp = tempfile::tempdir().unwrap();
    let ws1 = tmp.path().join("ws1");
    let ws2 = tmp.path().join("ws2");
    let state = tmp.path().join("state");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();

    std::fs::write(ws1.join("dmuxai.toml"), "[web]\nmax_workspaces = 1\n").unwrap();
    let (_guard1, _url1) = spawn_and_read_url(&["--port", "0"], &ws1, &state, &cfg, &[]);
    let disc_path = state.join("web.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !disc_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }

    let out_full = base_cmd(&ws2, &state, &cfg)
        .output()
        .expect("run second dmuxai when full");
    assert_eq!(
        out_full.status.code(),
        Some(4),
        "expected exit 4 when dashboard is full, stderr: {}",
        String::from_utf8_lossy(&out_full.stderr)
    );

    // Takeover of stale/serve web.json: write a stale `mode = "serve"` discovery file.
    let stale_state = tmp.path().join("stale_state");
    std::fs::create_dir_all(&stale_state).unwrap();
    std::fs::write(
        stale_state.join("web.json"),
        r#"{"base_url":"http://127.0.0.1:1","port":1,"pid":999999999,"uid":0,"mode":"serve","token_required":false,"workspaces":[]}"#,
    )
    .unwrap();
    let (_guard_takeover, url_takeover) =
        spawn_and_read_url(&["--port", "0"], &ws2, &stale_state, &cfg, &[]);
    assert!(url_takeover.starts_with("http://127.0.0.1:"));
    let taken_over: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(stale_state.join("web.json")).unwrap())
            .unwrap();
    assert_eq!(taken_over["mode"], "web");
}

// TC-2974 / FR-2974: Busy port falls back to a free port when `--new --port <busy>` is used.
#[test]
fn tc_2974_busy_port_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let state = tmp.path().join("state");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();

    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let busy_str = busy_port.to_string();

    let (_guard, url) = spawn_and_read_url(&["--new", "--port", &busy_str], &ws, &state, &cfg, &[]);
    assert!(
        !url.contains(&format!(":{busy_port}")),
        "expected fallback to a free port, got {url}"
    );
}

// TC-2970 / TC-2971 / TC-2976: invalid [ui] default_mode, explicit web flags, denied cwd, non-TTY tui.
#[test]
fn tc_2970_config_error_explicit_flags_denied_cwd_and_non_tty_tui() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let state = tmp.path().join("state");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();

    // 1. Invalid `[ui] default_mode` exits 2 with key name on stderr.
    std::fs::write(ws.join("dmuxai.toml"), "[ui]\ndefault_mode = \"bogus\"\n").unwrap();
    let out_bad = base_cmd(&ws, &state, &cfg).output().unwrap();
    assert_eq!(out_bad.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out_bad.stderr).contains("[ui] default_mode"),
        "stderr: {}",
        String::from_utf8_lossy(&out_bad.stderr)
    );

    // 2. Resolved tui mode (`[ui] default_mode = "tui"`) rejects web-only flags with exit 2.
    std::fs::write(ws.join("dmuxai.toml"), "[ui]\ndefault_mode = \"tui\"\n").unwrap();
    let out_tui_flag = base_cmd(&ws, &state, &cfg)
        .args(["--port", "9000"])
        .output()
        .unwrap();
    assert_eq!(out_tui_flag.status.code(), Some(2));

    // 3. Non-TTY tui mode exits 1 BEFORE touching recent-workspaces.json.
    let out_nontty = base_cmd(&ws, &state, &cfg).output().unwrap();
    assert_eq!(out_nontty.status.code(), Some(1));
    assert!(
        !state.join("recent-workspaces.json").exists(),
        "non-TTY tui mode must fail before writing recent-workspaces.json"
    );

    // 4. Invalid `--bind 0.0.0.0` (without `--allow-remote`) exits 2 even when a dashboard is live.
    std::fs::remove_file(ws.join("dmuxai.toml")).unwrap();
    let out_bind = base_cmd(&ws, &state, &cfg)
        .args(["--bind", "0.0.0.0"])
        .output()
        .unwrap();
    assert_eq!(out_bind.status.code(), Some(2));

    // 5. Plain `dmuxai` from denied cwd `/` exits 2 and does not start a server.
    let out_root = base_cmd(Path::new("/"), &state, &cfg).output().unwrap();
    assert_eq!(out_root.status.code(), Some(2));
}
