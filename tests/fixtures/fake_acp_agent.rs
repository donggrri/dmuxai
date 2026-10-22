//! Minimal ACP v1 agent for tests (SPEC-acp-runners TC-1300..TC-1304, TC-1317).
//! Validates request params strictly (mirrors Cursor/grok/opencode expectations).

use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut session = String::from("fake-session-1");
    let mut reader = std::io::BufReader::new(stdin.lock());
    let mut line_buf = String::new();

    while reader
        .read_line(&mut line_buf)
        .ok()
        .filter(|n| *n > 0)
        .is_some()
    {
        let line = line_buf.trim_end_matches(['\r', '\n']).to_owned();
        line_buf.clear();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(&line).unwrap_or(serde_json::Value::Null);
        let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = value.get("id").cloned();
        let params = value
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        match method {
            "initialize" => {
                if let Some(id) = id {
                    respond(
                        &mut stdout,
                        id,
                        serde_json::json!({
                            "protocolVersion": 1,
                            "agentInfo": { "name": "fake-acp", "version": "0" },
                            "agentCapabilities": {
                                "loadSession": true,
                                "resume": true,
                                "set_mode": true,
                                "set_model": true,
                                "set_config_option": true
                            }
                        }),
                    );
                }
            }
            "session/new" => {
                if let Some(id) = id {
                    if let Err(msg) = require_session_new_params(&params) {
                        error(&mut stdout, id, -32602, &msg);
                        continue;
                    }
                    if std::env::var("FAKE_ACP_SESSION_NEW_ERROR").is_ok() {
                        error(&mut stdout, id, -32603, "session/new refused for test");
                        continue;
                    }
                    session = "fake-session-1".to_owned();
                    let fast_only = std::env::var("FAKE_ACP_FAST_ONLY").is_ok();
                    let current = if fast_only {
                        "composer-2.5[fast=true]"
                    } else {
                        "composer-2.5"
                    };
                    let available = if fast_only {
                        serde_json::json!(["composer-2.5[fast=true]"])
                    } else {
                        serde_json::json!(["composer-2.5", "composer-2.5[fast=true]"])
                    };
                    if let Some(path) = std::env::var_os("FAKE_ACP_MCP_CAPTURE") {
                        let _ = std::fs::write(path, params["mcpServers"].to_string());
                    }
                    // NFR-1410: FAKE_ACP_MODES = modes (default) | config | both | none.
                    let shape = std::env::var("FAKE_ACP_MODES").unwrap_or_else(|_| "modes".into());
                    let mut result = serde_json::json!({
                        "sessionId": session,
                        "models": {
                            "currentModelId": current,
                            "availableModels": available
                        }
                    });
                    if shape == "modes" || shape == "both" {
                        result["modes"] = serde_json::json!({
                            "currentModeId": "agent",
                            "availableModes": [{"id": "agent"}, {"id": "ask"}]
                        });
                    }
                    if shape == "config" || shape == "both" {
                        result["configOptions"] = serde_json::json!([{
                            "id": "mode", "category": "mode", "type": "select",
                            "currentValue": "agent",
                            "options": [{"value": "agent"}, {"value": "plan"}]
                        }]);
                    }
                    respond(&mut stdout, id, result);
                }
            }
            "session/load" => {
                if let Some(id) = id {
                    if let Err(msg) = require_resume_params(&params) {
                        error(&mut stdout, id, -32602, &msg);
                        continue;
                    }
                    let sid = params
                        .get("sessionId")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&session);
                    session = sid.to_owned();
                    respond(&mut stdout, id, serde_json::json!({ "sessionId": session }));
                }
            }
            "session/resume" => {
                if let Some(id) = id {
                    if let Err(msg) = require_resume_params(&params) {
                        error(&mut stdout, id, -32602, &msg);
                        continue;
                    }
                    let sid = params
                        .get("sessionId")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&session);
                    session = sid.to_owned();
                    respond(&mut stdout, id, serde_json::json!({ "sessionId": session }));
                }
            }
            "session/set_config_option" | "session/set_mode" | "session/set_model" => {
                if let Some(path) = std::env::var_os("FAKE_ACP_MODE_CAPTURE")
                    && (method == "session/set_mode"
                        || params.get("configId").and_then(|v| v.as_str()) == Some("mode"))
                {
                    let value = params
                        .get("modeId")
                        .or_else(|| params.get("value"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let _ = std::fs::write(path, format!("{method} {value}"));
                }
                if method == "session/set_mode"
                    && let Err(msg) = require_set_mode_params(&params)
                    && let Some(req_id) = id.clone()
                {
                    error(&mut stdout, req_id, -32602, &msg);
                    continue;
                }
                if method == "session/set_config_option"
                    && let Some(path) = std::env::var_os("FAKE_ACP_MODEL_CAPTURE")
                {
                    let model = params.get("value").and_then(|v| v.as_str()).unwrap_or("");
                    let _ = std::fs::write(path, model);
                }
                if method == "session/set_model" {
                    let model = params
                        .get("modelId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("composer-2.5");
                    if let Some(path) = std::env::var_os("FAKE_ACP_MODEL_CAPTURE") {
                        let _ = std::fs::write(path, model);
                    }
                }
                if let Some(id) = id {
                    let result = if method == "session/set_model" {
                        serde_json::json!({
                            "models": { "currentModelId": params.get("modelId").cloned().unwrap_or(serde_json::json!("composer-2.5")) }
                        })
                    } else if method == "session/set_config_option" {
                        serde_json::json!({
                            "value": params.get("value").cloned().unwrap_or(serde_json::json!(""))
                        })
                    } else {
                        serde_json::json!({})
                    };
                    respond(&mut stdout, id, result);
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
                if let Some(path) = std::env::var_os("FAKE_ACP_PROMPT_CAPTURE") {
                    let _ = std::fs::write(path, b"1");
                }
                let mut denied = false;
                if std::env::var("FAKE_ACP_PERMISSION_OUTSIDE").is_ok() {
                    let perm = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "session/request_permission",
                        "id": 99,
                        "params": {
                            "options": [
                                {"optionId": "allow", "kind": "allow_once"},
                                {"optionId": "reject", "kind": "reject-once"}
                            ],
                            "path": "/etc/passwd"
                        }
                    });
                    writeln!(stdout, "{perm}").unwrap();
                    stdout.flush().unwrap();
                    line_buf.clear();
                    let _ = reader.read_line(&mut line_buf);
                    denied = !line_buf.contains("\"optionId\":\"allow\"");
                    line_buf.clear();
                }
                if denied {
                    if let Some(id) = id {
                        respond(
                            &mut stdout,
                            id,
                            serde_json::json!({ "stopReason": "cancelled" }),
                        );
                    }
                    continue;
                }
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

fn require_session_new_params(params: &serde_json::Value) -> Result<(), String> {
    if params
        .get("cwd")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.is_empty())
    {
        return Err("session/new missing cwd".to_owned());
    }
    if !params
        .get("mcpServers")
        .map(|v| v.is_array())
        .unwrap_or(false)
    {
        return Err("session/new missing mcpServers array".to_owned());
    }
    Ok(())
}

fn require_resume_params(params: &serde_json::Value) -> Result<(), String> {
    if params
        .get("sessionId")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.is_empty())
    {
        return Err("missing sessionId".to_owned());
    }
    if params
        .get("cwd")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.is_empty())
    {
        return Err("missing cwd".to_owned());
    }
    if !params
        .get("mcpServers")
        .map(|v| v.is_array())
        .unwrap_or(false)
    {
        return Err("missing mcpServers array".to_owned());
    }
    Ok(())
}

fn require_set_mode_params(params: &serde_json::Value) -> Result<(), String> {
    if params
        .get("modeId")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.is_empty())
    {
        return Err("session/set_mode requires modeId".to_owned());
    }
    if params.get("mode").is_some() {
        return Err("session/set_mode must not use legacy mode field".to_owned());
    }
    Ok(())
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
