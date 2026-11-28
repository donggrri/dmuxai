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
    pub model: Option<String>,
    pub approve_plan: bool,
    pub approve_plan_id: Option<String>,
    pub allow_sensitive: bool,
    pub graph: bool,
    pub todo_id: Option<String>,
    pub no_spec_first: bool,
    pub no_spec_review: bool,
}

/// Operations behind the MCP tools. Each returns a JSON value that becomes
/// the tool's text content, or an error message (`isError: true`).
pub trait McpBackend {
    fn todo_add(&self, title: &str, body: &str) -> Result<Value, String>;
    fn todo_list(&self, status: Option<&str>) -> Result<Value, String>;
    fn run(&self, args: RunArgs) -> Result<Value, String>;
    fn status(&self) -> Result<Value, String>;
    fn events_since(&self, since: u64, limit: Option<usize>) -> Result<Value, String>;
    fn models(&self, agent: Option<&str>) -> Result<Value, String>;
    fn runners_status(&self, agent: Option<&str>) -> Result<Value, String>;
    fn board_tool(&self, name: &str, args: &Value) -> Result<Value, String>;
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
            "description": "Run a prompt (or an existing to-do with todo_id) through a DmuxAI agent or the graph planner and wait for the result. Returns JSON with ok, exit_code, text, messages, and error_kind (e.g. auth for login failure, provider, process_exit). The deprecated mode argument is rejected.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "agent": { "type": "string", "description": "Runner id, e.g. agy or a custom agent name" },
                    "model": {
                        "type": "string",
                        "description": "Runner model id (same as dmuxai run --model); requires agent and cannot be used with graph"
                    },
                    "approve_plan": {
                        "type": "boolean",
                        "description": "Approve a pending plan and confirm sensitive gates like dmuxai run --approve-plan"
                    },
                    "approve_plan_id": {
                        "type": "string",
                        "description": "Run a plan saved by a previous exit 7 (same as dmuxai run --graph --approve-plan-id); 16 lowercase hex chars; requires graph"
                    },
                    "allow_sensitive": {
                        "type": "boolean",
                        "description": "Confirm every sensitive gate like dmuxai run --allow-sensitive"
                    },
                    "graph": { "type": "boolean", "description": "Use the graph planner" },
                    "no_spec_first": {
                        "type": "boolean",
                        "description": "Disable spec-first workflow for this run (same as dmuxai run --no-spec-first)"
                    },
                    "no_spec_review": {
                        "type": "boolean",
                        "description": "Skip the human spec review for this run (same as dmuxai run --no-spec-review)"
                    },
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
        },
        {
            "name": "models",
            "description": "List available models and runner model policy for all runners or a specific agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "Optional runner id (e.g. agy, cursor-agent, codex, or a custom agent name)"
                    }
                }
            }
        },
        {
            "name": "runners_status",
            "description": "Read-only runner status: installed, version, auth state and masked account for all runners or a specific agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "Optional runner id (e.g. agy, cursor-agent, codex, or a custom agent name)"
                    }
                }
            }
        },
        {
            "name": "board_add",
            "description": "Add an item to the user-level coordinator decision board.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "kind": { "type": "string", "enum": ["debug", "feature", "question", "review", "other"] },
                    "summary": { "type": "string" },
                    "grade": { "type": "string", "enum": ["A", "B", "C", "D"] },
                    "lane": { "type": "string", "enum": ["auto", "needs_decision", "approved", "in_progress", "done", "rejected", "unknown"] },
                    "bot": { "type": "string" },
                    "channel": { "type": "string" },
                    "ref": { "type": "string" },
                    "workspace": { "type": "string" },
                    "recommendation": { "type": "string" },
                    "options": { "type": "array", "items": { "type": "string" } },
                    "plan_hash": { "type": "string" },
                    "key": { "type": "string" },
                    "by": { "type": "string" }
                },
                "required": ["title", "kind"]
            }
        },
        {
            "name": "board_list",
            "description": "List decision board items.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "lane": { "type": "array", "items": { "type": "string", "enum": ["auto", "needs_decision", "approved", "in_progress", "done", "rejected", "unknown"] } },
                    "workspace": { "type": "string" },
                    "all": { "type": "boolean" }
                }
            }
        },
        {
            "name": "board_show",
            "description": "Show one decision board item.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        },
        {
            "name": "board_update",
            "description": "Update decision board item fields.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "summary": { "type": "string" },
                    "recommendation": { "type": "string" },
                    "options": { "type": "array", "items": { "type": "string" } },
                    "grade": { "type": "string", "enum": ["A", "B", "C", "D", ""] },
                    "kind": { "type": "string", "enum": ["debug", "feature", "question", "review", "other"] },
                    "run_id": { "type": "string" },
                    "if_version": { "type": "integer", "minimum": 1 },
                    "by": { "type": "string" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "board_move",
            "description": "Move a decision board item to a lane.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "lane": { "type": "string", "enum": ["auto", "needs_decision", "approved", "in_progress", "done", "rejected", "unknown"] },
                    "if_version": { "type": "integer", "minimum": 1 },
                    "by": { "type": "string" }
                },
                "required": ["id", "lane"]
            }
        },
        {
            "name": "board_plan",
            "description": "Attach or edit a graph plan on a board item.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "hash": { "type": "string" },
                    "steps": { "type": "array", "items": { "type": "object" } },
                    "tasks": { "type": "array", "items": { "type": "object" } },
                    "prompt": { "type": "string" },
                    "by": { "type": "string" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "board_pending",
            "description": "List decision board items waiting for human action (needs_decision, failed/unknown, or unchecked done).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace": { "type": "string" }
                }
            }
        },
        {
            "name": "board_say",
            "description": "Append a thread message on a board item.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "text": { "type": "string" },
                    "from": { "type": "string" }
                },
                "required": ["id", "text"]
            }
        },
        {
            "name": "board_wait",
            "description": "Wait for decision board activity (timeout capped at 300s).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "any": { "type": "boolean" },
                    "since_version": { "type": "integer", "minimum": 0 },
                    "since_seq": { "type": "integer", "minimum": 0 },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 300 },
                    "wait_for": { "type": "string", "enum": ["decision", "message", "any"] },
                    "waiter": { "type": "string" }
                }
            }
        },
        {
            "name": "board_prompt",
            "description": "Return the unmasked graph plan prompt for a board item.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "hash": { "type": "string" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "board_events",
            "description": "List decision board events with seq greater than `since`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since": { "type": "integer", "minimum": 0 }
                }
            }
        },
        {
            "name": "board_ask",
            "description": "Ask the planner a question on a decision board item or request a plan revision (blocks up to 900s).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "text": { "type": "string" },
                    "revise": { "type": "boolean" },
                    "agent": { "type": "string" },
                    "model": { "type": "string" },
                    "by": { "type": "string" }
                },
                "required": ["id", "text"]
            }
        },
        {
            "name": "board_run",
            "description": "Start executing an approved or auto decision board item's graph plan (detached; returns once in_progress).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "by": { "type": "string" }
                },
                "required": ["id"]
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
        Ok(mut value) => {
            // Exit 7 is a headless approval gate. Surface it explicitly on the
            // MCP result so clients do not have to interpret the exit code.
            let approval_required = value.get("exit_code").and_then(Value::as_i64) == Some(7);
            if approval_required {
                value["approval_required"] = Value::Bool(true);
            }
            let is_error = value.get("ok") == Some(&Value::Bool(false)) || approval_required;
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
            if args.get("mode").is_some() {
                return Err(
                    "mode was removed; DmuxAI runs a single agent mode (write-capable). Omit mode."
                        .to_owned(),
                );
            }
            let run = RunArgs {
                prompt: str_arg(args, "prompt").map(str::to_owned),
                agent: str_arg(args, "agent").map(str::to_owned),
                model: str_arg(args, "model").map(str::to_owned),
                approve_plan: args
                    .get("approve_plan")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                approve_plan_id: str_arg(args, "approve_plan_id").map(str::to_owned),
                allow_sensitive: args
                    .get("allow_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                graph: args.get("graph").and_then(Value::as_bool).unwrap_or(false),
                todo_id: str_arg(args, "todo_id").map(str::to_owned),
                no_spec_first: args
                    .get("no_spec_first")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                no_spec_review: args
                    .get("no_spec_review")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
            if run.todo_id.is_none() && run.prompt.as_deref().is_none_or(|p| p.trim().is_empty()) {
                return Err("prompt or todo_id is required".to_owned());
            }
            if run.graph && run.agent.is_some() {
                return Err("use either agent or graph, not both".to_owned());
            }
            if run.approve_plan_id.is_some() && !run.graph {
                return Err("approve_plan_id requires graph".to_owned());
            }
            // FR-2550: reject a malformed id before the backend touches the
            // pending-plan path.
            if let Some(plan_id) = &run.approve_plan_id
                && !crate::state_path::valid_pending_plan_hash(plan_id)
            {
                return Err("approve_plan_id must be 16 lowercase hex characters".to_owned());
            }
            if run.model.is_some() && run.graph {
                return Err("model cannot be used with graph".to_owned());
            }
            if run.model.is_some() && run.agent.is_none() {
                return Err("model needs agent".to_owned());
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
        "models" => backend.models(str_arg(args, "agent")),
        "runners_status" => backend.runners_status(str_arg(args, "agent")),
        name if name.starts_with("board_") => backend.board_tool(name, args),
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
            let known = [
                "todo_add",
                "todo_list",
                "run",
                "status",
                "events_since",
                "models",
                "runners_status",
                "board_add",
                "board_list",
                "board_show",
                "board_update",
                "board_move",
                "board_plan",
                "board_pending",
                "board_say",
                "board_wait",
                "board_prompt",
                "board_events",
                "board_ask",
                "board_run",
            ];
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
        run_result: Mutex<Option<Value>>,
        models_calls: Mutex<Vec<Option<String>>>,
        runners_status_calls: Mutex<Vec<Option<String>>>,
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
            Ok(self
                .run_result
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| json!({ "ok": true, "exit_code": 0, "text": "done" })))
        }
        fn status(&self) -> Result<Value, String> {
            Ok(json!({ "workspace": "/w" }))
        }
        fn events_since(&self, since: u64, limit: Option<usize>) -> Result<Value, String> {
            Ok(json!({ "since": since, "limit": limit }))
        }
        fn models(&self, agent: Option<&str>) -> Result<Value, String> {
            self.models_calls
                .lock()
                .unwrap()
                .push(agent.map(str::to_owned));
            Ok(json!({ "ok": true, "agent": agent, "runners": [] }))
        }
        fn runners_status(&self, agent: Option<&str>) -> Result<Value, String> {
            self.runners_status_calls
                .lock()
                .unwrap()
                .push(agent.map(str::to_owned));
            Ok(json!({ "ok": true, "runners": [{ "id": agent }] }))
        }
        fn board_tool(&self, name: &str, args: &Value) -> Result<Value, String> {
            Ok(crate::board::mcp_invoke(
                std::path::Path::new("/tmp"),
                name,
                args,
            ))
        }
    }

    fn call(stub: &Stub, line: Value) -> Value {
        serde_json::from_str(&handle_line(stub, &line.to_string()).expect("response")).unwrap()
    }

    // TC-3113 / FR-3113: board MCP tools are listed, every optional CLI flag
    // has a schema field, and calls map validation errors to exit code 2.
    #[test]
    fn tc_3113_mcp_board_tools_listed_and_mapped() {
        let state_home = tempfile::tempdir().unwrap();
        let _guard = crate::state_path::test_env_lock();
        // SAFETY: test-only mutation of the process environment.
        unsafe { std::env::set_var("XDG_STATE_HOME", state_home.path()) };
        let stub = Stub::default();
        let list = call(&stub, json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        let tools = list["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        let properties = |name: &str| -> Vec<String> {
            tools
                .iter()
                .find(|t| t["name"] == name)
                .and_then(|t| t["inputSchema"]["properties"].as_object())
                .map(|map| map.keys().cloned().collect())
                .unwrap_or_default()
        };
        let expected: &[(&str, &[&str])] = &[
            (
                "board_add",
                &[
                    "title",
                    "kind",
                    "summary",
                    "grade",
                    "lane",
                    "bot",
                    "channel",
                    "ref",
                    "workspace",
                    "recommendation",
                    "options",
                    "plan_hash",
                    "key",
                    "by",
                ],
            ),
            ("board_list", &["lane", "workspace", "all"]),
            ("board_show", &["id"]),
            (
                "board_update",
                &[
                    "id",
                    "title",
                    "summary",
                    "recommendation",
                    "options",
                    "grade",
                    "kind",
                    "run_id",
                    "if_version",
                    "by",
                ],
            ),
            ("board_move", &["id", "lane", "if_version", "by"]),
            (
                "board_plan",
                &["id", "hash", "steps", "tasks", "prompt", "by"],
            ),
            ("board_pending", &["workspace"]),
            ("board_say", &["id", "text", "from"]),
            (
                "board_wait",
                &[
                    "id",
                    "any",
                    "since_version",
                    "since_seq",
                    "timeout_secs",
                    "wait_for",
                    "waiter",
                ],
            ),
            ("board_prompt", &["id", "hash"]),
            ("board_events", &["since"]),
        ];
        let ok_call = call(
            &stub,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"board_add","arguments":{"title":"T","kind":"debug"}}}),
        );
        let bad = call(
            &stub,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"board_add","arguments":{"title":"T","kind":"not-a-kind"}}}),
        );
        let bad_lane = call(
            &stub,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"board_add","arguments":{"title":"T","kind":"debug","lane":"not-a-lane"}}}),
        );
        let events = call(
            &stub,
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"board_events","arguments":{"since":0}}}),
        );
        unsafe { std::env::remove_var("XDG_STATE_HOME") };

        for (name, wanted) in expected {
            assert!(names.contains(name), "missing MCP tool {name}");
            let actual = properties(name);
            for property in *wanted {
                assert!(
                    actual.iter().any(|p| p == property),
                    "tool {name} schema missing property {property}: {actual:?}"
                );
            }
            assert_eq!(
                actual.len(),
                wanted.len(),
                "tool {name} has unexpected schema fields: {actual:?}"
            );
        }
        assert_eq!(ok_call["result"]["structuredContent"]["ok"], true);
        assert_eq!(bad["result"]["isError"], true);
        assert_eq!(bad["result"]["structuredContent"]["exit_code"], 2);
        assert_eq!(bad_lane["result"]["isError"], true);
        assert_eq!(bad_lane["result"]["structuredContent"]["exit_code"], 2);
        assert_eq!(events["result"]["structuredContent"]["ok"], true);
    }

    // TC-768 / FR-767: initialize, tools/list, tools/call for every tool,
    // unknown tool, bad arguments, notifications and the stdio loop.
    #[test]
    fn tc_768_mcp_stdio_protocol_and_tools() {
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
        assert!(
            names.contains(&"todo_add"),
            "tools list must include todo_add"
        );
        for tool in [
            "board_add",
            "board_list",
            "board_show",
            "board_update",
            "board_move",
            "board_plan",
            "board_pending",
            "board_say",
            "board_wait",
        ] {
            assert!(names.contains(&tool), "missing tool {tool}");
        }
        assert!(
            !names.contains(&"board_decide"),
            "board_decide must not be exposed over MCP"
        );
        let run_tool = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "run")
            .expect("run tool");
        let run_properties = &run_tool["inputSchema"]["properties"];
        // TC-2512 / FR-2511: the run tool schema advertises the parity fields
        // and never the removed `mode` argument.
        for field in ["model", "approve_plan", "allow_sensitive"] {
            assert!(
                run_properties.get(field).is_some(),
                "run schema is missing {field}"
            );
        }
        assert!(
            run_properties.get("mode").is_none(),
            "the deprecated mode argument must not be advertised"
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
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"hi","agent":"agy","model":"m","approve_plan":true,"allow_sensitive":true}}}),
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

    // TC-3701 / FR-3701: MCP `run` accepts `no_spec_review`.
    #[test]
    fn tc_3701_mcp_run_no_spec_review() {
        let stub = Stub::default();
        let result = call(
            &stub,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"x","agent":"agy","no_spec_review":true}}}),
        );
        assert_eq!(result["result"]["isError"], false);
        assert!(stub.runs.lock().unwrap()[0].no_spec_review);
        let schema = tool_definitions().to_string();
        assert!(schema.contains("no_spec_review"));
    }

    // TC-3708 / FR-3708: MCP exposes no spec review decision (or any spec review) tool.
    #[test]
    fn tc_3708_mcp_has_no_spec_review_tool() {
        let stub = Stub::default();
        let list = call(&stub, json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        let tools = list["result"]["tools"].as_array().expect("tools array");
        for tool in tools {
            let name = tool["name"].as_str().unwrap_or_default();
            assert!(
                !name.contains("spec_review") && !name.contains("spec-review"),
                "{name}"
            );
            assert!(
                !name.contains("approve") && !name.contains("decide"),
                "{name}"
            );
        }
        let denied = call(
            &stub,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"spec_review_decide","arguments":{"id":"x","verdict":"approve"}}}),
        );
        assert!(
            denied.get("error").is_some() || denied["result"]["isError"] == true,
            "{denied}"
        );
    }

    // TC-3513 / FR-3513: MCP `run` accepts `no_spec_first`.
    #[test]
    fn tc_3513_mcp_run_no_spec_first() {
        let stub = Stub::default();
        let result = call(
            &stub,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"x","agent":"agy","no_spec_first":true}}}),
        );
        assert_eq!(result["result"]["isError"], false);
        assert!(stub.runs.lock().unwrap()[0].no_spec_first);
    }

    #[test]
    fn tc_2509_mcp_run_forwards_parity_fields() {
        let stub = Stub::default();
        let result = call(
            &stub,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"ship it","agent":"agy","model":"gemini-3.8-flash-high","approve_plan":true,"allow_sensitive":true}}}),
        );
        assert_eq!(result["result"]["isError"], false);
        {
            let runs = stub.runs.lock().unwrap();
            assert_eq!(
                runs[0],
                RunArgs {
                    prompt: Some("ship it".to_owned()),
                    agent: Some("agy".to_owned()),
                    model: Some("gemini-3.8-flash-high".to_owned()),
                    approve_plan: true,
                    approve_plan_id: None,
                    allow_sensitive: true,
                    graph: false,
                    todo_id: None,
                    no_spec_first: false,
                    no_spec_review: false,
                }
            );
        }

        let todo = call(
            &stub,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run","arguments":{"todo_id":"t1","agent":"agy","model":"m","approve_plan":true,"allow_sensitive":true}}}),
        );
        assert_eq!(todo["result"]["isError"], false);
        let runs = stub.runs.lock().unwrap();
        assert_eq!(runs[1].todo_id.as_deref(), Some("t1"));
        assert_eq!(runs[1].model.as_deref(), Some("m"));
        assert!(runs[1].approve_plan);
        assert!(runs[1].allow_sensitive);
    }

    // TC-2549 / FR-2548: `run` accepts approve_plan_id and requires graph.
    #[test]
    fn tc_2549_mcp_run_approve_plan_id_requires_graph() {
        let stub = Stub::default();
        let result = call(
            &stub,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"plan it","graph":true,"approve_plan_id":"deadbeefdeadbeef"}}}),
        );
        assert_eq!(result["result"]["isError"], false);
        {
            let runs = stub.runs.lock().unwrap();
            assert_eq!(runs[0].approve_plan_id.as_deref(), Some("deadbeefdeadbeef"));
            assert!(runs[0].graph);
        }

        let rejected = call(
            &stub,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"plan it","approve_plan_id":"deadbeefdeadbeef"}}}),
        );
        assert_eq!(
            rejected["result"]["content"][0]["text"],
            "approve_plan_id requires graph"
        );
        assert_eq!(stub.runs.lock().unwrap().len(), 1);

        let tools = call(&stub, json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}));
        let run_tool = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "run")
            .expect("run tool");
        assert_eq!(
            run_tool["inputSchema"]["properties"]["approve_plan_id"]["type"],
            "string"
        );
    }

    // TC-2550 / FR-2550: the run tool rejects approve_plan_id values that are
    // not exactly 16 lowercase hex characters, without calling the backend.
    #[test]
    fn tc_2550_mcp_run_approve_plan_id_rejects_non_hex() {
        let stub = Stub::default();
        for bad in [
            "../x",
            "ABC",
            "ABCDEF0123456789",
            "deadbeefdeadbee",
            "deadbeefdeadbeef1",
            "g000000000000000",
        ] {
            let rejected = call(
                &stub,
                json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"plan it","graph":true,"approve_plan_id":bad}}}),
            );
            assert_eq!(
                rejected["result"]["content"][0]["text"],
                "approve_plan_id must be 16 lowercase hex characters",
                "{bad}"
            );
            assert_eq!(
                rejected["result"]["isError"], true,
                "non-hex id {bad} must be an error"
            );
        }
        assert_eq!(stub.runs.lock().unwrap().len(), 0, "backend never called");
    }

    // TC-2510 / FR-2511: the deprecated mode is rejected like the CLI's
    // removed --mode, and the model rules mirror parse_run_options.
    #[test]
    fn tc_2510_mcp_run_rejects_mode_and_validates_model() {
        let stub = Stub::default();
        for mode in ["ask", "plan", "implement", "agent", "anything"] {
            let result = call(
                &stub,
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"hi","agent":"agy","mode":mode}}}),
            );
            assert_eq!(result["result"]["isError"], true);
            let text = result["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("mode was removed"), "{text}");
            assert!(text.contains("Omit mode"), "{text}");
        }

        let model_without_agent = call(
            &stub,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"hi","model":"m"}}}),
        );
        assert_eq!(
            model_without_agent["result"]["content"][0]["text"],
            "model needs agent"
        );

        let model_with_graph = call(
            &stub,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"hi","graph":true,"model":"m"}}}),
        );
        assert_eq!(
            model_with_graph["result"]["content"][0]["text"],
            "model cannot be used with graph"
        );

        let agent_with_graph = call(
            &stub,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"hi","agent":"agy","graph":true}}}),
        );
        assert_eq!(
            agent_with_graph["result"]["content"][0]["text"],
            "use either agent or graph, not both"
        );

        assert!(
            stub.runs.lock().unwrap().is_empty(),
            "invalid runs must never reach the backend"
        );
    }

    // TC-2512 / FR-2511: the run tool schema advertises the parity fields
    // and never the removed `mode` argument.
    #[test]
    fn tc_2512_mcp_run_schema_advertises_parity_fields_and_omits_mode() {
        let stub = Stub::default();
        let list = call(&stub, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        let run_tool = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "run")
            .expect("run tool");
        let run_properties = &run_tool["inputSchema"]["properties"];
        for field in ["model", "approve_plan", "allow_sensitive"] {
            assert!(
                run_properties.get(field).is_some(),
                "run schema is missing {field}"
            );
        }
        assert!(
            run_properties.get("mode").is_none(),
            "the deprecated mode argument must not be advertised"
        );
    }

    // TC-2511 / FR-2512: exit 7 is an approval gate, not an opaque failure.
    #[test]
    fn tc_2511_mcp_run_surfaces_exit_7_approval_required() {
        let stub = Stub::default();
        *stub.run_result.lock().unwrap() = Some(json!({
            "ok": false,
            "exit_code": 7,
            "error": "approval required (headless)",
            "text": "step one"
        }));
        let result = call(
            &stub,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"plan it"}}}),
        );
        assert_eq!(result["result"]["isError"], true);
        assert_eq!(result["result"]["structuredContent"]["exit_code"], 7);
        assert_eq!(
            result["result"]["structuredContent"]["approval_required"],
            true
        );
        assert_eq!(
            result["result"]["structuredContent"]["error"],
            "approval required (headless)"
        );
        assert_eq!(result["result"]["structuredContent"]["text"], "step one");

        *stub.run_result.lock().unwrap() = Some(json!({
            "ok": true,
            "exit_code": 0,
            "text": "done"
        }));
        let success = call(
            &stub,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run","arguments":{"prompt":"hi"}}}),
        );
        assert!(
            success["result"]["structuredContent"]
                .get("approval_required")
                .is_none()
        );
    }

    fn call_raw(stub: &Stub, line: &str) -> Value {
        serde_json::from_str(&handle_line(stub, line).unwrap()).unwrap()
    }

    // TC-2838 / FR-2837: docs and MCP run description document error_kind "auth"
    #[test]
    fn tc_2838_mcp_documents_auth_and_models_tool() {
        let tools = tool_definitions();
        let run_tool = tools
            .as_array()
            .and_then(|arr| arr.iter().find(|t| t["name"] == "run"))
            .expect("run tool");
        let desc = run_tool["description"].as_str().unwrap_or_default();
        assert!(
            desc.contains("auth"),
            "run tool description should mention auth: {desc}"
        );

        let models_tool = tools
            .as_array()
            .and_then(|arr| arr.iter().find(|t| t["name"] == "models"))
            .expect("models tool");
        assert!(
            models_tool["inputSchema"]["properties"]
                .get("agent")
                .is_some(),
            "models tool schema should declare agent"
        );

        let api_md = include_str!("../docs/API.md");
        assert!(
            api_md.contains("`auth` (sign-in required; no runner fallback)"),
            "docs/API.md should document auth error_kind"
        );
    }

    // TC-2838 / FR-2832: MCP models tool dispatches agent parameter to backend
    #[test]
    fn tc_2838_mcp_models_tool_dispatch() {
        let stub = Stub::default();
        let res = call(
            &stub,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "models",
                    "arguments": {
                        "agent": "agy"
                    }
                }
            }),
        );
        assert_eq!(res["result"]["isError"], false);
        let calls = stub.models_calls.lock().unwrap();
        assert_eq!(calls.as_slice(), &[Some("agy".to_owned())]);
    }

    // TC-3309 / FR-3310: the read-only runners_status tool is listed with only
    // an optional agent argument, has no write counterpart, and dispatches.
    #[test]
    fn tc_3309_mcp_runners_status() {
        let stub = Stub::default();
        let list = call(&stub, json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        let tools = list["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"runners_status"), "tools: {names:?}");
        assert!(
            names.iter().all(|name| !name.contains("enable")),
            "runner enable must not be exposed over MCP: {names:?}"
        );

        let tool = tools
            .iter()
            .find(|t| t["name"] == "runners_status")
            .expect("runners_status tool");
        let properties: Vec<String> = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(properties, vec!["agent".to_owned()]);

        let res = call(
            &stub,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "runners_status",
                    "arguments": { "agent": "agy" }
                }
            }),
        );
        assert_eq!(res["result"]["isError"], false);
        assert_eq!(res["result"]["structuredContent"]["ok"], true);
        assert_eq!(
            res["result"]["structuredContent"]["runners"][0]["id"],
            "agy"
        );
        assert_eq!(
            stub.runners_status_calls.lock().unwrap().as_slice(),
            &[Some("agy".to_owned())]
        );
    }

    // TC-3205 / FR-3204
    #[test]
    fn tc_3205_mcp_accepts_cursor_agent_keyword() {
        let stub = Stub::default();
        let res = call(
            &stub,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "models",
                    "arguments": {
                        "agent": "cursor-agent"
                    }
                }
            }),
        );
        assert_eq!(res["result"]["isError"], false);
        let calls = stub.models_calls.lock().unwrap();
        assert_eq!(calls.as_slice(), &[Some("cursor-agent".to_owned())]);
        assert_eq!(
            crate::runner_id::canonical_runner_id(" cursor-agent "),
            crate::runner_id::CURSOR_RUNNER_ID
        );
    }
}
