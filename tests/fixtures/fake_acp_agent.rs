//! Minimal ACP v1 agent for tests (SPEC-acp-runners TC-1300..TC-1304).

use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut session = String::from("fake-session-1");

    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(&line).unwrap_or(serde_json::Value::Null);
        let Some(id) = value.get("id").cloned() else {
            continue;
        };
        let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");

        match method {
            "initialize" => {
                respond(
                    &mut stdout,
                    id,
                    serde_json::json!({
                        "protocolVersion": 1,
                        "agentInfo": { "name": "fake-acp", "version": "0" }
                    }),
                );
            }
            "session/new" => {
                session = "fake-session-1".to_owned();
                respond(&mut stdout, id, serde_json::json!({ "sessionId": session }));
            }
            "session/prompt" => {
                let chunk = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": { "type": "text", "text": "ACP_OK" }
                        }
                    }
                });
                writeln!(stdout, "{chunk}").unwrap();
                stdout.flush().unwrap();
                if std::env::var("FAKE_ACP_CRASH").is_ok() {
                    std::process::exit(1);
                }
                respond(
                    &mut stdout,
                    id,
                    serde_json::json!({ "stopReason": "end_turn" }),
                );
            }
            "session/request_permission" => {
                // Emulate agent asking client (reverse direction not used in this fake)
            }
            _ => {}
        }
    }
}

fn respond(stdout: &mut std::io::Stdout, id: serde_json::Value, result: serde_json::Value) {
    let msg = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
    writeln!(stdout, "{msg}").unwrap();
    stdout.flush().unwrap();
}
