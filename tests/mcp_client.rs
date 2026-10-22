//! TC-561 / INT-562, NFR-560: MCP client over stdio and streamable HTTP.

use std::collections::BTreeMap;
use std::time::Duration;

use dmuxai::mcp::client::McpClient;
use dmuxai::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
use serde_json::json;

#[tokio::test]
async fn mcp_client_stdio_and_http_against_mock() {
    // -------------------------------------------------------------------------
    // 1. Stdio transport against in-repo mock: dmuxai __mock-mcp
    // -------------------------------------------------------------------------
    let stdio_server = ResolvedMcpServer {
        name: "mock-stdio".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Stdio {
            command: env!("CARGO_BIN_EXE_dmuxai").to_owned(),
            args: vec!["__mock-mcp".to_owned()],
            env: BTreeMap::new(),
        },
        runners: None,
        tools: ToolNames::default(),
    };

    let client = McpClient::connect(&stdio_server, Duration::from_secs(5))
        .await
        .expect("connect to mock stdio mcp server");

    // tools/list
    let tools = client
        .list_tools()
        .await
        .expect("list tools from mock stdio");
    assert!(
        tools.iter().any(|t| t.name == "echo"),
        "tools list should contain 'echo'"
    );
    assert!(
        tools.iter().any(|t| t.name == "fail"),
        "tools list should contain 'fail'"
    );

    // tools/call: echo
    let echo_res = client
        .call_tool("echo", json!({"message": "stdio ping"}))
        .await
        .expect("call echo on stdio");
    assert_eq!(echo_res["structuredContent"]["message"], "stdio ping");

    // tools/call: fail returns error with server name
    let fail_err = client
        .call_tool("fail", json!({}))
        .await
        .expect_err("fail tool should return error");
    let err_str = fail_err.to_string();
    assert!(
        err_str.contains("mock-stdio"),
        "error message should contain server name 'mock-stdio': {err_str}"
    );

    // stdio crash error names the server
    let crash_server = ResolvedMcpServer {
        name: "crash-stdio".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Stdio {
            command: "nonexistent-cmd-dmuxai-test-12345".to_owned(),
            args: vec![],
            env: BTreeMap::new(),
        },
        runners: None,
        tools: ToolNames::default(),
    };
    let crash_err = McpClient::connect(&crash_server, Duration::from_secs(2))
        .await
        .expect_err("nonexistent command should fail to connect");
    let crash_str = crash_err.to_string();
    assert!(
        crash_str.contains("crash-stdio"),
        "crash error should contain server name 'crash-stdio': {crash_str}"
    );

    // stdio timeout error names the server
    let timeout_server = ResolvedMcpServer {
        name: "timeout-stdio".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Stdio {
            command: env!("CARGO_BIN_EXE_dmuxai").to_owned(),
            args: vec!["__mock-mcp".to_owned()],
            // The mock never answers, so the timeout fires on every platform
            // (a 1ns timeout raced the fast mock on Windows CI).
            env: BTreeMap::from([("DMUXAI_MOCK_MCP_SILENT".to_owned(), "1".to_owned())]),
        },
        runners: None,
        tools: ToolNames::default(),
    };
    let timeout_err = McpClient::connect(&timeout_server, Duration::from_millis(300))
        .await
        .expect_err("connect with 1ns timeout should fail");
    let timeout_str = timeout_err.to_string();
    assert!(
        timeout_str.contains("timeout-stdio"),
        "timeout error should contain server name 'timeout-stdio': {timeout_str}"
    );

    // -------------------------------------------------------------------------
    // 2. HTTP transport against in-repo mock on 127.0.0.1:0
    // -------------------------------------------------------------------------
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener for mock http");
    let addr = listener.local_addr().expect("listener local_addr");
    let mock_task = tokio::spawn(async move {
        let _ = dmuxai::mcp::mock::serve_http(listener).await;
    });

    let base_url = format!("http://{addr}/mcp");

    // Standard JSON mode
    let http_server = ResolvedMcpServer {
        name: "mock-http".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Http {
            url: base_url.clone(),
            headers: BTreeMap::new(),
        },
        runners: None,
        tools: ToolNames::default(),
    };

    let client = McpClient::connect(&http_server, Duration::from_secs(5))
        .await
        .expect("connect to mock http mcp server");

    let tools = client
        .list_tools()
        .await
        .expect("list tools from mock http");
    assert!(tools.iter().any(|t| t.name == "echo"));

    let echo_res = client
        .call_tool("echo", json!({"message": "http ping"}))
        .await
        .expect("call echo on http");
    assert_eq!(echo_res["structuredContent"]["message"], "http ping");

    let fail_err = client
        .call_tool("fail", json!({}))
        .await
        .expect_err("call fail on http should error");
    let err_str = fail_err.to_string();
    assert!(
        err_str.contains("mock-http"),
        "http error should contain server name 'mock-http': {err_str}"
    );

    // SSE mode via configured header
    let mut sse_headers = BTreeMap::new();
    sse_headers.insert("x-mock-sse".to_owned(), "1".to_owned());
    let sse_server = ResolvedMcpServer {
        name: "mock-sse".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Http {
            url: base_url,
            headers: sse_headers,
        },
        runners: None,
        tools: ToolNames::default(),
    };

    let client = McpClient::connect(&sse_server, Duration::from_secs(5))
        .await
        .expect("connect to mock sse mcp server");

    let tools = client.list_tools().await.expect("list tools from mock sse");
    assert!(tools.iter().any(|t| t.name == "echo"));

    let echo_res = client
        .call_tool("echo", json!({"message": "sse ping"}))
        .await
        .expect("call echo on sse");
    assert_eq!(echo_res["structuredContent"]["message"], "sse ping");

    // HTTP connection failure names the server
    let bad_http_server = ResolvedMcpServer {
        name: "bad-http".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Http {
            url: "http://127.0.0.1:1/mcp".to_owned(),
            headers: BTreeMap::new(),
        },
        runners: None,
        tools: ToolNames::default(),
    };
    let bad_err = McpClient::connect(&bad_http_server, Duration::from_secs(1))
        .await
        .expect_err("connection to closed port should error");
    let bad_str = bad_err.to_string();
    assert!(
        bad_str.contains("bad-http"),
        "connection error should contain server name 'bad-http': {bad_str}"
    );

    mock_task.abort();
}

