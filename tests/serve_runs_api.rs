//! Runs API integration tests (SPEC-headless-runs-dashboard FR-2806..FR-2809 / TC-2806..TC-2809).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct ServeFixture {
    _workspace: tempfile::TempDir,
    workspace: PathBuf,
    data_home: PathBuf,
    authority: String,
    token: String,
    _guard: ChildGuard,
}

impl ServeFixture {
    fn new() -> Self {
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
        let mut reader = BufReader::new(stdout);
        let mut first_line = String::new();
        reader
            .read_line(&mut first_line)
            .expect("read dashboard url line");
        let url = first_line
            .trim()
            .strip_prefix("dmuxai dashboard: ")
            .unwrap_or_else(|| panic!("unexpected stdout: {first_line:?}"));
        let rest = url
            .strip_prefix("http://")
            .unwrap_or_else(|| panic!("expected http url: {url:?}"));
        let (authority, query) = rest
            .split_once('/')
            .unwrap_or_else(|| panic!("expected path in url"));
        let token = query
            .strip_prefix("?token=")
            .unwrap_or_else(|| panic!("expected token query"))
            .to_string();

        Self {
            _workspace: workspace,
            workspace: ws_path,
            data_home,
            authority: authority.to_string(),
            token,
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

fn with_token(path: &str, token: Option<&str>) -> String {
    match token {
        Some(t) => {
            if path.contains('?') {
                format!("{path}&token={t}")
            } else {
                format!("{path}?token={t}")
            }
        }
        None => path.to_string(),
    }
}

fn http_request(
    authority: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (u16, String) {
    let path = with_token(path, token);
    let mut stream = TcpStream::connect(authority).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n");
    if let Some(body) = body {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    if let Some(body) = body {
        req.push_str(body);
    }
    stream.write_all(req.as_bytes()).expect("write request");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    let response = String::from_utf8_lossy(&response).into_owned();
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (status, response)
}

fn http_get(authority: &str, path: &str, token: Option<&str>) -> (u16, String) {
    http_request(authority, "GET", path, token, None)
}

fn json_body(response: &str) -> Value {
    let body = response.split("\r\n\r\n").nth(1).unwrap_or(response);
    serde_json::from_str(body.trim()).unwrap_or_else(|e| panic!("json parse: {e}\n{response}"))
}

fn write_manifest(run_dir: &Path, manifest: &Value) {
    fs::create_dir_all(run_dir).expect("run dir");
    fs::write(
        run_dir.join("manifest.json"),
        serde_json::to_vec_pretty(manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
}

fn base_manifest(run_id: &str, created_at_ms: u64, status: &str) -> Value {
    json!({
        "schema_version": 1,
        "run_id": run_id,
        "origin": "cli_headless",
        "status": status,
        "created_at_ms": created_at_ms,
        "completed_at_ms": null,
        "duration_ms": null,
        "requested_runner": "codex",
        "resolved_runner": null,
        "requested_model": null,
        "resolved_model": null,
        "dmuxai_version": "0.0.0",
        "build_commit": null,
        "pid": null,
        "dmuxai_exit_code": null,
        "child_exit_code": null,
        "child_signal": null,
        "error_kind": null,
        "error_summary": null,
        "links": {
            "conversation_id": null,
            "turn_id": null,
            "graph_id": null,
            "node_id": null,
            "attempt_index": null
        },
        "diagnostics": { "stderr_bytes": 0, "tail_sample": null },
        "end_summary": null
    })
}

fn write_graph_fixture(workspace: &Path, graph_id: &str) {
    let graphs = workspace.join(".dmuxai").join("graphs");
    fs::create_dir_all(&graphs).expect("graphs dir");
    let run = json!({
        "plan": {
            "schema": "dmuxai.graph/v1",
            "id": "serve-run",
            "orchestrator_model": "gpt-4o",
            "user_prompt": "serve graph",
            "limits": { "max_parallel": 2, "max_retries": 2 },
            "nodes": [{
                "id": "a", "role": "plan", "job": "root",
                "runner": "codex", "model_id": "m", "skill": null,
                "requires_gate": false, "status": "completed", "attempt": 1
            }],
            "edges": []
        },
        "conversation_id": "conv-1",
        "phase": "active",
        "waiting_gate_node": null
    });
    fs::write(
        graphs.join(format!("{graph_id}.json")),
        serde_json::to_vec_pretty(&run).expect("graph json"),
    )
    .expect("write graph");
}

fn append_event_line(path: &Path, seq: u64, run_id: Option<&str>) {
    let mut record = json!({
        "v": 1,
        "seq": seq,
        "ts": "2026-01-01T00:00:00.000Z",
        "ts_ms": 1,
        "workspace": "/ws",
        "kind": "agent",
        "to_state": "running"
    });
    if let Some(id) = run_id {
        record["run_id"] = json!(id);
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open events");
    use std::io::Write;
    writeln!(file, "{record}").expect("append event");
}

// TC-2806 / FR-2806: list runs newest-first, stale flags, limit validation, skip malformed
#[test]
fn tc_2806_runs_list() {
    let fixture = ServeFixture::new();
    let runs = fixture.runs_dir();
    fs::create_dir_all(&runs).expect("runs root");

    let alive = "aaaaaaaaaaaaaaaa";
    let dead = "bbbbbbbbbbbbbbbb";
    let done = "cccccccccccccccc";
    let bad = "dddddddddddddddd";

    let mut m1 = base_manifest(alive, 4_000, "active");
    m1["pid"] = json!(std::process::id());
    write_manifest(&runs.join(alive), &m1);

    let mut m2 = base_manifest(dead, 3_000, "active");
    m2["pid"] = json!(999_999);
    write_manifest(&runs.join(dead), &m2);

    let mut m3 = base_manifest(done, 2_000, "completed");
    m3["completed_at_ms"] = json!(2_100);
    m3["duration_ms"] = json!(100);
    write_manifest(&runs.join(done), &m3);

    fs::create_dir_all(runs.join(bad)).expect("bad dir");
    fs::write(runs.join(bad).join("manifest.json"), "{ not json").expect("bad manifest");

    let (status, body) = http_get(&fixture.authority, "/api/runs", Some(&fixture.token));
    assert_eq!(status, 200);
    let json = json_body(&body);
    let ids: Vec<&str> = json["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .map(|r| r["run_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![alive, dead, done]);

    let by_id: std::collections::HashMap<_, _> = json["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["run_id"].as_str().unwrap(), r))
        .collect();
    #[cfg(target_os = "linux")]
    {
        assert_eq!(by_id[alive]["stale"], false);
        assert_eq!(by_id[dead]["stale"], true);
    }
    assert_eq!(by_id[done]["stale"], false);

    let (s_limit, b_limit) = http_get(
        &fixture.authority,
        "/api/runs?limit=1",
        Some(&fixture.token),
    );
    assert_eq!(s_limit, 200);
    assert_eq!(json_body(&b_limit)["runs"].as_array().unwrap().len(), 1);

    let (s_zero, _) = http_get(
        &fixture.authority,
        "/api/runs?limit=0",
        Some(&fixture.token),
    );
    assert_eq!(s_zero, 400);
}

// TC-2807 / FR-2807: detail graph, approval hint, progress tail, 400/404
#[test]
fn tc_2807_run_detail_graph_and_approval() {
    let fixture = ServeFixture::new();
    write_graph_fixture(&fixture.workspace, "serve");
    let runs = fixture.runs_dir();
    let run_id = "eeeeeeeeeeeeeeee";
    let mut manifest = base_manifest(run_id, 5_000, "awaiting_approval");
    manifest["links"]["graph_id"] = json!("serve");
    manifest["end_summary"] = json!({ "plan_hash": "deadbeefdeadbeef" });
    write_manifest(&runs.join(run_id), &manifest);

    let progress_path = runs.join(run_id).join("progress.jsonl");
    fs::create_dir_all(runs.join(run_id)).expect("run dir");
    fs::write(&progress_path, "{\"line\":1}\n{\"line\":2}\n{\"line\":3}\n").expect("progress");

    let (status, body) = http_get(
        &fixture.authority,
        &format!("/api/runs/{run_id}"),
        Some(&fixture.token),
    );
    assert_eq!(status, 200);
    let json = json_body(&body);
    assert!(json["graph"].is_object());
    let approval = &json["approval"];
    assert!(
        approval["hint"]
            .as_str()
            .unwrap()
            .contains("--approve-plan-id deadbeefdeadbeef")
    );
    assert_eq!(json["progress"].as_array().unwrap().len(), 3);

    let (s400, _) = http_get(
        &fixture.authority,
        "/api/runs/not-valid",
        Some(&fixture.token),
    );
    assert_eq!(s400, 400);

    let (s404, _) = http_get(
        &fixture.authority,
        "/api/runs/ffffffffffffffff",
        Some(&fixture.token),
    );
    assert_eq!(s404, 404);
}

// TC-2808 / FR-2808: SSE emits run, progress, filtered state-change, end
#[test]
fn tc_2808_run_stream_sse() {
    let fixture = ServeFixture::new();
    let runs = fixture.runs_dir();
    let run_id = "6666666666666666";
    let mut manifest = base_manifest(run_id, 6_000, "completed");
    manifest["completed_at_ms"] = json!(6_500);
    manifest["duration_ms"] = json!(500);
    write_manifest(&runs.join(run_id), &manifest);

    let progress_path = runs.join(run_id).join("progress.jsonl");
    fs::write(&progress_path, "{\"tick\":1}\n").expect("progress line");

    let events_path = fixture.workspace.join(".dmuxai").join("events.jsonl");
    append_event_line(&events_path, 1, Some(run_id));
    append_event_line(&events_path, 2, Some("0000000000000001"));

    let path = format!("/api/runs/{run_id}/stream");
    let full_path = with_token(&path, Some(&fixture.token));

    let mut stream = TcpStream::connect(&fixture.authority).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let req = format!(
        "GET {full_path} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n",
        fixture.authority
    );
    stream.write_all(req.as_bytes()).expect("write sse");

    let mut buf = Vec::new();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(20);
    let mut temp = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut temp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&temp[..n]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => break,
        }
        if String::from_utf8_lossy(&buf).contains("event: end") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    assert!(text.contains("event: run"), "missing run event: {text}");
    assert!(text.contains("event: progress"), "missing progress: {text}");
    let matching_state_change = text.split("\n\n").any(|block| {
        block
            .lines()
            .any(|line| line.trim() == "event: state-change")
            && (block.contains("\"run_id\":\"6666666666666666\"")
                || block.contains("\"run_id\": \"6666666666666666\""))
    });
    assert!(
        matching_state_change,
        "missing matching state-change: {text}"
    );
    assert!(
        !text.contains("0000000000000001"),
        "must not stream other run_id events: {text}"
    );
    assert!(text.contains("event: end"), "missing end: {text}");
}

// TC-2809 / FR-2809: auth, read-only POST, masked secrets, no token in body
#[test]
fn tc_2809_runs_api_read_only_and_masked() {
    let fixture = ServeFixture::new();
    let runs = fixture.runs_dir();
    let run_id = "1212121212121212";
    let mut manifest = base_manifest(run_id, 7_000, "failed");
    manifest["error_summary"] = json!(format!(
        "failed with API_KEY=sk-test-123456 and dashboard token {}",
        fixture.token
    ));
    write_manifest(&runs.join(run_id), &manifest);

    let progress_path = runs.join(run_id).join("progress.jsonl");
    fs::create_dir_all(runs.join(run_id)).expect("run dir");
    fs::write(
        &progress_path,
        format!("{{\"line\":\"token {}\"}}\n", fixture.token),
    )
    .expect("progress");

    let (s401, _) = http_get(&fixture.authority, "/api/runs", None);
    assert_eq!(s401, 401);

    let (s405, _body_post) = http_request(
        &fixture.authority,
        "POST",
        "/api/runs",
        Some(&fixture.token),
        Some("{}"),
    );
    assert_eq!(s405, 405);

    let (_, body) = http_get(&fixture.authority, "/api/runs", Some(&fixture.token));
    assert!(
        !body.contains(&fixture.token),
        "list response must not echo dashboard token"
    );
    let json = json_body(&body);
    let summary = json["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == run_id)
        .and_then(|r| r["error_summary"].as_str())
        .expect("summary");
    assert!(!summary.contains("sk-test-123456"));
    assert!(summary.contains("[REDACTED]"));

    let (status, detail_body) = http_get(
        &fixture.authority,
        &format!("/api/runs/{run_id}"),
        Some(&fixture.token),
    );
    assert_eq!(status, 200);
    assert!(
        !detail_body.contains(&fixture.token),
        "detail response must not echo dashboard token"
    );
    let detail = json_body(&detail_body);
    assert!(
        detail["progress"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.to_string().contains("[REDACTED]"))
    );

    let path = format!("/api/runs/{run_id}/stream");
    let full_path = with_token(&path, Some(&fixture.token));
    let mut stream = TcpStream::connect(&fixture.authority).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let req = format!(
        "GET {full_path} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n",
        fixture.authority
    );
    stream.write_all(req.as_bytes()).expect("write sse");

    let mut buf = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut temp = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut temp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&temp[..n]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => break,
        }
        if String::from_utf8_lossy(&buf).contains("event: end") {
            break;
        }
    }
    let stream_text = String::from_utf8_lossy(&buf);
    assert!(
        !stream_text.contains(&fixture.token),
        "stream must not echo dashboard token"
    );
    assert!(stream_text.contains("[REDACTED]"));
}

// TC-2816 / FR-2816: `approved` is a terminal status, gets no approval hint,
// and exposes `approved_by_run_id` in detail and list payloads.
#[test]
fn tc_2816_approved_run_has_no_hint() {
    let fixture = ServeFixture::new();
    let runs = fixture.runs_dir();
    let approved_id = "aaaaaaaaaaaaaaaa";
    let approver_id = "ffffffffffffffff";
    let mut manifest = base_manifest(approved_id, 8_000, "approved");
    manifest["completed_at_ms"] = json!(8_500);
    manifest["duration_ms"] = json!(500);
    manifest["end_summary"] = json!({
        "plan_hash": "deadbeefdeadbeef",
        "approved_by_run_id": approver_id
    });
    write_manifest(&runs.join(approved_id), &manifest);

    // Detail: no approval payload, approved_by_run_id surfaced.
    let (status, body) = http_get(
        &fixture.authority,
        &format!("/api/runs/{approved_id}"),
        Some(&fixture.token),
    );
    assert_eq!(status, 200);
    let detail = json_body(&body);
    assert!(
        detail["approval"].is_null(),
        "approved run must have no approval hint: {detail}"
    );
    assert_eq!(detail["run"]["status"], "approved");
    assert_eq!(detail["run"]["approved_by_run_id"], approver_id);
    assert_eq!(
        detail["manifest"]["end_summary"]["approved_by_run_id"],
        approver_id
    );

    // List: terminal flags and the approver id are exposed.
    let (_, body) = http_get(&fixture.authority, "/api/runs", Some(&fixture.token));
    let list = json_body(&body);
    let run = list["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .find(|run| run["run_id"] == approved_id)
        .expect("approved run in list");
    assert_eq!(run["status"], "approved");
    assert_eq!(run["stale"], false);
    assert_eq!(run["stalled"], false);
    assert_eq!(run["approved_by_run_id"], approver_id);

    // Stream: approved is terminal, so the stream drains and ends (bounded wait).
    let path = format!("/api/runs/{approved_id}/stream");
    let full_path = with_token(&path, Some(&fixture.token));
    let mut stream = TcpStream::connect(&fixture.authority).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let req = format!(
        "GET {full_path} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n",
        fixture.authority
    );
    stream.write_all(req.as_bytes()).expect("write sse");

    let mut buf = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut temp = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut temp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&temp[..n]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => break,
        }
        if String::from_utf8_lossy(&buf).contains("event: end") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    assert!(
        text.contains("event: end"),
        "approved run stream must end: {text}"
    );
    assert!(
        text.contains("\"status\":\"approved\""),
        "stream run event must carry the approved status: {text}"
    );
}
