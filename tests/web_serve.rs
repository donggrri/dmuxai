//! TC-531 / FR-520, FR-521: `dmuxai serve` prints a token URL and serves state.

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

fn http_get(authority: &str, path: &str, token: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(authority).expect("connect to dashboard");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");

    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n");
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
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
    (status, response)
}

/// A graph fixture whose plan carries edges, for TC-702 / FR-701 / NFR-700.
fn write_graph_fixture(workspace: &std::path::Path) {
    let graphs = workspace.join(".dmuxai").join("graphs");
    std::fs::create_dir_all(&graphs).expect("create graphs dir");
    let run = serde_json::json!({
        "plan": {
            "schema": "dmuxai.graph/v1",
            "id": "serve-run",
            "orchestrator_model": "gpt-4o",
            "user_prompt": "serve graph",
            "limits": { "max_parallel": 2, "max_retries": 2 },
            "nodes": [
                {
                    "id": "a", "role": "plan", "job": "root",
                    "runner": "codex", "model_id": "m", "skill": null,
                    "requires_gate": false, "status": "completed", "attempt": 1
                },
                {
                    "id": "b", "role": "implement", "job": "left",
                    "runner": "codex", "model_id": "m", "skill": null,
                    "requires_gate": false, "status": "running", "attempt": 1
                },
                {
                    "id": "c", "role": "review", "job": "right",
                    "runner": "codex", "model_id": "m", "skill": null,
                    "requires_gate": false, "status": "pending", "attempt": 0
                }
            ],
            "edges": [
                { "from": "a", "to": "b" },
                { "from": "a", "to": "c" }
            ]
        },
        "conversation_id": "conv-1",
        "phase": "active",
        "waiting_gate_node": null
    });
    std::fs::write(
        graphs.join("serve.json"),
        serde_json::to_vec_pretty(&run).expect("serialize graph fixture"),
    )
    .expect("write graph fixture");
}

#[test]
fn serve_prints_token_url_and_serves_state() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    write_graph_fixture(workspace.path());

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(["serve", "--port", "0"])
        .current_dir(workspace.path())
        .env_remove("DMUXAI_WORKSPACE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dmuxai serve");

    let stdout = child.stdout.take().expect("stdout is piped");
    let guard = ChildGuard(Some(child));

    // Read the first stdout line in a worker thread so we can enforce a bound.
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
    assert!(token.len() >= 32, "token too short: {token:?}");
    assert!(
        token
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
        "token is not lowercase hex: {token:?}"
    );

    let (with_token_status, with_token_body) = http_get(authority, "/api/state", Some(token));
    assert_eq!(
        with_token_status, 200,
        "expected 200 with token, body: {with_token_body}"
    );
    assert!(
        with_token_body.contains("dmuxai.dashboard/v1"),
        "body missing snapshot schema: {with_token_body}"
    );

    // TC-702 / FR-701 / NFR-700: the file-sourced graph exposes deps, layout
    // and layers. The serve loop polls on an interval, so retry briefly.
    let mut graph_body = with_token_body.clone();
    for _ in 0..40 {
        if graph_body.contains("\"deps\":[\"a\"]")
            && graph_body.contains("\"layout\":{\"layer\":1,\"order\":0}")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
        let (_, body) = http_get(authority, "/api/state", Some(token));
        graph_body = body;
    }
    assert!(
        graph_body.contains("\"deps\":[\"a\"]"),
        "served graph node missing deps: {graph_body}"
    );
    assert!(
        graph_body.contains("\"layout\":{\"layer\":1,\"order\":0}"),
        "served graph node missing layout: {graph_body}"
    );
    assert!(
        graph_body.contains("\"layers\":2"),
        "served graph missing layers count: {graph_body}"
    );

    let (without_token_status, _) = http_get(authority, "/api/state", None);
    assert_eq!(without_token_status, 401, "expected 401 without token");

    drop(guard);
}
