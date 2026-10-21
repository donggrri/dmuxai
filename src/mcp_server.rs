//! `dmuxai mcp`: DmuxAI as a stdio MCP server (SPEC-agent-api FR-767).
//!
//! Transport: newline-delimited JSON-RPC 2.0 on stdin/stdout. Only protocol
//! messages are written to stdout; diagnostics go to stderr.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// Protocol versions this server understands, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Arguments of the `run` tool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunArgs {
    pub prompt: Option<String>,
    pub agent: Option<String>,
    pub mode: Option<String>,
    /// When true, the child `dmuxai run` uses `--read-only` (legacy ask/plan modes).
    pub read_only: bool,
    pub graph: bool,
    pub todo_id: Option<String>,
}

/// Operations behind the MCP tools. Each returns a JSON value that becomes
/// the tool's text content, or an error message (`isError: true`).
pub trait McpBackend {
    fn todo_add(&self, title: &str, body: &str) -> Result<Value, String>;
    fn todo_list(&self, status: Option<&str>) -> Result<Value, String>;
    fn run(&self, args: RunArgs) -> Result<Value, String>;
    fn status(&self) -> Result<Value, String>;
    fn events_since(&self, since: u64, limit: Option<usize>) -> Result<Value, String>;
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "todo_add",
            "description": "Add a to-do to the DmuxAI to-do board of this workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "To-do title" },
                    "body": { "type": "string", "description": "Optional details" }
                },
                "required": ["title"]
            }
        },
        {
            "name": "todo_list",
            "description": "List DmuxAI to-dos, optionally filtered by status (open, running, done).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "enum": ["open", "running", "done"] }
                }
            }
        },
        {
            "name": "run",
            "description": "Run a prompt (or an existing to-do with todo_id) through a DmuxAI agent or the graph planner and wait for the result. Returns JSON with ok, exit_code, text and messages.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "agent": { "type": "string", "description": "Runner id, e.g. agy or a custom agent name" },
                    "mode": {
                        "type": "string",
                        "description": "Deprecated, ignored. DmuxAI runs a single write-capable agent policy."
                    },
                    "graph": { "type": "boolean", "description": "Use the graph planner" },
                    "todo_id": { "type": "string", "description": "Run this to-do instead of a prompt" }
                }
            }
        },
        {
            "name": "status",
            "description": "Workspace status: agents, allowed runners, to-do counts, last graph run, last event seq and the running dashboard.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "events_since",
            "description": "State-change events (agent, node, graph, to-do) with seq greater than `since`, oldest first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since": { "type": "integer", "minimum": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 1000 }
                }
            }
        }
    ])
}

fn response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_result(outcome: Result<Value, String>) -> Value {
    match outcome {
        Ok(value) => {
            let is_error = value.get("ok") == Some(&Value::Bool(false));
            json!({
                "content": [{ "type": "text", "text": value.to_string() }],
                "structuredContent": value,
                "isError": is_error,
            })
        }
        Err(message) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn call_tool(backend: &dyn McpBackend, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "todo_add" => {
            let title = str_arg(args, "title").ok_or("title is required")?;
            backend.todo_add(title, str_arg(args, "body").unwrap_or(""))
        }
        "todo_list" => backend.todo_list(str_arg(args, "status")),
        "run" => {
            let mode = str_arg(args, "mode");
            let read_only = match mode {
                None => false,
                Some(value) => match value.trim().to_ascii_lowercase().as_str() {
                    "ask" | "plan" => true,
                    "implement" | "agent" => false,
                    other => {
                        return Err(format!(
                            "invalid mode '{other}'; use ask or plan (read-only), implement (agent), or omit mode"
                        ));
                    }
                },
            };
            let run = RunArgs {
                prompt: str_arg(args, "prompt").map(str::to_owned),
                agent: str_arg(args, "agent").map(str::to_owned),
                mode: mode.map(str::to_owned),
                read_only,
                graph: args.get("graph").and_then(Value::as_bool).unwrap_or(false),
                todo_id: str_arg(args, "todo_id").map(str::to_owned),
            };
            if run.todo_id.is_none() && run.prompt.as_deref().is_none_or(|p| p.trim().is_empty()) {
                return Err("prompt or todo_id is required".to_owned());
            }
            if run.graph && run.agent.is_some() {
                return Err("use either agent or graph, not both".to_owned());
            }
            backend.run(run)
        }
        "status" => backend.status(),
        "events_since" => {
            let since = args.get("since").and_then(Value::as_u64).unwrap_or(0);
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|l| l.clamp(1, 1000) as usize);
            backend.events_since(since, limit)
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Handles one JSON-RPC line. Returns the response line, or `None` for
/// notifications and blank input.
pub fn handle_line(backend: &dyn McpBackend, line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let message: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return Some(error_response(&Value::Null, -32700, "parse error").to_string()),
    };
    if !message.is_object() || message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        return Some(error_response(&id, -32600, "invalid request").to_string());
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response or an invalid message; responses from the client are ignored.
        if message.get("id").is_some()
            && message.get("result").is_none()
            && message.get("error").is_none()
        {
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            return Some(error_response(&id, -32600, "invalid request").to_string());
        }
        return None;
    };
    // Notifications (no id) never get a response.
    let id = message.get("id").cloned()?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let reply = match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
            let version = SUPPORTED_PROTOCOL_VERSIONS
                .iter()
                .find(|v| **v == requested)
                .copied()
                .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
            response(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "dmuxai", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "DmuxAI runs prompts through local AI coding agents. Use status first, todo_add/todo_list for the to-do board, run to execute, events_since to follow progress."
                }),
            )
        }
        "ping" => response(&id, json!({})),
        "tools/list" => response(&id, json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error_response(&id, -32602, "missing tool name").to_string());
            };
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let known = ["todo_add", "todo_list", "run", "status", "events_since"];
            if !known.contains(&name) {
                return Some(
                    error_response(&id, -32602, &format!("unknown tool: {name}")).to_string(),
                );
            }
            response(&id, tool_result(call_tool(backend, name, &args)))
        }
        "resources/list" => response(&id, json!({ "resources": [] })),
        "prompts/list" => response(&id, json!({ "prompts": [] })),
        _ => error_response(&id, -32601, &format!("method not found: {method}")),
    };
    Some(reply.to_string())
}

