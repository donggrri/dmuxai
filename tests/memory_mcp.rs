//! TC-729 (INT-729) / TC-730 (FR-730): `dmuxai mcp-memory` stdio MCP server.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dmuxai::mcp::client::McpClient;
use dmuxai::mcp::resolve_servers;
use serde_json::{Value, json};

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

fn encode(value: Value) -> String {
    serde_json::to_string(&value).unwrap()
}

/// A request line gets a response unless it is a notification: no `id`, or
/// `"id": null` with a `notifications/*` method (mirrors `src/memory_mcp.rs`).
fn expects_response(request: &str) -> bool {
    match serde_json::from_str::<Value>(request) {
        Ok(Value::Object(object)) => match object.get("id") {
            None => false,
            Some(Value::Null) => !object
                .get("method")
                .and_then(Value::as_str)
                .is_some_and(|method| method.starts_with("notifications/")),
            Some(_) => true,
        },
        _ => true,
    }
}

fn read_line(receiver: &mpsc::Receiver<String>) -> String {
    match receiver.recv_timeout(Duration::from_secs(20)) {
        Ok(line) => line,
        Err(error) => panic!("timed out waiting for an MCP response: {error}"),
    }
}

/// Spawn `command`, send `requests` (one per line), then close stdin and read
/// the responses, returning them in order. Fails on timeout or extra responses.
fn exchange(command: Command, requests: &[String]) -> Vec<Value> {
    exchange_with_stderr(command, requests).0
}

/// Like [`exchange`], but also returns everything the server wrote to stderr.
fn exchange_with_stderr(mut command: Command, requests: &[String]) -> (Vec<Value>, String) {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().expect("spawn dmuxai mcp-memory");
    let mut guard = ChildGuard(Some(child));
    let mut stdin = guard.0.as_mut().unwrap().stdin.take().expect("stdin");
    let stdout = guard.0.as_mut().unwrap().stdout.take().expect("stdout");
    let mut stderr = guard.0.as_mut().unwrap().stderr.take().expect("stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
        text
    });

    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Write from a separate thread so a large request (TC-729 oversized line)
    // can never deadlock against unread stdout on small pipe buffers.
    let expected = requests
        .iter()
        .filter(|request| expects_response(request))
        .count();
    let owned: Vec<String> = requests.to_vec();
    let writer = std::thread::spawn(move || {
        for request in &owned {
            writeln!(stdin, "{request}").expect("write request");
        }
        stdin.flush().expect("flush requests");
        stdin
    });

    let mut responses = Vec::with_capacity(expected);
    for _ in 0..expected {
        let line = read_line(&receiver);
        responses.push(
            serde_json::from_str(&line)
                .unwrap_or_else(|error| panic!("invalid JSON response {line:?}: {error}")),
        );
    }
    assert!(
        receiver.recv_timeout(Duration::from_millis(1000)).is_err(),
        "server sent an unexpected extra response"
    );

    drop(writer.join().expect("stdin writer"));
    let child = guard.0.as_mut().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("server did not exit after stdin closed");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert!(status.success(), "server exited with {status}");
    reader.join().unwrap();
    (responses, stderr_reader.join().unwrap())
}

fn tool_response(response: &Value) -> (&str, bool) {
    (
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or(""),
        response["result"]["isError"].as_bool().unwrap_or(false),
    )
}

