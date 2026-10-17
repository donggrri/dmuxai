//! Deterministic in-repo mock MCP server (slice C, NFR-560).
//!
//! Every fixture is fake and lives in this file: `example.invalid` URLs and
//! placeholder text only, so tests and CI never touch real Jira or Confluence.
//! Served over stdio (newline-delimited JSON) or streamable HTTP, per the
//! "Slice C contract" in docs/plans/PLAN-web-dashboard.md.

use std::io::{BufRead, Write};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Value, json};

/// Session id returned on initialize for the streamable HTTP mock.
const MOCK_SESSION_ID: &str = "mock-session-1";
const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "dmuxai-mock-mcp";

/// Search keyword that serves DEMO-1 with a changed status/assignee so a
/// re-import can be checked deterministically (TC-573). Fake and exact.
pub const JIRA_STATUS_KEYWORD: &str = "demo-sync";

/// Handle one JSON-RPC 2.0 message. `None` means "no response" (notifications).
pub fn handle_request(req: &Value) -> Option<Value> {
    let Some(id) = req.get("id").cloned() else {
        // Notifications never get a response; notifications/initialized included.
        return None;
    };
    let Some(method) = req.get("method").and_then(Value::as_str) else {
        return Some(error_response(id, -32600, "Invalid Request"));
    };
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "initialize" => Some(ok_response(id, initialize_result())),
        "ping" => Some(ok_response(id, json!({}))),
        "tools/list" => Some(ok_response(id, tools_list_result())),
        "tools/call" => Some(tools_call(id, &params)),
        _ => Some(error_response(id, -32601, "Method not found")),
    }
}

/// Serve the mock over stdio: newline-delimited JSON in, one response per
/// line out, flushed after every response.
pub async fn serve_stdio() {
    let _ = tokio::task::spawn_blocking(stdio_loop).await;
}

/// Env var that makes the stdio mock read requests but never answer, so
/// timeout handling can be tested deterministically on every platform.
pub const MOCK_SILENT_ENV: &str = "DMUXAI_MOCK_MCP_SILENT";

fn stdio_loop() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let silent = std::env::var_os(MOCK_SILENT_ENV).is_some();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if silent {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(trimmed) {
            Ok(request) => handle_request(&request),
            Err(_) => Some(error_response(Value::Null, -32700, "Parse error")),
        };
        let Some(response) = response else { continue };
        if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
}

/// Serve the mock over streamable HTTP on an already-bound listener.
pub async fn serve_http(listener: tokio::net::TcpListener) -> std::io::Result<()> {
    axum::serve(listener, http_router()).await
}

fn http_router() -> Router {
    Router::new().route("/mcp", post(mcp_post))
}

async fn mcp_post(headers: HeaderMap, body: String) -> Response {
    let request = match serde_json::from_str::<Value>(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let is_initialize = request.get("method").and_then(Value::as_str) == Some("initialize");
    if !is_initialize && !headers.contains_key("mcp-session-id") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if request.get("id").is_none() {
        // Notifications (notifications/initialized) get an empty 202.
        return StatusCode::ACCEPTED.into_response();
    }
    let Some(response) = handle_request(&request) else {
        return StatusCode::ACCEPTED.into_response();
    };
    let sse = headers.get("x-mock-sse").and_then(|v| v.to_str().ok()) == Some("1");
    let payload = serde_json::to_string(&response).unwrap_or_default();
    let (content_type, payload) = if sse {
        (
            "text/event-stream",
            format!("event: message\ndata: {payload}\n\n"),
        )
    } else {
        ("application/json", payload)
    };
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type);
    if is_initialize {
        builder = builder.header("mcp-session-id", MOCK_SESSION_ID);
    }
    builder
        .body(Body::from(payload))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
    })
}

fn tools_list_result() -> Value {
    json!({"tools": [
        {
            "name": "echo",
            "description": "Echo the call arguments back as text",
            "inputSchema": {"type": "object", "properties": {"value": {"type": "string"}}}
        },
        {
            "name": "jira_search",
            "description": "Search fake Jira issues (case-insensitive substring on id/title)",
            "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}
        },
        {
            "name": "jira_get",
            "description": "Get one fake Jira issue by key",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}
        },
        {
            "name": "confluence_search",
            "description": "Search fake Confluence pages (case-insensitive substring on id/title)",
            "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}
        },
        {
            "name": "confluence_get",
            "description": "Get one fake Confluence page by id",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}
        },
        {
            "name": "fail",
            "description": "Always returns a tool error",
            "inputSchema": {"type": "object"}
        }
    ]})
}

fn jira_items() -> Vec<Value> {
    vec![
        json!({
            "id": "DEMO-1",
            "title": "Fix login timeout",
            "url": "https://example.invalid/jira/DEMO-1",
            "status": "In Progress",
            "assignee": "Demo User",
            "body": "Fake fixture: sessions expire after a minute; placeholder text only, never real Jira data."
        }),
        json!({
            "id": "DEMO-2",
            "title": "Add CSV export",
            "url": "https://example.invalid/jira/DEMO-2",
            "status": "To Do",
            "assignee": null,
            "body": "Fake fixture: export the monthly report as CSV; placeholder text only, never real Jira data."
        }),
    ]
}

