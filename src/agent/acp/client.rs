//! ACP v1 stdio client (Linux-supported path per SPEC-acp-runners).

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

use crate::agent::{Agent, AgentEvent, AgentOutput, OutputContext};
use dmuxai::process::strip_dashboard_env;

use super::permissions::{PermissionContext, choose_permission_option};
use super::updates::map_session_update;

const CLIENT_NAME: &str = "dmuxai";
const CLIENT_VERSION: &str = "0.2.2";
const PROTOCOL_VERSION: i64 = 1;

#[allow(dead_code)]
pub struct AcpSession {
    pub session_id: String,
}

pub struct AcpRunOutcome {
    pub session_id: Option<String>,
    pub stop_reason: Option<String>,
}

struct PendingRequest {
    #[allow(dead_code)]
    response: oneshot::Sender<Result<Value, String>>,
}

struct SharedState {
    pending: Mutex<HashMap<u64, PendingRequest>>,
    next_id: Mutex<u64>,
    cancelled: AtomicBool,
}

impl SharedState {
    fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().unwrap();
        *id += 1;
        *id
    }
}

/// Run one ACP turn: spawn `program` with `args`, drive protocol, emit events.
#[allow(clippy::too_many_arguments)]
pub async fn run_acp_turn(
    program: &Path,
    args: &[String],
    cwd: &Path,
    prompt: &str,
    resume_session: Option<&str>,
    permission_ctx: PermissionContext,
    event_tx: &mpsc::Sender<AgentOutput>,
    agent: Agent,
    context: &OutputContext,
) -> io::Result<AcpRunOutcome> {
    let mut command = Command::new(program);
    command.args(args);
    command.current_dir(cwd);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    strip_dashboard_env(command.as_std_mut());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }

    let mut child = command.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("acp stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("acp stdout"))?;

    let state = Arc::new(SharedState {
        pending: Mutex::new(HashMap::new()),
        next_id: Mutex::new(0),
        cancelled: AtomicBool::new(false),
    });

    let (line_tx, mut line_rx) = mpsc::channel::<String>(64);
    let reader = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line_tx.send(line).await.is_err() {
                break;
            }
        }
    });

    let mut stdin = stdin;
    let session_id = if let Some(id) = resume_session {
        id.to_owned()
    } else {
        initialize_and_create_session(&mut stdin, &mut line_rx, &state, cwd)
            .await
            .map_err(io::Error::other)?
    };

    send_event(
        event_tx,
        agent,
        context,
        AgentEvent::ProviderSessionBound(session_id.clone()),
    )
    .await;
    send_event(event_tx, agent, context, AgentEvent::Started).await;

    let prompt_result = run_prompt(
        &mut stdin,
        &mut line_rx,
        &state,
        &session_id,
        prompt,
        &permission_ctx,
        event_tx,
        agent,
        context,
    )
    .await;

    if state.cancelled.load(Ordering::SeqCst) {
        let _ = write_notification(
            &mut stdin,
            "session/cancel",
            json!({ "sessionId": session_id }),
        )
        .await;
    }

    let _ = child.start_kill();
    let status = child.wait().await?;
    reader.abort();

    match prompt_result {
        Ok(outcome) => {
            if status.success() || outcome.stop_reason.is_some() {
                send_event(event_tx, agent, context, AgentEvent::Completed).await;
                Ok(outcome)
            } else {
                let msg = format!("ACP agent exited with {status}");
                send_event(event_tx, agent, context, AgentEvent::Error(msg.clone())).await;
                Err(io::Error::other(msg))
            }
        }
        Err(error) => {
            send_event(event_tx, agent, context, AgentEvent::Error(error.clone())).await;
            Err(io::Error::other(error))
        }
    }
}

async fn initialize_and_create_session(
    stdin: &mut ChildStdin,
    line_rx: &mut mpsc::Receiver<String>,
    state: &Arc<SharedState>,
    cwd: &Path,
) -> Result<String, String> {
    let init_params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {},
        "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION }
    });
    let _ = request(stdin, line_rx, state, "initialize", init_params).await?;

    let new_params = json!({
        "cwd": cwd.to_string_lossy(),
        "mcpServers": []
    });
    let created = request(stdin, line_rx, state, "session/new", new_params).await?;
    created
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "session/new missing sessionId".to_owned())
}

