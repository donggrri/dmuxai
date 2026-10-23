//! McpClient implementation (slice C, INT-562).
//!
//! Connects to an MCP server over stdio or streamable HTTP, performs the
//! JSON-RPC 2.0 handshake (`initialize` with protocolVersion "2025-06-18",
//! followed by `notifications/initialized`), and exposes `list_tools` and
//! `call_tool`. Every error names the server.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

use super::config::{ResolvedMcpServer, Transport};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

static INIT_CRYPTO: Once = Once::new();

fn init_crypto() {
    INIT_CRYPTO.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// An error that occurred while communicating with an MCP server.
///
/// Always names the server in its display representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpError {
    server: String,
    message: String,
}

impl McpError {
    pub fn new(server: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            message: message.into(),
        }
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.server, self.message)
    }
}

impl std::error::Error for McpError {}

/// One tool advertised by an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

struct StdioClient {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
    #[cfg(unix)]
    pgid: u32,
}

impl StdioClient {
    fn kill(&mut self) {
        #[cfg(unix)]
        if self.pgid > 0 {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", self.pgid)])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.start_kill();
    }
}

impl Drop for StdioClient {
    fn drop(&mut self) {
        self.kill();
    }
}

fn get_stderr_tail(tail: &Arc<Mutex<VecDeque<u8>>>) -> String {
    let lock = tail.lock().unwrap();
    let bytes: Vec<u8> = lock.iter().copied().collect();
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

#[derive(Clone)]
struct HttpClient {
    client: reqwest::Client,
    url: String,
    headers: BTreeMap<String, String>,
    session_id: Arc<RwLock<Option<String>>>,
}

#[derive(Clone)]
enum ClientTransport {
    Stdio(Arc<tokio::sync::Mutex<StdioClient>>),
    Http(HttpClient),
}

/// Client for communicating with an MCP server over stdio or streamable HTTP.
#[derive(Clone)]
pub struct McpClient {
    server_name: String,
    transport: ClientTransport,
    timeout: Duration,
    next_id: Arc<AtomicU64>,
}

impl fmt::Debug for McpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpClient")
            .field("server_name", &self.server_name)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl McpClient {
    /// Connects to `server`, performing the `initialize` and
    /// `notifications/initialized` handshake with the given per-request timeout.
    pub async fn connect(server: &ResolvedMcpServer, timeout: Duration) -> Result<Self, McpError> {
        let transport = match &server.transport {
            Transport::Stdio { command, args, env } => {
                let mut cmd = tokio::process::Command::new(command);
                cmd.args(args);
                crate::process::apply_child_env_policy_tokio(&mut cmd);
                for (k, v) in env {
                    cmd.env(k, v);
                }
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);

                #[cfg(unix)]
                cmd.process_group(0);

                let mut child = cmd.spawn().map_err(|e| {
                    McpError::new(&server.name, format!("failed to spawn '{command}': {e}"))
                })?;

                #[cfg(unix)]
                let pgid = child.id().unwrap_or(0);

                let stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| McpError::new(&server.name, "stdin not piped"))?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| McpError::new(&server.name, "stdout not piped"))?;
                let mut stderr = child
                    .stderr
                    .take()
                    .ok_or_else(|| McpError::new(&server.name, "stderr not piped"))?;

                let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(4096)));
                let tail_clone = Arc::clone(&stderr_tail);
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = stderr.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        let mut tail = tail_clone.lock().unwrap();
                        for &b in &buf[..n] {
                            if tail.len() >= 4096 {
                                tail.pop_front();
                            }
                            tail.push_back(b);
                        }
                    }
                });

                let lines = BufReader::new(stdout).lines();
                ClientTransport::Stdio(Arc::new(tokio::sync::Mutex::new(StdioClient {
                    child,
                    stdin,
                    lines,
                    stderr_tail,
                    #[cfg(unix)]
                    pgid,
                })))
            }
            Transport::Http { url, headers } => {
                init_crypto();
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|e| {
                        McpError::new(&server.name, format!("failed to build HTTP client: {e}"))
                    })?;
                ClientTransport::Http(HttpClient {
                    client,
                    url: url.clone(),
                    headers: headers.clone(),
                    session_id: Arc::new(RwLock::new(None)),
                })
            }
        };

        let client = Self {
            server_name: server.name.clone(),
            transport,
            timeout,
            next_id: Arc::new(AtomicU64::new(1)),
        };

        // Handshake: initialize
        let init_params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "dmuxai",
                "version": env!("CARGO_PKG_VERSION")
            }
        });
        let _ = client.send_request("initialize", init_params).await?;

        // Handshake: notifications/initialized
        client
            .send_notification("notifications/initialized", json!({}))
            .await?;

        Ok(client)
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Lists tools advertised by the server (`tools/list`).
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let result = self.send_request("tools/list", json!({})).await?;
        let tools_val = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                McpError::new(
                    &self.server_name,
                    "tools/list response missing 'tools' array",
                )
            })?;
        let mut tools = Vec::with_capacity(tools_val.len());
        for item in tools_val {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::new(&self.server_name, "tool entry missing 'name'"))?
                .to_owned();
            let description = item
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned);
            tools.push(McpTool { name, description });
        }
        Ok(tools)
    }

    /// Calls a tool on the server (`tools/call`).
    ///
    /// If `isError` is true on the response, returns an `Err(McpError)`.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value, McpError> {
        let result = self
            .send_request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": args
                }),
            )
            .await?;

        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let message = extract_tool_error_message(&result)
                .unwrap_or_else(|| "tool reported error".to_owned());
            return Err(McpError::new(&self.server_name, message));
        }

        Ok(result)
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let req_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        match &self.transport {
            ClientTransport::Stdio(client_mutex) => {
                let req_future = async {
                    let mut client = client_mutex.lock().await;
                    let request_json = json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "method": method,
                        "params": params,
                    });
                    let mut line = serde_json::to_string(&request_json).map_err(|e| {
                        McpError::new(
                            &self.server_name,
                            format!("failed to serialize request: {e}"),
                        )
                    })?;
                    line.push('\n');

                    client.stdin.write_all(line.as_bytes()).await.map_err(|e| {
                        let stderr = get_stderr_tail(&client.stderr_tail);
                        let err = if stderr.is_empty() {
                            format!("failed to write to server stdin: {e}")
                        } else {
                            format!("failed to write to server stdin: {e}: {stderr}")
                        };
                        McpError::new(&self.server_name, err)
                    })?;
                    client.stdin.flush().await.map_err(|e| {
                        McpError::new(&self.server_name, format!("failed to flush stdin: {e}"))
                    })?;

                    loop {
                        match client.lines.next_line().await {
                            Ok(Some(line)) => {
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }
                                if let Ok(val) = serde_json::from_str::<Value>(trimmed)
                                    && val.get("id") == Some(&json!(req_id))
                                {
                                    return Ok(val);
                                }
                            }
                            Ok(None) => {
                                let stderr = get_stderr_tail(&client.stderr_tail);
                                let status_msg = match client.child.try_wait() {
                                    Ok(Some(status)) => format!(" (exited with {status})"),
                                    _ => String::new(),
                                };
                                let err = if stderr.is_empty() {
                                    format!("server process closed stdout unexpectedly{status_msg}")
                                } else {
                                    format!("server process closed stdout{status_msg}: {stderr}")
                                };
                                return Err(McpError::new(&self.server_name, err));
                            }
                            Err(e) => {
                                let stderr = get_stderr_tail(&client.stderr_tail);
                                let err = if stderr.is_empty() {
                                    format!("error reading stdout: {e}")
                                } else {
                                    format!("error reading stdout: {e}: {stderr}")
                                };
                                return Err(McpError::new(&self.server_name, err));
                            }
                        }
                    }
                };

                let response_json = match tokio::time::timeout(self.timeout, req_future).await {
                    Ok(res) => res?,
                    Err(_) => {
                        let mut client = client_mutex.lock().await;
                        client.kill();
                        return Err(McpError::new(
                            &self.server_name,
                            format!("request '{method}' timed out after {:?}", self.timeout),
                        ));
                    }
                };

                if let Some(err) = response_json.get("error") {
                    let msg = err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown RPC error");
                    return Err(McpError::new(
                        &self.server_name,
                        format!("RPC error: {msg}"),
                    ));
                }

                Ok(response_json.get("result").cloned().unwrap_or(Value::Null))
            }
            ClientTransport::Http(http) => {
                let send_future = async {
                    let mut req = http
                        .client
                        .post(&http.url)
                        .header(
                            reqwest::header::ACCEPT,
                            "application/json, text/event-stream",
                        )
                        .header(reqwest::header::CONTENT_TYPE, "application/json");

                    for (k, v) in &http.headers {
                        req = req.header(k, v);
                    }

                    if let Some(session) = http.session_id.read().unwrap().as_deref() {
                        req = req.header("mcp-session-id", session);
                    }

                    let body = json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "method": method,
                        "params": params,
                    });

                    let mut response = req.body(body.to_string()).send().await.map_err(|e| {
                        McpError::new(&self.server_name, format!("HTTP request error: {e}"))
                    })?;

                    if let Some(sess) = response
                        .headers()
                        .get("mcp-session-id")
                        .and_then(|h| h.to_str().ok())
                    {
                        *http.session_id.write().unwrap() = Some(sess.to_owned());
                    }

                    let status = response.status();
                    if !status.is_success() {
                        if status.is_redirection() {
                            return Err(McpError::new(
                                &self.server_name,
                                format!("HTTP redirect status {status} not followed"),
                            ));
                        }
                        let text = response.text().await.unwrap_or_default();
                        return Err(McpError::new(
                            &self.server_name,
                            format!("HTTP error status {status}: {text}"),
                        ));
                    }

                    let is_sse = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .map(|ct| ct.contains("text/event-stream"))
                        .unwrap_or(false);

                    let target_id = json!(req_id);
                    let resp_json = if is_sse {
                        let mut byte_buffer: Vec<u8> = Vec::new();
                        let mut data_lines: Vec<String> = Vec::new();
                        let mut matched_val = None;

                        while let Some(chunk) = response.chunk().await.map_err(|e| {
                            McpError::new(
                                &self.server_name,
                                format!("failed reading SSE chunk: {e}"),
                            )
                        })? {
                            byte_buffer.extend_from_slice(&chunk);

                            while let Some(idx) = byte_buffer.iter().position(|&b| b == b'\n') {
                                let mut line_bytes = &byte_buffer[..idx];
                                if let Some(stripped) = line_bytes.strip_suffix(b"\r") {
                                    line_bytes = stripped;
                                }
                                let line = std::str::from_utf8(line_bytes).map_err(|e| {
                                    McpError::new(
                                        &self.server_name,
                                        format!("invalid UTF-8 in SSE stream: {e}"),
                                    )
                                })?;

                                let trimmed = line.trim();
                                let is_empty = trimmed.is_empty();
                                let data_line = if !is_empty {
                                    line.strip_prefix("data:")
                                        .map(|d| d.trim_start().to_owned())
                                } else {
                                    None
                                };

                                byte_buffer.drain(..=idx);

                                if is_empty {
                                    if !data_lines.is_empty() {
                                        let joined = data_lines.join("\n");
                                        data_lines.clear();
                                        if let Ok(v) = serde_json::from_str::<Value>(&joined)
                                            && v.get("id") == Some(&target_id)
                                        {
                                            matched_val = Some(v);
                                            break;
                                        }
                                    }
                                } else if let Some(data) = data_line {
                                    data_lines.push(data);
                                }
                            }

                            if matched_val.is_some() {
                                break;
                            }
                        }

                        if matched_val.is_none() && !byte_buffer.is_empty() {
                            let mut line_bytes = byte_buffer.as_slice();
                            if let Some(stripped) = line_bytes.strip_suffix(b"\r") {
                                line_bytes = stripped;
                            }
                            if let Some(data) = std::str::from_utf8(line_bytes)
                                .ok()
                                .and_then(|line| line.strip_prefix("data:"))
                            {
                                data_lines.push(data.trim_start().to_owned());
                            }
                        }

                        if matched_val.is_none() && !data_lines.is_empty() {
                            let joined = data_lines.join("\n");
                            if let Ok(v) = serde_json::from_str::<Value>(&joined)
                                && v.get("id") == Some(&target_id)
                            {
                                matched_val = Some(v);
                            }
                        }

                        matched_val.ok_or_else(|| {
                            McpError::new(
                                &self.server_name,
                                format!("no SSE message with id {target_id} found in response"),
                            )
                        })?
                    } else {
                        let text = response.text().await.map_err(|e| {
                            McpError::new(
                                &self.server_name,
                                format!("failed to read HTTP body: {e}"),
                            )
                        })?;
                        serde_json::from_str::<Value>(&text).map_err(|e| {
                            McpError::new(
                                &self.server_name,
                                format!("invalid JSON in response: {e}"),
                            )
                        })?
                    };

                    if let Some(err) = resp_json.get("error") {
                        let msg = err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown RPC error");
                        return Err(McpError::new(
                            &self.server_name,
                            format!("RPC error: {msg}"),
                        ));
                    }

                    Ok(resp_json.get("result").cloned().unwrap_or(Value::Null))
                };

                match tokio::time::timeout(self.timeout, send_future).await {
                    Ok(res) => res,
                    Err(_) => Err(McpError::new(
                        &self.server_name,
                        format!("request '{method}' timed out after {:?}", self.timeout),
                    )),
                }
            }
        }
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<(), McpError> {
        match &self.transport {
            ClientTransport::Stdio(client_mutex) => {
                let notif_future = async {
                    let mut client = client_mutex.lock().await;
                    let notif_json = json!({
                        "jsonrpc": "2.0",
                        "method": method,
                        "params": params,
                    });
                    let mut line = serde_json::to_string(&notif_json).map_err(|e| {
                        McpError::new(
                            &self.server_name,
                            format!("failed to serialize notification: {e}"),
                        )
                    })?;
                    line.push('\n');

                    client.stdin.write_all(line.as_bytes()).await.map_err(|e| {
                        McpError::new(
                            &self.server_name,
                            format!("failed to write notification to stdin: {e}"),
                        )
                    })?;
                    client.stdin.flush().await.map_err(|e| {
                        McpError::new(
                            &self.server_name,
                            format!("failed to flush notification to stdin: {e}"),
                        )
                    })?;
                    Ok::<(), McpError>(())
                };

                match tokio::time::timeout(self.timeout, notif_future).await {
                    Ok(res) => res,
                    Err(_) => {
                        let mut client = client_mutex.lock().await;
                        client.kill();
                        Err(McpError::new(
                            &self.server_name,
                            format!("notification '{method}' timed out after {:?}", self.timeout),
                        ))
                    }
                }
            }
            ClientTransport::Http(http) => {
                let send_future = async {
                    let mut req = http
                        .client
                        .post(&http.url)
                        .header(
                            reqwest::header::ACCEPT,
                            "application/json, text/event-stream",
                        )
                        .header(reqwest::header::CONTENT_TYPE, "application/json");

                    for (k, v) in &http.headers {
                        req = req.header(k, v);
                    }

                    if let Some(session) = http.session_id.read().unwrap().as_deref() {
                        req = req.header("mcp-session-id", session);
                    }

                    let body = json!({
                        "jsonrpc": "2.0",
                        "method": method,
                        "params": params,
                    });

                    let response = req.body(body.to_string()).send().await.map_err(|e| {
                        McpError::new(&self.server_name, format!("HTTP notification error: {e}"))
                    })?;

                    let status = response.status();
                    if !status.is_success() {
                        if status.is_redirection() {
                            return Err(McpError::new(
                                &self.server_name,
                                format!("HTTP redirect status {status} not followed"),
                            ));
                        }
                        return Err(McpError::new(
                            &self.server_name,
                            format!("notification failed with status {status}"),
                        ));
                    }
                    Ok(())
                };

                match tokio::time::timeout(self.timeout, send_future).await {
                    Ok(res) => res,
                    Err(_) => Err(McpError::new(
                        &self.server_name,
                        format!("notification '{method}' timed out after {:?}", self.timeout),
                    )),
                }
            }
        }
    }
}

fn extract_tool_error_message(result: &Value) -> Option<String> {
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for part in content {
            if part.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = part.get("text").and_then(Value::as_str)
            {
                return Some(text.to_owned());
            }
        }
    }
    result
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
}