#[tokio::test]
#[ignore]
async fn mcp_client_smoke_server_everything() {
    let npx_check = std::process::Command::new("npx").arg("--version").output();
    if npx_check.is_err() {
        println!("SKIP: npx is not installed or not in PATH");
        return;
    }

    let everything_server = ResolvedMcpServer {
        name: "everything".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Stdio {
            command: "npx".to_owned(),
            args: vec![
                "-y".to_owned(),
                "@modelcontextprotocol/server-everything".to_owned(),
            ],
            env: BTreeMap::new(),
        },
        runners: None,
        tools: ToolNames::default(),
    };

    let client = match McpClient::connect(&everything_server, Duration::from_secs(30)).await {
        Ok(client) => client,
        Err(err) => {
            println!("SKIP: could not connect to @modelcontextprotocol/server-everything: {err}");
            return;
        }
    };

    let tools = client.list_tools().await.expect("list tools");
    assert!(!tools.is_empty(), "expected tools from server-everything");

    let echo_tool = tools.iter().find(|t| t.name == "echo");
    if echo_tool.is_some() {
        let res = client
            .call_tool("echo", json!({"message": "hello everything"}))
            .await
            .expect("call echo tool");
        assert!(res.to_string().contains("hello everything"));
    }
}

#[tokio::test]
async fn mcp_client_refuses_redirects() {
    // TC-574, NFR-561: HTTP MCP requests never follow redirects (3xx is an error naming the server).
    // The redirect target receives no request.
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // 1. Target listener that records any incoming requests
    let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_addr = target_listener.local_addr().unwrap();
    let target_hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = target_hits.clone();

    let target_task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = target_listener.accept().await {
            hits_clone.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let _ = socket.write_all(resp.as_bytes()).await;
        }
    });

    // 2. Redirecting server that answers 302 to the target listener
    let redirect_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect_addr = redirect_listener.local_addr().unwrap();
    let redirect_url = format!("http://{target_addr}/redirected");

    let redirect_task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = redirect_listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 302 Found\r\nLocation: {redirect_url}\r\nContent-Length: 0\r\n\r\n"
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        }
    });

    let mut headers = BTreeMap::new();
    headers.insert("X-API-Key".to_owned(), "super-secret".to_owned());

    let server = ResolvedMcpServer {
        name: "redirect-server".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Http {
            url: format!("http://{redirect_addr}/mcp"),
            headers,
        },
        runners: None,
        tools: ToolNames::default(),
    };

    let err = McpClient::connect(&server, Duration::from_secs(2))
        .await
        .expect_err("connect to 302 redirecting server must fail");

    let err_msg = err.to_string();
    assert!(
        err_msg.contains("redirect-server"),
        "error must name the server: {err_msg}"
    );
    assert!(
        err_msg.to_lowercase().contains("redirect") || err_msg.contains("302"),
        "error must mention redirect or 302: {err_msg}"
    );

    // Target must receive no request
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        target_hits.load(Ordering::SeqCst),
        0,
        "redirect target received a request!"
    );

    target_task.abort();
    redirect_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn mcp_client_kills_stdio_process_group() {
    // TC-575, NFR-561: stdio server runs in its own process group (unix),
    // which is killed on drop, timeout or error, so launcher grandchildren (e.g. npx) do not survive.
    // Grandchild pid is gone within 2 s.
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn is_pid_alive(pid: i32) -> bool {
        // A killed grandchild is re-parented; until its new parent reaps it,
        // it lingers as a zombie that `kill -0` still reports as alive (seen
        // on GitHub's ubuntu runners). Treat zombies as dead.
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            let state = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next());
            return !matches!(state, Some("Z") | Some("X"));
        }
        if std::path::Path::new("/proc/self/stat").exists() {
            return false;
        }
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    // Part A: connect, then drop client
    let temp_a = tempfile::tempdir().unwrap();
    let pid_file_a = temp_a.path().join("grandchild_a.pid");
    let launcher_a = temp_a.path().join("launcher_a.sh");

    let script_a = format!(
        r#"#!/bin/sh
sleep 60 &
echo $! > "{pid_file}"
while IFS= read -r line; do
  case "$line" in
    *initialize*)
      echo '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}},"serverInfo":{{"name":"mock","version":"1"}}}}}}'
      ;;
    *notifications/initialized*)
      ;;
  esac