#[allow(clippy::too_many_arguments)]
async fn run_prompt(
    stdin: &mut ChildStdin,
    line_rx: &mut mpsc::Receiver<String>,
    state: &Arc<SharedState>,
    session_id: &str,
    prompt: &str,
    permission_ctx: &PermissionContext,
    event_tx: &mpsc::Sender<AgentOutput>,
    agent: Agent,
    context: &OutputContext,
) -> Result<AcpRunOutcome, String> {
    let params = json!({
        "sessionId": session_id,
        "prompt": [{ "type": "text", "text": prompt }]
    });

    let id = state.next_id();
    let line = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": params
    });
    write_line(stdin, &line.to_string())
        .await
        .map_err(|e| e.to_string())?;

    let mut stop_reason = None;
    while let Some(raw) = line_rx.recv().await {
        let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if let Some(resp_id) = value.get("id").and_then(Value::as_u64)
            && resp_id == id
        {
            if let Some(err) = value.get("error") {
                return Err(err.to_string());
            }
            if let Some(result) = value.get("result") {
                stop_reason = result
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            break;
        }
        dispatch_inbound(
            &value,
            stdin,
            line_rx,
            state,
            permission_ctx,
            event_tx,
            agent,
            context,
        )
        .await?;
    }

    Ok(AcpRunOutcome {
        session_id: Some(session_id.to_owned()),
        stop_reason,
    })
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_inbound(
    value: &Value,
    stdin: &mut ChildStdin,
    _line_rx: &mut mpsc::Receiver<String>,
    _state: &Arc<SharedState>,
    permission_ctx: &PermissionContext,
    event_tx: &mpsc::Sender<AgentOutput>,
    agent: Agent,
    context: &OutputContext,
) -> Result<(), String> {
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        match method {
            "session/update" => {
                let update = value
                    .pointer("/params/update")
                    .or_else(|| value.pointer("/params"))
                    .cloned()
                    .unwrap_or(Value::Null);
                for event in map_session_update(&update) {
                    send_event(event_tx, agent, context, event).await;
                }
            }
            "session/request_permission" => {
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                let req_id = value.get("id").cloned().unwrap_or(Value::Null);
                let option = choose_permission_option(&params, permission_ctx);
                let result = if let Some(option_id) = option {
                    json!({
                        "outcome": {
                            "outcome": "selected",
                            "optionId": option_id
                        }
                    })
                } else {
                    json!({ "outcome": { "outcome": "cancelled" } })
                };
                let resp = json!({ "jsonrpc": "2.0", "id": req_id, "result": result });
                write_line(stdin, &resp.to_string())
                    .await
                    .map_err(|e| e.to_string())?;
            }
            _ => {
                eprintln!("acp: ignored notification/request method={method}");
            }
        }
    }
    Ok(())
}

async fn request(
    stdin: &mut ChildStdin,
    line_rx: &mut mpsc::Receiver<String>,
    state: &Arc<SharedState>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let id = state.next_id();
    let (tx, _rx) = oneshot::channel();
    state
        .pending
        .lock()
        .unwrap()
        .insert(id, PendingRequest { response: tx });
    let line = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    });
    write_line(stdin, &line.to_string())
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("acp: request method={method}");

    while let Some(raw) = line_rx.recv().await {
        let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(err) = value.get("error") {
                return Err(err.to_string());
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
        if value.get("method").is_some() {
            // Best-effort: ignore inbound during handshake
            continue;
        }
    }
    Err("ACP agent closed stdout".to_owned())
}

async fn write_notification(stdin: &mut ChildStdin, method: &str, params: Value) -> io::Result<()> {
    let line = json!({ "jsonrpc": "2.0", "method": method, "params": params });
    write_line(stdin, &line.to_string()).await
}

async fn write_line(stdin: &mut ChildStdin, line: &str) -> io::Result<()> {
    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}

async fn send_event(
    event_tx: &mpsc::Sender<AgentOutput>,
    agent: Agent,
    context: &OutputContext,
    event: AgentEvent,
) {
    let _ = event_tx
        .send(AgentOutput {
            agent,
            conversation_id: Some(context.conversation_id.clone()),
            turn_id: Some(context.turn_id.clone()),
            event,
        })
        .await;
}