// TC-729 (INT-729): spawn the real binary against a temporary vault and drive
// initialize, tools/list, the three tools, an unknown method and a malformed
// line over piped stdio.
#[test]
fn mcp_memory_serves_tools_against_a_vault() {
    let vault = tempfile::tempdir().expect("vault");
    let root = vault.path().join("cursor-workflow").join("memory");
    std::fs::create_dir_all(root.join("gotchas")).expect("mkdir gotchas");
    std::fs::write(
        root.join("human.md"),
        "- [2026-10-17] prefer rg over grep\n",
    )
    .unwrap();
    std::fs::write(
        root.join("gotchas").join("windows.md"),
        "- [2026-10-17] windows path quirk\n",
    )
    .unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    command
        .arg("mcp-memory")
        .arg("--vault")
        .arg(vault.path())
        .arg("--project")
        .arg("demo");

    let requests = vec![
        encode(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        encode(json!({"jsonrpc": "2.0", "id": "init-1", "method": "initialize", "params": {}})),
        encode(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"})),
        encode(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})),
        encode(json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "memory_propose", "arguments": {"text": "always run fmt"}}
        })),
        encode(json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": {"name": "memory_get_context", "arguments": {}}
        })),
        encode(json!({
            "jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": {"name": "memory_search", "arguments": {"query": "windows"}}
        })),
        encode(json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": "memory_search", "arguments": {"query": "windows", "limit": 1}}
        })),
        encode(json!({
            "jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": {"name": "memory_search", "arguments": {"query": "windows", "limit": 0}}
        })),
        encode(json!({"jsonrpc": "2.0", "id": 9, "method": "nope/nope"})),
        "{ this is not json".to_owned(),
    ];

    let responses = exchange(command, &requests);
    assert_eq!(responses.len(), 10);

    // initialize (string id is echoed)
    assert_eq!(responses[0]["id"], "init-1");
    assert_eq!(responses[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(
        responses[0]["result"]["serverInfo"]["name"],
        "dmuxai-memory"
    );
    assert_eq!(
        responses[0]["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(responses[0]["result"]["capabilities"]["tools"].is_object());

    // ping
    assert_eq!(responses[1]["id"], 2);
    assert_eq!(responses[1]["result"], json!({}));

    // tools/list
    let names: Vec<&str> = responses[2]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["memory_get_context", "memory_search", "memory_propose"]
    );

    // tools/call memory_propose
    let (queued, queued_error) = tool_response(&responses[3]);
    assert!(!queued_error, "propose failed: {queued}");
    assert!(
        queued.starts_with("queued "),
        "unexpected propose text: {queued}"
    );
    assert!(
        queued.contains("for approval"),
        "unexpected propose text: {queued}"
    );
    assert!(
        queued.contains("/memory approve "),
        "unexpected propose text: {queued}"
    );

    // tools/call memory_get_context
    let (context, context_error) = tool_response(&responses[4]);
    assert!(!context_error, "get_context failed: {context}");
    assert!(
        context.contains("prefer rg over grep"),
        "unexpected context: {context}"
    );

    // tools/call memory_search (default limit)
    let (hits, hits_error) = tool_response(&responses[5]);
    assert!(!hits_error, "search failed: {hits}");
    assert_eq!(
        hits,
        "gotchas/windows.md: - [2026-10-17] windows path quirk"
    );

    // tools/call memory_search with an explicit limit
    let (limited, limited_error) = tool_response(&responses[6]);
    assert!(!limited_error, "search failed: {limited}");
    assert_eq!(limited.lines().count(), 1);

    // tools/call memory_search with an invalid limit
    let (invalid, invalid_error) = tool_response(&responses[7]);
    assert!(invalid_error);
    assert_eq!(invalid, "invalid limit (integer 1-100)");

    // unknown method and malformed line
    assert_eq!(responses[8]["id"], 9);
    assert_eq!(responses[8]["error"]["code"], -32601);
    assert!(responses[9]["id"].is_null());
    assert_eq!(responses[9]["error"]["code"], -32700);

    // The proposal was persisted to the queue.
    let queue = vault
        .path()
        .join("cursor-workflow")
        .join("memory")
        .join("queue")
        .join("proposals.json");
    assert!(queue.is_file(), "proposals.json was not written");
}

// TC-729 (INT-729): with no workspace config and an empty home, memory is
// disabled and every tools/call reports an error.
#[test]
fn mcp_memory_reports_disabled_memory() {
    let empty_home = tempfile::tempdir().expect("empty home");
    let workspace = tempfile::tempdir().expect("workspace");

    let mut command = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    command
        .arg("mcp-memory")
        .arg("--workspace")
        .arg(workspace.path())
        .env("HOME", empty_home.path())
        .env("XDG_CONFIG_HOME", empty_home.path())
        .env("USERPROFILE", empty_home.path());

    let requests = vec![
        encode(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})),
        encode(json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "memory_get_context", "arguments": {}}
        })),
    ];

    let responses = exchange(command, &requests);
    assert_eq!(responses.len(), 2);
    assert_eq!(
        responses[0]["result"]["serverInfo"]["name"],
        "dmuxai-memory"
    );
    let (text, is_error) = tool_response(&responses[1]);
    assert!(is_error, "expected isError for disabled memory");
    assert_eq!(
        text,
        "memory is not configured: set [memory] vault in dmuxai.toml"
    );
}