/// Serves MCP over the given reader/writer until EOF.
pub fn serve<R: BufRead, W: Write>(
    backend: &dyn McpBackend,
    reader: R,
    mut writer: W,
) -> std::io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if let Some(reply) = handle_line(backend, &line) {
            writer.write_all(reply.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Stub {
        runs: Mutex<Vec<RunArgs>>,
    }

    impl McpBackend for Stub {
        fn todo_add(&self, title: &str, body: &str) -> Result<Value, String> {
            Ok(json!({ "id": "t1", "title": title, "body": body }))
        }
        fn todo_list(&self, status: Option<&str>) -> Result<Value, String> {
            Ok(json!([{ "id": "t1", "status": status.unwrap_or("open") }]))
        }
        fn run(&self, args: RunArgs) -> Result<Value, String> {
            self.runs.lock().unwrap().push(args);
            Ok(json!({ "ok": true, "exit_code": 0, "text": "done" }))
        }
        fn status(&self) -> Result<Value, String> {
            Ok(json!({ "workspace": "/w" }))
        }
        fn events_since(&self, since: u64, limit: Option<usize>) -> Result<Value, String> {
            Ok(json!({ "since": since, "limit": limit }))
        }
    }

    fn call(stub: &Stub, line: Value) -> Value {
        serde_json::from_str(&handle_line(stub, &line.to_string()).expect("response")).unwrap()
    }

    // TC-768 / FR-767: initialize, tools/list, tools/call for every tool,
    // unknown tool, bad arguments, notifications and the stdio loop.
    #[test]
    fn mcp_stdio_protocol_and_tools() {
        let stub = Stub::default();
        let init = call(
            &stub,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "dmuxai");
        assert!(init["result"]["capabilities"]["tools"].is_object());

        // Notification: no response.
        assert!(
            handle_line(
                &stub,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
            )
            .is_none()
        );

        let list = call(&stub, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["todo_add", "todo_list", "run", "status", "events_since"]
        );

        let add = call(
            &stub,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"todo_add","arguments":{"title":"T","body":"B"}}}),
        );
        assert_eq!(add["result"]["isError"], false);
        let text: Value =
            serde_json::from_str(add["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text["title"], "T");

        let todo_list = call(
            &stub,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"todo_list","arguments":{"status":"done"}}}),
        );
        assert_eq!(
            todo_list["result"]["structuredContent"][0]["status"],
            "done"
        );

        let run = call(
            &stub,
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"hi","agent":"agy","mode":"ask"}}}),
        );
        assert_eq!(run["result"]["structuredContent"]["text"], "done");
        assert_eq!(stub.runs.lock().unwrap()[0].agent.as_deref(), Some("agy"));

        let status = call(
            &stub,
            json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"status"}}),
        );
        assert_eq!(status["result"]["structuredContent"]["workspace"], "/w");

        let events = call(
            &stub,
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"events_since","arguments":{"since":4,"limit":5000}}}),
        );
        assert_eq!(events["result"]["structuredContent"]["since"], 4);
        assert_eq!(events["result"]["structuredContent"]["limit"], 1000);

        // Bad arguments -> isError result; unknown tool -> JSON-RPC error.
        let bad = call(
            &stub,
            json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"run","arguments":{}}}),
        );
        assert_eq!(bad["result"]["isError"], true);
        let unknown = call(
            &stub,
            json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"nope"}}),
        );
        assert_eq!(unknown["error"]["code"], -32602);
        let missing = call(&stub, json!({"jsonrpc":"2.0","id":10,"method":"nope/nope"}));
        assert_eq!(missing["error"]["code"], -32601);
        let parse = call_raw(&stub, "{not json");
        assert_eq!(parse["error"]["code"], -32700);
        let not_object = call_raw(&stub, "[1,2]");
        assert_eq!(not_object["error"]["code"], -32600);
        let string_id = call(&stub, json!({"jsonrpc":"2.0","id":"abc","method":"ping"}));
        assert_eq!(string_id["id"], "abc");

        // The stdio loop writes one line per request and nothing for notifications.
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\n";
        let mut out = Vec::new();
        serve(&stub, input.as_bytes(), &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.lines().count(), 1);
        assert!(out.contains("\"id\":1"));
    }

    fn call_raw(stub: &Stub, line: &str) -> Value {
        serde_json::from_str(&handle_line(stub, line).unwrap()).unwrap()
    }
}
