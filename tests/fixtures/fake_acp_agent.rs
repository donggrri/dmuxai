//! Minimal ACP v1 agent for tests (SPEC-acp-runners TC-1300..TC-1304, TC-1317).

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
        let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = value.get("id").cloned();

        match method {
            "initialize" => {
                if let Some(id) = id {
                    respond(
                        &mut stdout,
                        id,
                        serde_json::json!({
                            "protocolVersion": 1,
                            "agentInfo": { "name": "fake-acp", "version": "0" },
                            "agentCapabilities": { "loadSession": true, "resume": true }
                        }),
                    );
                }
            }
            "session/new" => {
                if let Some(id) = id {
                    session = "fake-session-1".to_owned();
                    respond(&mut stdout, id, serde_json::json!({ "sessionId": session }));
                }
            }
            "session/load" | "session/resume" => {
                if let Some(id) = id {
                    let sid = value
                        .pointer("/params/sessionId")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&session);
                    session = sid.to_owned();
                    respond(&mut stdout, id, serde_json::json!({ "sessionId": session }));
                }
            }
            "session/set_config_option" | "session/set_mode" => {
                if let Some(id) = id {
                    respond(&mut stdout, id, serde_json::json!({}));
                }
            }
            "session/cancel" => {
                if let Some(path) = std::env::var_os("FAKE_ACP_CANCEL_CAPTURE") {
                    let _ = std::fs::write(path, b"1");
                }
                if let Some(id) = id {
                    respond(&mut stdout, id, serde_json::json!({}));
                }
            }
            "session/prompt" => {
                if std::env::var("FAKE_ACP_SLEEP_MS").is_ok() {
                    let ms = std::env::var("FAKE_ACP_SLEEP_MS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(5000);
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
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
                if let Some(id) = id {
                    respond(
                        &mut stdout,
                        id,
                        serde_json::json!({ "stopReason": "end_turn" }),
                    );
                }
            }
            "session/request_permission" => {}
            _ => {
                if let Some(id) = id {
                    error(&mut stdout, id, -32601, "Method not found");
                }
            }
        }
    }
}

fn respond(stdout: &mut std::io::Stdout, id: serde_json::Value, result: serde_json::Value) {
    let msg = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
    writeln!(stdout, "{msg}").unwrap();
    stdout.flush().unwrap();
}

fn error(stdout: &mut std::io::Stdout, id: serde_json::Value, code: i64, message: &str) {
    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    writeln!(stdout, "{msg}").unwrap();
    stdout.flush().unwrap();
}
