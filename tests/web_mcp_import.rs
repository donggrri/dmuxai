//! TC-569 / INT-563, NFR-560: end-to-end import from mock MCP server via `dmuxai serve`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Kills and reaps the child on every exit path, including panics.
struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn http_get(authority: &str, path: &str, token: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(authority).expect("connect to dashboard");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .expect("write http request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read http response");
    let response = String::from_utf8_lossy(&response).into_owned();

    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or("")
        .to_owned();
    (status, body)
}

fn http_post(authority: &str, path: &str, body: &str, token: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(authority).expect("connect to dashboard");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");

    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("write http request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read http response");
    let response = String::from_utf8_lossy(&response).into_owned();

    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or("")
        .to_owned();
    (status, body)
}

fn parse_json_payload(body: &str) -> serde_json::Value {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        return v;
    }
    let first_brace = body.find('{');
    let first_bracket = body.find('[');
    let (start, is_brace) = match (first_brace, first_bracket) {
        (Some(b), Some(k)) if b < k => (Some(b), true),
        (Some(_), Some(k)) => (Some(k), false),
        (Some(b), None) => (Some(b), true),
        (None, Some(k)) => (Some(k), false),
        (None, None) => (None, false),
    };
    if let Some(start) = start {
        let end = if is_brace {
            body.rfind('}')
        } else {
            body.rfind(']')
        };
        if let Some(end) = end
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&body[start..=end])
        {
            return v;
        }
    }
    panic!("could not parse json from response body: {body}");
}

#[test]
fn serve_imports_from_mock_mcp_end_to_end() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let state_dir = tempfile::tempdir().expect("temp state dir");

    let bin_path = env!("CARGO_BIN_EXE_dmuxai").replace('\\', "\\\\");
    let config = format!(
        r#"[graph]
enabled = false

[[mcp.servers]]
name = "jira-mock"
kind = "jira"
command = "{bin_path}"
args = ["__mock-mcp"]
"#
    );
    std::fs::write(workspace.path().join("dmuxai.toml"), config).expect("write dmuxai.toml");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace.path())
        .env("DMUXAI_STATE_DIR", state_dir.path())
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dmuxai serve");

    let stdout = child.stdout.take().expect("stdout is piped");
    let guard = ChildGuard(Some(child));

    // Read first stdout line bounded.
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
        Err(error) => panic!("timed out waiting for dashboard URL: {error}"),
    };

    let url = first_line
        .trim()
        .strip_prefix("dmuxai dashboard: ")
        .unwrap_or_else(|| panic!("unexpected first stdout line: {first_line:?}"));

    let rest = url
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("expected an http URL, got {url:?}"));
    let (authority, query) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("expected a path in {url:?}"));
    assert!(!authority.ends_with(":0"), "port should be resolved");
    let token = query
        .strip_prefix("?token=")
        .unwrap_or_else(|| panic!("expected a ?token= query in {url:?}"));

    // 1. GET /api/mcp/servers lists jira-mock
    let (status, body) = http_get(authority, "/api/mcp/servers", token);
    assert_eq!(status, 200, "GET /api/mcp/servers failed: {body}");
    assert!(
        body.contains("jira-mock"),
        "servers list should contain 'jira-mock': {body}"
    );

    // 2. POST /api/import {"server":"jira-mock","query":"DEMO"} -> 2 imported
    let (status, body) = http_post(
        authority,
        "/api/import",
        r#"{"server":"jira-mock","query":"DEMO"}"#,
        token,
    );
    assert_eq!(status, 200, "first POST /api/import failed: {body}");
    let val1 = parse_json_payload(&body);
    assert_eq!(val1["imported"], 2, "expected 2 imported: {val1}");
    assert_eq!(val1["updated"], 0, "expected 0 updated: {val1}");

    // 3. POST /api/import again -> 0 imported 2 updated
    let (status, body) = http_post(
        authority,
        "/api/import",
        r#"{"server":"jira-mock","query":"DEMO"}"#,
        token,
    );
    assert_eq!(status, 200, "second POST /api/import failed: {body}");
    let val2 = parse_json_payload(&body);
    assert_eq!(
        val2["imported"], 0,
        "expected 0 imported on re-import: {val2}"
    );
    assert_eq!(
        val2["updated"], 2,
        "expected 2 updated on re-import: {val2}"
    );

    // 4. GET /api/state has 2 todos with source jira and source_ref DEMO-1/DEMO-2
    let (status, body) = http_get(authority, "/api/state", token);
    assert_eq!(status, 200, "GET /api/state failed: {body}");
    let state = parse_json_payload(&body);
    let todos = state["todos"]
        .as_array()
        .expect("todos array in state snapshot");
    assert_eq!(todos.len(), 2, "expected 2 todos in state: {todos:?}");
    assert!(
        todos.iter().all(|t| t["source"] == "jira"),
        "all todos should have source 'jira': {todos:?}"
    );
    assert!(
        todos.iter().any(|t| t["source_ref"] == "DEMO-1"),
        "missing DEMO-1 in todos: {todos:?}"
    );
    assert!(
        todos.iter().any(|t| t["source_ref"] == "DEMO-2"),
        "missing DEMO-2 in todos: {todos:?}"
    );

    // If external_key/link_url fields are present, verify them
    let demo1 = todos.iter().find(|t| t["source_ref"] == "DEMO-1").unwrap();
    if let Some(key) = demo1.get("external_key").and_then(|k| k.as_str()) {
        assert_eq!(key, "DEMO-1");
    }
    if let Some(link) = demo1.get("link_url").and_then(|l| l.as_str()) {
        assert!(link.contains("DEMO-1"));
    }

    drop(guard);
}

#[test]
fn serve_snapshot_has_jira_base_url_at_startup() {
    // TC-579: dmuxai serve with [todo] jira_base_url seeds the hub snapshot at startup before any mutation.
    let workspace = tempfile::tempdir().expect("temp workspace");
    let state_dir = tempfile::tempdir().expect("temp state dir");

    let config = r#"[graph]
enabled = false

[todo]
jira_base_url = "https://jira.example.invalid"
"#;
    std::fs::write(workspace.path().join("dmuxai.toml"), config).expect("write dmuxai.toml");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace.path())
        .env("DMUXAI_STATE_DIR", state_dir.path())
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dmuxai serve");

    let stdout = child.stdout.take().expect("stdout is piped");
    let guard = ChildGuard(Some(child));

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
        Err(error) => panic!("timed out waiting for dashboard URL: {error}"),
    };

    let url = first_line
        .trim()
        .strip_prefix("dmuxai dashboard: ")
        .unwrap_or_else(|| panic!("unexpected first stdout line: {first_line:?}"));

    let rest = url
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("expected an http URL, got {url:?}"));
    let (authority, query) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("expected a path in {url:?}"));
    let token = query
        .strip_prefix("?token=")
        .unwrap_or_else(|| panic!("expected a ?token= query in {url:?}"));

    // First GET /api/state before any mutation
    let (status, body) = http_get(authority, "/api/state", token);
    assert_eq!(status, 200, "GET /api/state failed: {body}");
    let state = parse_json_payload(&body);
    assert_eq!(
        state["jira_base_url"], "https://jira.example.invalid",
        "expected jira_base_url in initial snapshot: {state}"
    );

    drop(guard);
}
