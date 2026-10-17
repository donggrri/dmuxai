//! `dmuxai mcp-memory`: a newline-delimited JSON-RPC 2.0 MCP server on stdio
//! (SPEC-obsidian-memory INT-729, FR-730).
//!
//! The server is deliberately protocol-only: it never touches the terminal and
//! writes nothing but JSON-RPC messages to `output`. Memory access is delegated
//! to [`crate::memory::MemoryStore`]; agents can only propose, never save.

use std::io::{BufRead, Write};

use serde_json::{Map, Value, json};

use crate::memory::{MemoryHandle, ProposalInput};

/// MCP protocol version this server implements.
const PROTOCOL_VERSION: &str = "2024-11-05";
/// `serverInfo.name` advertised in `initialize`.
const SERVER_NAME: &str = "dmuxai-memory";
/// `tools/call` error text when no `[memory]` vault is configured.
const NOT_CONFIGURED: &str = "memory is not configured: set [memory] vault in dmuxai.toml";
/// Default number of search hits when the caller omits `limit` (FR-730).
const DEFAULT_SEARCH_LIMIT: usize = 20;
/// Inclusive upper bound accepted for a caller-supplied `limit` (FR-730).
const MAX_SEARCH_LIMIT: u64 = 100;
/// Maximum size of a single request line. Longer lines are answered once with
/// `-32700` and the rest of the line is discarded so the server keeps serving.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// Serve newline-delimited JSON-RPC 2.0 MCP messages from `input`, writing
/// responses to `output` (INT-729).
///
/// Blank lines are ignored, messages without an `id` are notifications and get
/// no response, malformed JSON (or invalid UTF-8, or an over-long line) yields
/// `-32700`, and unknown methods with an id yield `-32601`. A request that has
/// an id but no string `method` yields `-32600`. A `"id": null` message is
/// treated as a notification only when its method starts with `notifications/`.
/// EOF returns `Ok(())`.
pub fn serve<R: BufRead, W: Write>(
    memory: Option<MemoryHandle>,
    mut input: R,
    mut output: W,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    loop {
        match read_line(&mut input, &mut buffer)? {
            LineRead::Eof => break,
            LineRead::Oversized => {
                write_error(&mut output, Value::Null, -32700, "Parse error")?;
                continue;
            }
            LineRead::Line => {}
        }
        let line = match std::str::from_utf8(&buffer) {
            Ok(line) => line,
            Err(_) => {
                write_error(&mut output, Value::Null, -32700, "Parse error")?;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(_) => {
                write_error(&mut output, Value::Null, -32700, "Parse error")?;
                continue;
            }
        };
        let Some(object) = message.as_object() else {
            write_error(&mut output, Value::Null, -32600, "Invalid Request")?;
            continue;
        };
        // JSON-RPC notifications carry no `id`. Some clients send an explicit
        // `"id": null` for `notifications/*`; treat that as a notification too,
        // but answer every other null-id request (with a null id) so it is not
        // silently dropped.
        let id = object.get("id").cloned();
        let method = object.get("method").and_then(Value::as_str);
        let notification = match &id {
            None => true,
            Some(Value::Null) => method.is_some_and(|method| method.starts_with("notifications/")),
            Some(_) => false,
        };
        if notification {
            continue;
        }
        let id = id.unwrap_or(Value::Null);
        // An id-bearing request whose `method` is missing or not a string is
        // structurally invalid (`-32600`), not an unknown method.
        let Some(method) = method else {
            write_error(&mut output, id, -32600, "Invalid Request")?;
            continue;
        };
        match method {
            "initialize" => write_result(&mut output, id, initialize_result())?,
            "ping" => write_result(&mut output, id, json!({}))?,
            "tools/list" => write_result(&mut output, id, tools_list_result(memory.as_ref()))?,
            "tools/call" => write_result(&mut output, id, call_tool(memory.as_ref(), object))?,
            _ => write_error(&mut output, id, -32601, "Method not found")?,
        }
    }
    output.flush()
}

/// Result of reading one bounded line from the input.
enum LineRead {
    /// Input ended with no more data.
    Eof,
    /// A complete line (without its newline) was read into the buffer.
    Line,
    /// The line exceeded [`MAX_LINE_BYTES`]; its remainder was discarded.
    Oversized,
}

/// Read one `\n`-terminated line into `buffer` (excluding the newline), never
/// storing more than [`MAX_LINE_BYTES`] bytes. An over-long line is reported as
/// [`LineRead::Oversized`] and skipped up to and including its newline.
fn read_line<R: BufRead>(input: &mut R, buffer: &mut Vec<u8>) -> std::io::Result<LineRead> {
    buffer.clear();
    loop {
        let (consume, complete, overflowed) = {
            let available = match input.fill_buf() {
                Ok(available) => available,
                Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if available.is_empty() {
                return Ok(if buffer.is_empty() {
                    LineRead::Eof
                } else {
                    LineRead::Line
                });
            }
            match available.iter().position(|&byte| byte == b'\n') {
                Some(position) => {
                    let overflowed = buffer.len() + position > MAX_LINE_BYTES;
                    if !overflowed {
                        buffer.extend_from_slice(&available[..position]);
                    }
                    (position + 1, true, overflowed)
                }
                None => {
                    let overflowed = buffer.len() + available.len() > MAX_LINE_BYTES;
                    if !overflowed {
                        buffer.extend_from_slice(available);
                    }
                    (available.len(), false, overflowed)
                }
            }
        };
        input.consume(consume);
        if overflowed {
            if !complete {
                discard_line(input)?;
            }
            return Ok(LineRead::Oversized);
        }
        if complete {
            return Ok(LineRead::Line);
        }
    }
}

/// Discard input up to and including the next newline (or EOF).
fn discard_line<R: BufRead>(input: &mut R) -> std::io::Result<()> {
    loop {
        let (consume, complete) = {
            let available = match input.fill_buf() {
                Ok(available) => available,
                Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            match available.iter().position(|&byte| byte == b'\n') {
                Some(position) => (position + 1, true),
                None => (available.len(), false),
            }
        };
        input.consume(consume);
        if complete || consume == 0 {
            return Ok(());
        }
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn tools_list_result(memory: Option<&MemoryHandle>) -> Value {
    let target_default = memory
        .map(|handle| handle.project_target())
        .unwrap_or_else(|| "projects/<project>.md".to_owned());
    json!({
        "tools": [
            {
                "name": "memory_get_context",
                "description": "Return the memory context injected into every agent prompt.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "memory_search",
                "description": "Search memory entries by whitespace-separated terms.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "limit": { "type": "integer" }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            },
            {
                "name": "memory_propose",
                "description": "Queue a memory entry for human approval.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" },
                        "target": { "type": "string", "default": target_default }
                    },
                    "required": ["text"],
                    "additionalProperties": false
                }
            }
        ]
    })
}

fn call_tool(memory: Option<&MemoryHandle>, message: &Map<String, Value>) -> Value {
    let Some(handle) = memory else {
        return tool_error(NOT_CONFIGURED);
    };
    let params = message.get("params").and_then(Value::as_object);
    let name = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .or_else(|| message.get("name").and_then(Value::as_str))
        .unwrap_or("");
    let arguments = params
        .and_then(|params| params.get("arguments"))
        .or_else(|| message.get("arguments"))
        .and_then(Value::as_object);

    match name {
        "memory_get_context" => {
            let project = arg_str(arguments, "project")
                .filter(|project| !project.trim().is_empty())
                .unwrap_or(handle.project.as_str());
            let context = handle.store.build_context(project);
            if context.trim().is_empty() {
                tool_text("(no memory entries)")
            } else {
                tool_text(context)
            }
        }
        "memory_search" => {
            let query = arg_str(arguments, "query").unwrap_or("");
            if query.trim().is_empty() {
                return tool_error("empty query");
            }
            let limit = match arguments.and_then(|args| args.get("limit")) {
                None => DEFAULT_SEARCH_LIMIT,
                Some(value) => match value
                    .as_u64()
                    .filter(|limit| (1..=MAX_SEARCH_LIMIT).contains(limit))
                {
                    Some(limit) => limit as usize,
                    None => return tool_error("invalid limit (integer 1-100)"),
                },
            };
            let hits = handle.store.search(query, limit);
            if hits.is_empty() {
                tool_text("no matches")
            } else {
                let text = hits
                    .iter()
                    .map(|hit| format!("{}: {}", hit.target, hit.entry))
                    .collect::<Vec<_>>()
                    .join("\n");
                tool_text(text)
            }
        }
        "memory_propose" => {
            let text = arg_str(arguments, "text").unwrap_or("");
            let target = arg_str(arguments, "target")
                .filter(|target| !target.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| handle.project_target());
            let input = ProposalInput {
                target,
                text: text.to_owned(),
            };
            match handle.store.propose(&handle.project, "mcp", &[input]) {
                Ok(outcome) => {
                    if let Some(proposal) = outcome.added.first() {
                        tool_text(format!(
                            "queued {} for approval (approve in the dmuxai dashboard or with /memory approve {})",
                            proposal.id, proposal.id
                        ))
                    } else if let Some((_, reason)) = outcome.rejected.first() {
                        tool_error(reason.clone())
                    } else {
                        tool_error("rejected")
                    }
                }
                Err(error) => tool_error(error.to_string()),
            }
        }
        other => tool_error(format!("unknown tool: {other}")),
    }
}

fn arg_str<'a>(arguments: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a str> {
    arguments
        .and_then(|args| args.get(key))
        .and_then(Value::as_str)
}

fn tool_text(text: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": false,
    })
}