/// Jira search fixtures plus the cleaned needle. When `query` contains
/// [`JIRA_STATUS_KEYWORD`], DEMO-1 is served with its "changed" status/assignee
/// (a deterministic stand-in for a server-side update) and the keyword is
/// stripped before matching.
fn jira_search_fixtures(query: &str) -> (Vec<Value>, String) {
    let lower = query.to_lowercase();
    if lower.contains(JIRA_STATUS_KEYWORD) {
        let cleaned = lower.replace(JIRA_STATUS_KEYWORD, " ");
        let mut items = jira_items();
        items[0]["status"] = json!("Done");
        items[0]["assignee"] = json!("Demo Reviewer");
        (items, cleaned.trim().to_owned())
    } else {
        (jira_items(), query.to_owned())
    }
}

fn confluence_items() -> Vec<Value> {
    vec![json!({
        "id": "1001",
        "title": "Release checklist",
        "url": "https://example.invalid/confluence/1001",
        "body": "Fake fixture: tag the release, run the gates; placeholder text only, never real Confluence data."
    })]
}

fn tools_call(id: Value, params: &Value) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match name {
        "echo" => echo_result(&arguments),
        "jira_search" | "search" => {
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (items, needle) = jira_search_fixtures(query);
            search_result(&items, &needle)
        }
        "jira_get" | "get" => get_result(
            &jira_items(),
            arguments
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "confluence_search" => search_result(
            &confluence_items(),
            arguments
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "confluence_get" => get_result(
            &confluence_items(),
            arguments
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "fail" => tool_error("mock failure"),
        _ => tool_error(&format!("unknown tool: {name}")),
    };
    ok_response(id, result)
}

fn search_result(items: &[Value], query: &str) -> Value {
    let needle = query.to_lowercase();
    let matches: Vec<Value> = items
        .iter()
        .filter(|item| needle.is_empty() || fixture_matches(item, &needle))
        .cloned()
        .collect();
    tool_items_result(json!(matches))
}

fn fixture_matches(item: &Value, needle: &str) -> bool {
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let title = item
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    id.contains(needle) || title.contains(needle)
}

fn get_result(items: &[Value], id: &str) -> Value {
    match items
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
    {
        Some(item) => tool_items_result(json!([item])),
        None => tool_error(&format!("unknown id: {id}")),
    }
}

fn echo_result(arguments: &Value) -> Value {
    let text = arguments.to_string();
    json!({"content": [{"type": "text", "text": text}], "structuredContent": arguments})
}

fn tool_items_result(items: Value) -> Value {
    let text = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_owned());
    json!({"content": [{"type": "text", "text": text}], "structuredContent": {"items": items}})
}

fn tool_error(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn mock_mcp_handles_tools_list_and_calls() {
        // NFR-560 / TC-561: initialize uses the fixed protocol version and name.
        let init = handle_request(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}}
        }))
        .expect("initialize must respond");
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["serverInfo"]["name"], "dmuxai-mock-mcp");
        assert!(init["result"]["capabilities"]["tools"].is_object());

        // notifications/initialized never produces a response.
        assert!(
            handle_request(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .is_none()
        );

        // ping echoes the id.
        let pong = handle_request(&json!({"jsonrpc": "2.0", "id": "p", "method": "ping"})).unwrap();
        assert_eq!(pong["id"], "p");
        assert!(pong["result"].is_object());

        // tools/list advertises every mock tool, each with an inputSchema.
        let listed =
            handle_request(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        let tools = listed["result"]["tools"].as_array().expect("tools array");
        let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "confluence_get",
                "confluence_search",
                "echo",
                "fail",
                "jira_get",
                "jira_search"
            ]
        );
        assert!(tools.iter().all(|tool| tool["inputSchema"].is_object()));

        let call = |name: &str, arguments: Value| {
            handle_request(&json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": name, "arguments": arguments}
            }))
            .expect("tools/call must respond")
        };

        // echo round-trips the arguments.
        let echoed = call("echo", json!({"value": "hi"}));
        assert_eq!(
            echoed["result"]["structuredContent"],
            json!({"value": "hi"})
        );
        assert_eq!(
            echoed["result"]["content"][0]["text"],
            serde_json::to_string(&json!({"value": "hi"})).unwrap()
        );

        // Search filters case-insensitively; an empty query returns all fixtures.
        let all = call("jira_search", json!({"query": ""}));
        let items = &all["result"]["structuredContent"]["items"];
        assert_eq!(items.as_array().unwrap().len(), 2);
        let parsed_text: Value =
            serde_json::from_str(all["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(&parsed_text, items);
        let hits = call("jira_search", json!({"query": "LOGIN"}));
        let hits = &hits["result"]["structuredContent"]["items"];
        assert_eq!(hits.as_array().unwrap().len(), 1);
        assert_eq!(hits[0]["id"], "DEMO-1");
        let csv = call("jira_search", json!({"query": "csv"}));
        assert_eq!(
            csv["result"]["structuredContent"]["items"][0]["id"],
            "DEMO-2"
        );

        // get by id; unknown id is a tool error, not a JSON-RPC error.
        let got = call("jira_get", json!({"id": "DEMO-1"}));
        assert_eq!(
            got["result"]["structuredContent"]["items"][0]["title"],
            "Fix login timeout"
        );
        // FR-563: fixtures carry status/assignee (TC-573).
        assert_eq!(
            got["result"]["structuredContent"]["items"][0]["status"],
            "In Progress"
        );
        assert_eq!(
            got["result"]["structuredContent"]["items"][0]["assignee"],
            "Demo User"
        );
        let demo2 = call("jira_search", json!({"query": "csv"}));
        assert_eq!(
            demo2["result"]["structuredContent"]["items"][0]["assignee"],
            Value::Null
        );
        // TC-573: the status keyword serves DEMO-1 as Done and still matches
        // the rest of the query.
        let synced = call("jira_search", json!({"query": "DEMO demo-sync"}));
        let synced_items = &synced["result"]["structuredContent"]["items"];
        assert_eq!(synced_items.as_array().unwrap().len(), 2);
        assert_eq!(synced_items[0]["id"], "DEMO-1");
        assert_eq!(synced_items[0]["status"], "Done");
        assert_eq!(synced_items[0]["assignee"], "Demo Reviewer");
        assert!(
            got["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("example.invalid")
        );
        let missing = call("jira_get", json!({"id": "NOPE-9"}));
        assert_eq!(missing["result"]["isError"], true);
        assert!(missing["result"].get("structuredContent").is_none());

        // Confluence fixtures.
        let pages = call("confluence_search", json!({"query": "release"}));
        assert_eq!(
            pages["result"]["structuredContent"]["items"][0]["id"],
            "1001"
        );
        let page = call("confluence_get", json!({"id": "1001"}));
        let page_item = &page["result"]["structuredContent"]["items"][0];
        assert_eq!(page_item["title"], "Release checklist");
        assert!(
            page_item["url"]
                .as_str()
                .unwrap()
                .starts_with("https://example.invalid/")
        );
        assert_eq!(
            call("confluence_get", json!({"id": "404"}))["result"]["isError"],
            true
        );

        // fail always reports a tool error with the fixed text.
        let failed = call("fail", json!({}));
        assert_eq!(failed["result"]["isError"], true);
        assert_eq!(failed["result"]["content"][0]["text"], "mock failure");

        // Unknown tool: tool-level error.
        assert_eq!(call("nope", json!({}))["result"]["isError"], true);

        // Unknown method: JSON-RPC error -32601 with the id preserved.
        let bogus =
            handle_request(&json!({"jsonrpc": "2.0", "id": 9, "method": "no/such"})).unwrap();
        assert_eq!(bogus["error"]["code"], -32601);
        assert_eq!(bogus["id"], 9);
    }

    #[tokio::test]
    async fn mock_mcp_http_json_and_sse() {
        // TC-561: streamable HTTP on 127.0.0.1:0, raw HTTP/1.1 over TcpStream.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_http(listener));

        let initialize = post_mcp(
            addr,
            &[],
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}).to_string(),
        )
        .await;
        assert!(initialize.starts_with("HTTP/1.1 200"));
        assert!(
            initialize
                .to_ascii_lowercase()
                .contains("mcp-session-id: mock-session-1")
        );
        assert!(
            initialize
                .to_ascii_lowercase()
                .contains("content-type: application/json")
        );
        assert!(initialize.contains("2025-06-18"));

        let session = [("Mcp-Session-Id", "mock-session-1")];

        let notified = post_mcp(
            addr,
            &session,
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        )
        .await;
        assert!(notified.starts_with("HTTP/1.1 202"));

        let no_session = post_mcp(
            addr,
            &[],
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}).to_string(),
        )
        .await;
        assert!(no_session.starts_with("HTTP/1.1 400"));

        let sse_headers = [("Mcp-Session-Id", "mock-session-1"), ("X-Mock-Sse", "1")];
        let sse = post_mcp(
            addr,
            &sse_headers,
            &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}).to_string(),
        )
        .await;
        assert!(sse.starts_with("HTTP/1.1 200"));
        assert!(
            sse.to_ascii_lowercase()
                .contains("content-type: text/event-stream")
        );
        assert!(sse.contains("event: message\ndata: "));
        assert!(sse.contains("jira_search"));

        server.abort();
    }

    async fn post_mcp(addr: std::net::SocketAddr, headers: &[(&str, &str)], body: &str) -> String {
        let mut request = format!(
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect to mock http server");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write request");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.expect("read response");
        String::from_utf8(raw).expect("response is utf-8")
    }
}