// TC-729 (INT-729): a `--vault` that is not an existing directory exits 2 with
// an error on stderr and no protocol output on stdout.
#[test]
fn mcp_memory_rejects_missing_vault() {
    let temp = tempfile::tempdir().expect("temp");
    let missing = temp.path().join("does-not-exist");

    let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .arg("mcp-memory")
        .arg("--vault")
        .arg(&missing)
        .stdin(Stdio::null())
        .output()
        .expect("run dmuxai mcp-memory");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "stdout must stay empty");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not an existing directory"),
        "stderr: {stderr}"
    );
}

// TC-729 (INT-729 / NFR-731): a `--dir` that escapes the vault exits 2 with an
// error on stderr and no protocol output on stdout.
#[test]
fn mcp_memory_rejects_escape_dir() {
    let vault = tempfile::tempdir().expect("vault");

    let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .arg("mcp-memory")
        .arg("--vault")
        .arg(vault.path())
        .arg("--dir")
        .arg("../escape")
        .stdin(Stdio::null())
        .output()
        .expect("run dmuxai mcp-memory");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "stdout must stay empty");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must be a relative path"),
        "stderr: {stderr}"
    );
}

// TC-729 (INT-729 / FR-720): an invalid `--project` exits 2 whether the vault
// comes from `--vault` or from settings.
#[test]
fn mcp_memory_rejects_invalid_project() {
    let vault = tempfile::tempdir().expect("vault");
    let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .arg("mcp-memory")
        .arg("--vault")
        .arg(vault.path())
        .arg("--project")
        .arg("Bad Name!")
        .stdin(Stdio::null())
        .output()
        .expect("run dmuxai mcp-memory");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "stdout must stay empty");
    assert!(String::from_utf8_lossy(&output.stderr).contains("--project"));

    let workspace = tempfile::tempdir().expect("workspace");
    let empty_home = tempfile::tempdir().expect("home");
    let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .arg("mcp-memory")
        .arg("--workspace")
        .arg(workspace.path())
        .arg("--project")
        .arg("Bad Name!")
        .env("HOME", empty_home.path())
        .env("XDG_CONFIG_HOME", empty_home.path())
        .env("USERPROFILE", empty_home.path())
        .stdin(Stdio::null())
        .output()
        .expect("run dmuxai mcp-memory");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "stdout must stay empty");
    assert!(String::from_utf8_lossy(&output.stderr).contains("--project"));
}

// TC-729 (INT-729 / FR-720): `--project` overrides `[memory]` from settings,
// so a target-less proposal lands in `projects/<override>.md`.
#[test]
fn mcp_memory_applies_project_override_to_settings() {
    let vault = tempfile::tempdir().expect("vault");
    let workspace = tempfile::tempdir().expect("workspace");
    let empty_home = tempfile::tempdir().expect("home");
    let vault_path = vault.path().display().to_string().replace('\\', "/");
    std::fs::write(
        workspace.path().join("dmuxai.toml"),
        format!("[memory]\nvault = \"{vault_path}\"\n"),
    )
    .expect("write dmuxai.toml");

    let mut command = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    command
        .arg("mcp-memory")
        .arg("--workspace")
        .arg(workspace.path())
        .arg("--project")
        .arg("override")
        .env("HOME", empty_home.path())
        .env("XDG_CONFIG_HOME", empty_home.path())
        .env("USERPROFILE", empty_home.path());

    let requests = vec![encode(json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "memory_propose", "arguments": {"text": "project override works"}}
    }))];
    let responses = exchange(command, &requests);
    let (queued, queued_error) = tool_response(&responses[0]);
    assert!(!queued_error, "propose failed: {queued}");

    let queue = vault
        .path()
        .join("cursor-workflow")
        .join("memory")
        .join("queue")
        .join("proposals.json");
    let raw = std::fs::read_to_string(&queue).expect("proposals.json");
    let parsed: Value = serde_json::from_str(&raw).expect("valid proposals.json");
    assert_eq!(parsed[0]["target"], "projects/override.md");
}