fn tool_error(text: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": true,
    })
}

fn write_result<W: Write>(output: &mut W, id: Value, result: Value) -> std::io::Result<()> {
    write_message(
        output,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    )
}

fn write_error<W: Write>(
    output: &mut W,
    id: Value,
    code: i64,
    message: &str,
) -> std::io::Result<()> {
    write_message(
        output,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }),
    )
}

fn write_message<W: Write>(output: &mut W, message: &Value) -> std::io::Result<()> {
    let encoded = serde_json::to_string(message).map_err(std::io::Error::other)?;
    output.write_all(encoded.as_bytes())?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::Path;

    use super::*;
    use crate::memory::MemoryConfig;

    fn encode(value: Value) -> String {
        serde_json::to_string(&value).unwrap()
    }

    fn exchange_bytes(memory: Option<MemoryHandle>, input: Vec<u8>) -> Vec<Value> {
        let mut output = Vec::new();
        serve(memory, Cursor::new(input), &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn exchange(memory: Option<MemoryHandle>, requests: &[String]) -> Vec<Value> {
        let input = format!("{}\n", requests.join("\n"));
        exchange_bytes(memory, input.into_bytes())
    }

    fn handle_for(root: &Path) -> MemoryHandle {
        MemoryHandle::from_config(
            &MemoryConfig {
                root: root.to_path_buf(),
                dir: crate::memory::DEFAULT_DIR.to_owned(),
                project: Some("demo".to_owned()),
                inject: true,
            },
            Path::new("/workspace"),
        )
    }

    fn vault() -> (tempfile::TempDir, MemoryHandle) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cursor-workflow").join("memory");
        std::fs::create_dir_all(root.join("gotchas")).unwrap();
        let handle = handle_for(&dir.path().join("cursor-workflow").join("memory"));
        (dir, handle)
    }

    fn tool_response(response: &Value) -> (&str, bool) {
        (
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or(""),
            response["result"]["isError"].as_bool().unwrap_or(false),
        )
    }

    // TC-729 (INT-729): initialize advertises the protocol, tools capability
    // and server identity; ping returns an empty result.
    #[test]
    fn initialize_and_ping() {
        let responses = exchange(
            None,
            &[
                encode(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})),
                encode(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"})),
            ],
        );
        assert_eq!(responses.len(), 2);
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
        assert_eq!(responses[1]["result"], json!({}));
    }

    // TC-729 (INT-729): tools/list exposes the three memory tools.
    #[test]
    fn tools_list_names_and_schemas() {
        let responses = exchange(
            None,
            &[encode(
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            )],
        );
        let tools = responses[0]["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["memory_get_context", "memory_search", "memory_propose"]
        );
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
        assert_eq!(tools[1]["inputSchema"]["required"][0], "query");
        assert_eq!(tools[2]["inputSchema"]["required"][0], "text");
    }

    // TC-729 (INT-729): malformed JSON is a -32700 error with a null id.
    #[test]
    fn malformed_json_is_parse_error() {
        let responses = exchange(None, &["{ not json".to_owned()]);
        assert_eq!(responses[0]["error"]["code"], -32700);
        assert!(responses[0]["id"].is_null());
    }

    // TC-729 (INT-729): unknown methods with an id are -32601.
    #[test]
    fn unknown_method_is_method_not_found() {
        let responses = exchange(
            None,
            &[encode(
                json!({"jsonrpc": "2.0", "id": 7, "method": "nope/nope"}),
            )],
        );
        assert_eq!(responses[0]["id"], 7);
        assert_eq!(responses[0]["error"]["code"], -32601);
    }

    // TC-729 (INT-729): notifications get no response.
    #[test]
    fn notifications_get_no_response() {
        let responses = exchange(
            None,
            &[encode(
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            )],
        );
        assert!(responses.is_empty());
    }

    // TC-729 (INT-729): string ids are echoed exactly like numeric ids.
    #[test]
    fn responses_echo_string_ids() {
        let responses = exchange(
            None,
            &[encode(
                json!({"jsonrpc": "2.0", "id": "req-7", "method": "nope/nope"}),
            )],
        );
        assert_eq!(responses[0]["id"], "req-7");
        assert_eq!(responses[0]["error"]["code"], -32601);
    }

    // TC-729 (INT-729): `"id": null` is a notification only for
    // `notifications/*`; other null-id requests are answered with a null id.
    #[test]
    fn null_id_is_notification_only_for_notifications() {
        let responses = exchange(
            None,
            &[
                encode(
                    json!({"jsonrpc": "2.0", "id": null, "method": "notifications/initialized"}),
                ),
                encode(json!({"jsonrpc": "2.0", "id": null, "method": "ping"})),
            ],
        );
        assert_eq!(responses.len(), 1);
        assert!(responses[0]["id"].is_null());
        assert_eq!(responses[0]["result"], json!({}));
    }

    // TC-729 (INT-729): an id-bearing request with a missing or non-string
    // method is -32600, not -32601.
    #[test]
    fn missing_method_is_invalid_request() {
        let responses = exchange(
            None,
            &[
                encode(json!({"jsonrpc": "2.0", "id": 1})),
                encode(json!({"jsonrpc": "2.0", "id": 2, "method": 42})),
            ],
        );
        assert_eq!(responses.len(), 2);
        for response in &responses {
            assert_eq!(response["error"]["code"], -32600);
        }
    }

    // TC-729 (INT-729): an over-long line is a single -32700 and the server
    // keeps serving subsequent requests.
    #[test]
    fn oversized_line_is_parse_error_and_serving_continues() {
        let mut input = vec![b'x'; MAX_LINE_BYTES + 32];
        input.push(b'\n');
        input.extend_from_slice(
            encode(json!({"jsonrpc": "2.0", "id": 9, "method": "ping"})).as_bytes(),
        );
        input.push(b'\n');
        let responses = exchange_bytes(None, input);
        assert_eq!(responses.len(), 2);
        assert!(responses[0]["id"].is_null());
        assert_eq!(responses[0]["error"]["code"], -32700);
        assert_eq!(responses[1]["id"], 9);
        assert_eq!(responses[1]["result"], json!({}));
    }

    // TC-729 (INT-729): a line that is not valid UTF-8 is a -32700 error rather
    // than a hard failure.
    #[test]
    fn invalid_utf8_line_is_parse_error() {
        let mut input = vec![0xff, 0xfe, 0xfd, b'\n'];
        input.extend_from_slice(
            encode(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).as_bytes(),
        );
        input.push(b'\n');
        let responses = exchange_bytes(None, input);
        assert_eq!(responses.len(), 2);
        assert!(responses[0]["id"].is_null());
        assert_eq!(responses[0]["error"]["code"], -32700);
        assert_eq!(responses[1]["id"], 1);
    }

    // TC-729 (INT-729): blank lines are ignored.
    #[test]
    fn blank_lines_are_ignored() {
        let responses = exchange(None, &["".to_owned(), "   ".to_owned()]);
        assert!(responses.is_empty());
    }

    // TC-729 (INT-729): with no vault configured every tools/call is an error.
    #[test]
    fn tools_call_without_memory_is_error() {
        let responses = exchange(
            None,
            &[encode(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "memory_get_context", "arguments": {}}
            }))],
        );
        let (text, is_error) = tool_response(&responses[0]);
        assert!(is_error);
        assert_eq!(text, NOT_CONFIGURED);
    }

    // TC-729 (INT-729): an unknown tool name is an error.
    #[test]
    fn unknown_tool_is_error() {
        let (_dir, handle) = vault();
        let responses = exchange(
            Some(handle),
            &[encode(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "memory_delete", "arguments": {}}
            }))],
        );
        let (text, is_error) = tool_response(&responses[0]);
        assert!(is_error);
        assert!(text.contains("unknown tool"));
    }

    // TC-729 (INT-729): get_context returns the block, or a placeholder when empty.
    #[test]
    fn get_context_returns_block_or_placeholder() {
        let (_dir, handle) = vault();
        let responses = exchange(
            Some(handle.clone()),
            &[
                encode(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {"name": "memory_get_context", "arguments": {}}
                })),
                encode(json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {"name": "memory_get_context", "arguments": {"project": "other"}}
                })),
            ],
        );
        let (empty_text, empty_error) = tool_response(&responses[0]);
        assert_eq!(empty_text, "(no memory entries)");
        assert!(!empty_error);

        std::fs::write(
            handle.store.root().join("human.md"),
            "- [2026-10-17] prefer rg over grep\n",
        )
        .unwrap();
        let responses = exchange(
            Some(handle),
            &[encode(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "memory_get_context", "arguments": {}}
            }))],
        );
        let (text, is_error) = tool_response(&responses[0]);
        assert!(!is_error);
        assert!(text.contains("prefer rg over grep"));
    }

    // TC-730 (FR-730): search returns `<target>: <entry>` lines, an empty
    // query is an error and no hits returns a placeholder.
    #[test]
    fn search_formats_hits_and_handles_empty() {
        let (_dir, handle) = vault();
        std::fs::write(
            handle.store.root().join("gotchas").join("windows.md"),
            "- [2026-10-17] windows path quirk\n",
        )
        .unwrap();
        let responses = exchange(
            Some(handle),
            &[
                encode(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {"name": "memory_search", "arguments": {"query": "windows"}}
                })),
                encode(json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {"name": "memory_search", "arguments": {"query": "missing"}}
                })),
                encode(json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {"name": "memory_search", "arguments": {"query": "  "}}
                })),
            ],
        );
        let (hits, hits_error) = tool_response(&responses[0]);
        assert!(!hits_error);
        assert_eq!(
            hits,
            "gotchas/windows.md: - [2026-10-17] windows path quirk"
        );

        let (none, none_error) = tool_response(&responses[1]);
        assert!(!none_error);
        assert_eq!(none, "no matches");

        let (_, empty_error) = tool_response(&responses[2]);
        assert!(empty_error);
    }

    // TC-730 (FR-730): `limit` must be an integer in 1..=100 when present;
    // otherwise the tool is an error. An explicit in-range limit is honored.
    #[test]
    fn search_validates_limit() {
        let (_dir, handle) = vault();
        std::fs::write(
            handle.store.root().join("gotchas").join("windows.md"),
            "- [2026-10-17] windows quirk 0\n- [2026-10-17] windows quirk 1\n- [2026-10-17] windows quirk 2\n",
        )
        .unwrap();

        let bad_limits = [
            json!(0),
            json!(101),
            json!(-1),
            json!(2.5),
            json!("20"),
            json!(null),
        ];
        let mut requests = vec![encode(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "memory_search", "arguments": {"query": "windows", "limit": 1}}
        }))];
        for (index, limit) in bad_limits.iter().enumerate() {
            requests.push(encode(json!({
                "jsonrpc": "2.0",
                "id": index + 2,
                "method": "tools/call",
                "params": {"name": "memory_search", "arguments": {"query": "windows", "limit": limit}}
            })));
        }

        let responses = exchange(Some(handle), &requests);

        let (limited, limited_error) = tool_response(&responses[0]);
        assert!(!limited_error);
        assert_eq!(limited.lines().count(), 1);

        for response in &responses[1..] {
            let (text, is_error) = tool_response(response);
            assert!(is_error, "limit should have been rejected: {text}");
            assert_eq!(text, "invalid limit (integer 1-100)");
        }
    }

    // TC-729 (INT-729): propose queues an item for approval and rejected input
    // is reported as an error with the reason.
    #[test]
    fn propose_queues_and_reports_rejections() {
        let (_dir, handle) = vault();
        let responses = exchange(
            Some(handle.clone()),
            &[
                encode(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {"name": "memory_propose", "arguments": {"text": "always run fmt"}}
                })),
                encode(json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {"name": "memory_propose", "arguments": {"text": "the api key is sk-secret"}}
                })),
            ],
        );
        let (queued, queued_error) = tool_response(&responses[0]);
        assert!(!queued_error);
        assert!(queued.starts_with("queued "));
        assert!(queued.contains("for approval"));
        assert!(queued.contains("/memory approve "));

        let (reason, rejected_error) = tool_response(&responses[1]);
        assert!(rejected_error);
        assert!(!reason.is_empty());

        let pending = handle.store.list_pending().unwrap();
        assert_eq!(pending.len(), 1);
    }
}
