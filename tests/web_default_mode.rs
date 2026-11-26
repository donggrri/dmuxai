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
    let req = format!("GET /{path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (_, body) = raw.split_once("\r\n\r\n").unwrap();
    body.to_owned()
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
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    cmd.args(args)
        .current_dir(cwd)
        .env("DMUXAI_STATE_DIR", state)
        .env("XDG_STATE_HOME", state)
        .env("XDG_CONFIG_HOME", cfg)
        .env_remove("DMUXAI_UI")
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    let out2 = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .current_dir(&ws2)
        .env("DMUXAI_STATE_DIR", &state)
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", &cfg)
        .env_remove("DMUXAI_UI")
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
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
    let out3 = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .current_dir(&ws2)
        .env("DMUXAI_STATE_DIR", &state)
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", &cfg)
        .env_remove("DMUXAI_UI")
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
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

    let mut child2 = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["--port", "0"])
        .current_dir(&ws2)
        .env("DMUXAI_STATE_DIR", &state)
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", &cfg)
        .env("DMUXAI_TRUST_WORKSPACE_CONFIG", "1")
        .env_remove("DMUXAI_UI")
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dmuxai in custom-agents ws");

    let stdout2 = child2.stdout.take().unwrap();
    let stderr2 = child2.stderr.take().unwrap();
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
    let _ = child2.kill();
    let _ = child2.wait();

    assert_ne!(url1, url2);
    assert!(err_line.contains("cannot host this workspace"));
}