done
"#,
        pid_file = pid_file_a.display()
    );
    fs::write(&launcher_a, script_a).unwrap();
    let mut perms = fs::metadata(&launcher_a).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&launcher_a, perms).unwrap();

    let server_a = ResolvedMcpServer {
        name: "group-stdio-a".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Stdio {
            // Run through `sh` instead of exec'ing the freshly written script:
            // exec can fail with ETXTBSY while parallel tests fork.
            command: "/bin/sh".to_owned(),
            args: vec![launcher_a.to_str().unwrap().to_owned()],
            env: BTreeMap::new(),
        },
        runners: None,
        tools: ToolNames::default(),
    };

    let client_a = McpClient::connect(&server_a, Duration::from_secs(5))
        .await
        .expect("connect to launcher_a");

    let start = std::time::Instant::now();
    let mut pid_a: Option<i32> = None;
    while start.elapsed() < Duration::from_secs(2) {
        if let Some(p) = fs::read_to_string(&pid_file_a)
            .ok()
            .and_then(|c| c.trim().parse::<i32>().ok())
        {
            pid_a = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid_a = pid_a.expect("grandchild PID was not written");
    assert!(
        is_pid_alive(pid_a),
        "grandchild should be running before drop"
    );

    // Drop the client
    drop(client_a);

    // Assert grandchild PID is gone within 2 s
    let drop_start = std::time::Instant::now();
    let mut gone_a = false;
    while drop_start.elapsed() < Duration::from_secs(2) {
        if !is_pid_alive(pid_a) {
            gone_a = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        gone_a,
        "grandchild {pid_a} was not killed within 2s after drop; ps: {}",
        String::from_utf8_lossy(
            &std::process::Command::new("ps")
                .args([
                    "-o",
                    "pid,ppid,pgid,sid,stat,comm",
                    "-p",
                    &pid_a.to_string()
                ])
                .output()
                .map(|o| o.stdout)
                .unwrap_or_default()
        )
    );

    // Part B: separately hit a timeout
    let temp_b = tempfile::tempdir().unwrap();
    let pid_file_b = temp_b.path().join("grandchild_b.pid");
    let launcher_b = temp_b.path().join("launcher_b.sh");

    let script_b = format!(
        r#"#!/bin/sh
sleep 60 &
echo $! > "{pid_file}"
while IFS= read -r line; do
  case "$line" in
    *initialize*)
      echo '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}},"serverInfo":{{"name":"mock","version":"1"}}}}}}'
      ;;
    *notifications/initialized*)
      ;;
    *tools/call*)
      sleep 60
      ;;
  esac
done
"#,
        pid_file = pid_file_b.display()
    );
    fs::write(&launcher_b, script_b).unwrap();
    let mut perms = fs::metadata(&launcher_b).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&launcher_b, perms).unwrap();

    let server_b = ResolvedMcpServer {
        name: "group-stdio-b".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Stdio {
            // Run through `sh` instead of exec'ing the freshly written script:
            // exec can fail with ETXTBSY while parallel tests fork.
            command: "/bin/sh".to_owned(),
            args: vec![launcher_b.to_str().unwrap().to_owned()],
            env: BTreeMap::new(),
        },
        runners: None,
        tools: ToolNames::default(),
    };

    let client_b = McpClient::connect(&server_b, Duration::from_secs(1))
        .await
        .expect("connect to launcher_b");

    let start = std::time::Instant::now();
    let mut pid_b: Option<i32> = None;
    while start.elapsed() < Duration::from_secs(2) {
        if let Some(p) = fs::read_to_string(&pid_file_b)
            .ok()
            .and_then(|c| c.trim().parse::<i32>().ok())
        {
            pid_b = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid_b = pid_b.expect("grandchild PID was not written");
    assert!(is_pid_alive(pid_b), "grandchild should be running");

    let err = client_b
        .call_tool("block", json!({}))
        .await
        .expect_err("call_tool should time out");
    assert!(err.to_string().contains("group-stdio-b"));

    let to_start = std::time::Instant::now();
    let mut gone_b = false;
    while to_start.elapsed() < Duration::from_secs(2) {
        if !is_pid_alive(pid_b) {
            gone_b = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        gone_b,
        "grandchild {pid_b} was not killed within 2s after timeout"
    );
}

#[tokio::test]
async fn mcp_client_bounds_writes_and_streams_sse() {
    // TC-576, NFR-561: stdio server that never reads stdin; HTTP server whose SSE stream stays open
    // after the response event and splits it over two data: lines.
    // Call with a 1 s timeout: stdio -> timeout error naming the server within ~1 s;
    // SSE -> the result returns before the timeout and the JSON is reassembled.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // 1. Stdio: server that never reads stdin
    let (stdio_cmd, stdio_args) = ("sleep", vec!["60".to_owned()]);

    let stdio_server = ResolvedMcpServer {
        name: "unresponsive-stdio".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Stdio {
            command: stdio_cmd.to_owned(),
            args: stdio_args,
            env: BTreeMap::new(),
        },
        runners: None,
        tools: ToolNames::default(),
    };

    let start = std::time::Instant::now();
    let stdio_err = McpClient::connect(&stdio_server, Duration::from_millis(1000))
        .await
        .expect_err("connect to unresponsive server should time out");
    let elapsed = start.elapsed();
    let err_str = stdio_err.to_string();
    assert!(
        err_str.contains("unresponsive-stdio"),
        "error must name server: {err_str}"
    );
    assert!(
        elapsed < Duration::from_millis(2500),
        "stdio timeout must complete in ~1s, took {elapsed:?}"
    );

    // 2. HTTP server whose SSE stream stays open after response and splits over two data: lines
    // The mock answers one request per connection, so it says `Connection: close`;
    // otherwise reqwest may reuse a pooled socket the mock already dropped (CI flake).
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                let n = socket.read(&mut buffer).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buffer[..n]);

                if req_text.contains("initialize")
                    && !req_text.contains("notifications/initialized")
                {
                    let init_body = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"serverInfo\":{\"name\":\"mock\",\"version\":\"1\"}}}";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        init_body.len(),
                        init_body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                } else if req_text.contains("notifications/initialized") {
                    let resp = "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
                    let _ = socket.write_all(resp.as_bytes()).await;
                } else if req_text.contains("split_unicode") {
                    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n";
                    let _ = socket.write_all(headers.as_bytes()).await;

                    // SSE event whose JSON contains non-ASCII text ('Jira 한글 ✓')
                    // split mid-character across two writes/flushes, stream stays open
                    let frame = "data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\ndata: \"content\":[{\"type\":\"text\",\"text\":\"Jira 한글 ✓\"}]}}\n\n";
                    let frame_bytes = frame.as_bytes();
                    let split_idx = frame.find('한').unwrap() + 1;
                    let _ = socket.write_all(&frame_bytes[..split_idx]).await;
                    let _ = socket.flush().await;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = socket.write_all(&frame_bytes[split_idx..]).await;
                    let _ = socket.flush().await;

                    tokio::time::sleep(Duration::from_secs(30)).await;
                } else {
                    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n";
                    let _ = socket.write_all(headers.as_bytes()).await;

                    // Two data: lines split across chunks, stream stays open
                    let frame = "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\ndata: \"content\":[{\"type\":\"text\",\"text\":\"reassembled-ok\"}]}}\n\n";
                    let _ = socket.write_all(frame.as_bytes()).await;

                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            });
        }
    });

    let http_server = ResolvedMcpServer {
        name: "sse-stream-server".to_owned(),
        kind: ServerKind::Generic,
        transport: Transport::Http {
            url: format!("http://{addr}/mcp"),
            headers: BTreeMap::new(),
        },
        runners: None,
        tools: ToolNames::default(),
    };

    let client = McpClient::connect(&http_server, Duration::from_secs(2))
        .await
        .expect("connect to sse-stream-server");

    let start = std::time::Instant::now();
    let res = client
        .call_tool("any", json!({}))
        .await
        .expect("call_tool should return before timeout via streamed SSE");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(1500),
        "SSE should stream-parse immediately before timeout, took {elapsed:?}"
    );
    assert_eq!(
        res["content"][0]["text"], "reassembled-ok",
        "result JSON was not reassembled correctly: {res:?}"
    );

    // Mid-character split test with non-ASCII text across two writes/flushes
    let res_unicode = client
        .call_tool("split_unicode", json!({}))
        .await
        .expect("call_tool with non-ASCII split should succeed");
    assert_eq!(
        res_unicode["content"][0]["text"], "Jira 한글 ✓",
        "non-ASCII UTF-8 split mid-character was not decoded exactly: {res_unicode:?}"
    );

    server_task.abort();
}
