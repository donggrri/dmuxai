//! TC-2314 (SPEC-multi-workspace-host FR-2300, FR-2301, FR-2303, FR-2304): a headless `--web` dashboard keeps
//! the first folder running while a second folder is opened in-process, can
//! address each folder through `?ws=<id>`, and rejects unknown ids.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
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

fn json_body(response: &str) -> serde_json::Value {
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(response);
    serde_json::from_str(body).unwrap_or_else(|error| {
        panic!("expected a JSON body, got {error}: {response}");
    })
}

fn base_command(workspace: &Path, state: &Path, config: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    cmd.args(["--web", "-C"])
        .arg(workspace)
        .args(["--port", "0", "--bind", "127.0.0.1"])
        .env("DMUXAI_STATE_DIR", state)
        .env("HOME", state)
        .env("XDG_CONFIG_HOME", config)
        .env_remove("DMUXAI_CONFIG")
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_WORKSPACE")
        .env_remove("DMUXAI_HEADLESS_DASHBOARD")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    cmd
}

fn workspace_name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

fn git_init(dir: &Path) {
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .expect("run git init");
    assert!(status.success(), "git init failed in {}", dir.display());
}

// TC-2314 / FR-2303: second_folder_opens_while_first_runs
#[test]
fn tc_2314_second_folder_opens_while_first_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let ws1 = tmp.path().join("ws1");
    let ws2 = tmp.path().join("ws2");
    let state = tmp.path().join("state");
    let config = tmp.path().join("config");
    for dir in [&ws1, &ws2, &state, &config] {
        std::fs::create_dir_all(dir).unwrap();
    }
    git_init(&ws1);
    git_init(&ws2);

    let config_dir = config.join("dmuxai");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("dmuxai.toml"),
        r#"[graph]
enabled = false

[[agents.custom]]
name = "slow"
command = [
  "sh",
  "-c",
  "sleep 20; printf '%s\\n' '{\"type\":\"text\",\"part\":{\"text\":\"slow done\"}}'",
  "{prompt}",
]
prompt_via = "arg"
output = "jsonl"
text_path = "/part/text"
text_when = { path = "/type", equals = "text" }
timeout_secs = 60
"#,
    )
    .unwrap();

    let child = base_command(&ws1, &state, &config).spawn().unwrap();
    let mut guard = ChildGuard(Some(child));
    let discovery_path = ws1.join(".dmuxai").join("web.json");
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

    // Start a slow direct turn in the first folder.
    let prompt = http(
        port,
        "POST",
        "/api/prompt",
        &token,
        r#"{"text":"hello","agent":"slow","route":"direct"}"#,
    );
    assert!(prompt.starts_with("HTTP/1.1 202"), "{prompt}");

    // FR-2314: the default folder reports a running turn while it is hosted.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let list = http(port, "GET", "/api/workspaces", &token, "");
        assert!(list.starts_with("HTTP/1.1 200"), "{list}");
        let list = json_body(&list);
        let default = list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == "default")
            .expect("default workspace entry");
        if default["running"] == true {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "default folder never reported running: {list}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let ws2_canonical = std::fs::canonicalize(&ws2).unwrap();
    let switch_body = serde_json::json!({ "path": ws2_canonical.to_string_lossy() }).to_string();
    let switched = http(port, "POST", "/api/workspace/switch", &token, &switch_body);
    assert!(switched.starts_with("HTTP/1.1 200"), "{switched}");
    let switched = json_body(&switched);
    assert_eq!(switched["ok"], true);
    assert_eq!(switched["in_process"], true);
    assert_eq!(switched["opened"], true);
    let ws2_id = switched["id"].as_str().unwrap().to_owned();

    let new_conversation = http(
        port,
        "POST",
        &format!("/api/command?ws={ws2_id}"),
        &token,
        r#"{"command":"new_conversation"}"#,
    );
    assert!(
        new_conversation.starts_with("HTTP/1.1 2"),
        "{new_conversation}"
    );

    let state2 = http(port, "GET", &format!("/api/state?ws={ws2_id}"), &token, "");
    assert!(state2.starts_with("HTTP/1.1 200"), "{state2}");
    let state2 = json_body(&state2);
    let state2_workspace = state2["workspace"].as_str().unwrap();
    assert_eq!(state2_workspace, ws2_canonical.to_string_lossy());
    assert!(
        state2_workspace.contains(&workspace_name(&ws2)),
        "{state2_workspace}"
    );

    // The first folder is still running even though focus moved to the second.
    let list = http(port, "GET", "/api/workspaces", &token, "");
    assert!(list.starts_with("HTTP/1.1 200"), "{list}");
    let list = json_body(&list);
    let entries = list["workspaces"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{list}");
    let default = entries
        .iter()
        .find(|entry| entry["id"] == "default")
        .expect("default entry");
    assert_eq!(default["running"], true, "{list}");
    let opened = entries
        .iter()
        .find(|entry| entry["id"] == ws2_id.as_str())
        .expect("new workspace entry");
    assert_eq!(opened["focused"], true, "{list}");

    let state1 = http(port, "GET", "/api/state?ws=default", &token, "");
    assert!(state1.starts_with("HTTP/1.1 200"), "{state1}");
    let state1 = json_body(&state1);
    assert_eq!(
        state1["workspace"].as_str().unwrap(),
        std::fs::canonicalize(&ws1).unwrap().to_string_lossy()
    );
    let turn_running = state1["turn"]["state"] == "running"
        || state1["agents"]
            .as_array()
            .is_some_and(|agents| agents.iter().any(|agent| agent["busy"] == true));
    assert!(turn_running, "default turn is still running: {state1}");

    let ws1_canonical = std::fs::canonicalize(&ws1).unwrap();
    let switch_back = serde_json::json!({ "path": ws1_canonical.to_string_lossy() }).to_string();
    let focused = http(port, "POST", "/api/workspace/switch", &token, &switch_back);
    assert!(focused.starts_with("HTTP/1.1 200"), "{focused}");
    let focused = json_body(&focused);
    assert_eq!(focused["opened"], false);
    assert_eq!(focused["id"], "default");

    let missing = http(port, "GET", "/api/state?ws=nope", &token, "");
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
    assert_eq!(json_body(&missing)["error"], "no such open folder");

    let quit = http(
        port,
        "POST",
        "/api/command",
        &token,
        r#"{"command":"quit"}"#,
    );
    assert!(quit.starts_with("HTTP/1.1 200"), "{quit}");
    let deadline = Instant::now() + Duration::from_secs(20);
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