// TC-729 (INT-729, N1): `--project` with memory disabled warns on stderr and
// keeps stdout protocol-only.
#[test]
fn mcp_memory_warns_when_project_given_without_memory() {
    let empty_home = tempfile::tempdir().expect("empty home");
    let workspace = tempfile::tempdir().expect("workspace");

    let mut command = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    command
        .arg("mcp-memory")
        .arg("--workspace")
        .arg(workspace.path())
        .arg("--project")
        .arg("demo")
        .env("HOME", empty_home.path())
        .env("XDG_CONFIG_HOME", empty_home.path())
        .env("USERPROFILE", empty_home.path());

    let requests = vec![encode(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))];
    let (responses, stderr) = exchange_with_stderr(command, &requests);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["result"], json!({}));
    assert!(
        stderr.contains("warning: --project ignored: memory is not configured"),
        "missing warning on stderr: {stderr:?}"
    );
}

// TC-729 (INT-729, N2): a null-id `notifications/*` message gets no response
// from the real binary, while a null-id request is answered with a null id.
#[test]
fn mcp_memory_null_id_notification_is_silent() {
    let vault = tempfile::tempdir().expect("vault");
    let mut command = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    command.arg("mcp-memory").arg("--vault").arg(vault.path());

    let requests = vec![
        encode(json!({"jsonrpc": "2.0", "id": null, "method": "notifications/initialized"})),
        encode(json!({"jsonrpc": "2.0", "id": null, "method": "ping"})),
        encode(json!({"jsonrpc": "2.0", "id": 7, "method": "ping"})),
    ];
    assert!(!expects_response(&requests[0]));
    assert!(expects_response(&requests[1]));
    let responses = exchange(command, &requests);
    assert_eq!(responses.len(), 2);
    assert!(responses[0]["id"].is_null());
    assert_eq!(responses[0]["result"], json!({}));
    assert_eq!(responses[1]["id"], 7);
}

// TC-729 (INT-729, N3): a line over the 4 MiB cap sent to the real binary is
// answered with one -32700 parse error (null id) and serving continues.
#[test]
fn mcp_memory_oversized_line_is_parse_error_and_serving_continues() {
    let vault = tempfile::tempdir().expect("vault");
    let mut command = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
    command.arg("mcp-memory").arg("--vault").arg(vault.path());

    let oversized = "x".repeat(4 * 1024 * 1024 + 64);
    let requests = vec![
        oversized,
        encode(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"})),
    ];
    let responses = exchange(command, &requests);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["error"]["code"], -32700);
    assert!(responses[0]["id"].is_null());
    assert_eq!(responses[1]["id"], 2);
    assert_eq!(responses[1]["result"], json!({}));
}

// TC-735 (FR-733, INT-729): the built-in shared entry, built with the real
// binary as the executable, connects through McpClient and serves the tools.
#[tokio::test]
async fn builtin_memory_server_connects_via_shared_list() {
    let ws = tempfile::tempdir().unwrap();
    let vault = tempfile::tempdir().unwrap();
    let v = vault.path().display().to_string().replace('\\', "/");
    std::fs::write(
        ws.path().join("dmuxai.toml"),
        format!("[memory]\nvault = \"{v}\"\n"),
    )
    .expect("write dmuxai.toml");

    let exe = Path::new(env!("CARGO_BIN_EXE_dmuxai"));
    let mut spec = dmuxai::settings::builtin_memory_mcp_server(Some(exe), ws.path())
        .expect("built-in entry for the built binary");
    // Isolate the child's user config (HOME, XDG_CONFIG_HOME, USERPROFILE) so a
    // developer's ~/.config/dmuxai/dmuxai.toml cannot change the result.
    let empty_home = tempfile::tempdir().unwrap();
    let home =
        dmuxai::mcp::config::SecretValue::Literal(empty_home.path().to_string_lossy().into_owned());
    for key in ["HOME", "XDG_CONFIG_HOME", "USERPROFILE"] {
        spec.env.insert(key.to_owned(), home.clone());
    }
    let (resolved, warnings) = resolve_servers(&[spec], |_| None);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(resolved.len(), 1);

    let client = McpClient::connect(&resolved[0], Duration::from_secs(20))
        .await
        .expect("connect to built-in dmuxai-memory");
    let tools = client.list_tools().await.expect("tools/list");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    for expected in ["memory_get_context", "memory_search", "memory_propose"] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    let result = client
        .call_tool("memory_get_context", json!({}))
        .await
        .expect("memory_get_context call");
    assert_ne!(result.get("isError"), Some(&Value::Bool(true)), "{result}");
}
