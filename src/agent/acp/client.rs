//! ACP v1 stdio client (Linux-supported path per SPEC-acp-runners).

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::agent::{Agent, AgentEvent, AgentOutput, OutputContext, TurnPolicy};
use dmuxai::process::strip_dashboard_env;

use super::debug::acp_trace;
use super::permissions::{PermissionContext, choose_permission_option};
use super::session::{
    SessionOpenNote, cursor_acp_model_check, parse_agent_capabilities, read_only_acp_mode,
    session_set_mode_params,
};
use super::updates::map_session_update;

const CLIENT_NAME: &str = "dmuxai";
const CLIENT_VERSION: &str = "0.2.2";
const PROTOCOL_VERSION: i64 = 1;
const STDERR_TAIL: usize = 8;
const CANCEL_GRACE_MS: u64 = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpRunnerKind {
    Cursor,
    Grok,
    #[allow(dead_code)]
    Opencode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpLifecycle {
    /// Emit Started/Completed/Cancelled/Error (Cursor direct).
    Managed,
    /// Caller owns terminal lifecycle events (custom adapter).
    Delegate,
}

#[derive(Clone)]
pub struct AcpTurnConfig {
    pub runner: AcpRunnerKind,
    pub policy: TurnPolicy,
    pub lifecycle: AcpLifecycle,
    pub requested_model: Option<String>,
    pub session_config_model: Option<String>,
    #[allow(dead_code)]
    pub reasoning_effort: Option<String>,
    pub cancel: Arc<AtomicBool>,
}

pub struct AcpRunOutcome {
    pub session_id: Option<String>,
    pub stop_reason: Option<String>,
    #[allow(dead_code)]
    pub effective_model: Option<String>,
}

struct SharedState {
    next_id: Mutex<u64>,
    session_id: Mutex<Option<String>>,
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
    config: AcpTurnConfig,
) -> io::Result<AcpRunOutcome> {
    let mut command = Command::new(program);
    command.args(args);
    command.current_dir(cwd);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    strip_dashboard_env(command.as_std_mut());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }

    let mut child = command.spawn()?;
    let child_pid = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("acp stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("acp stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("acp stderr"))?;

    let state = Arc::new(SharedState {
        next_id: Mutex::new(0),
        session_id: Mutex::new(None),
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

    let stderr_tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let stderr_tail_reader = stderr_tail.clone();
    let stderr_reader = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut tail = stderr_tail_reader.lock().unwrap();
            tail.push_back(line);
            while tail.len() > STDERR_TAIL {
                tail.pop_front();
            }
        }
    });

    let mut stdin = stdin;
    if config.lifecycle == AcpLifecycle::Managed {
        send_event(event_tx, agent, context, AgentEvent::Started).await;
    }

    let open = open_session(
        // FR-1316 session resume/load
        &mut stdin,
        &mut line_rx,
        &state,
        cwd,
        resume_session,
        &config,
        event_tx,
        agent,
        context,
    )
    .await
    .map_err(io::Error::other)?;

    let session_id = open.session_id;
    *state.session_id.lock().unwrap() = Some(session_id.clone());

    if let Some(model) = &open.effective_model {
        send_event(
            event_tx,
            agent,
            context,
            AgentEvent::ModelSelected(model.clone()),
        )
        .await;
    }
    if let Some(note) = open.notice {
        send_event(event_tx, agent, context, AgentEvent::Notice(note)).await;
    }

    send_event(
        event_tx,
        agent,
        context,
        AgentEvent::ProviderSessionBound(session_id.clone()),
    )
    .await;

    apply_read_only_mode(
        &mut stdin,
        &mut line_rx,
        &state,
        &session_id,
        config.policy,
        config.runner,
    )
    .await
    .map_err(io::Error::other)?;

    if let Some(model) = config
        .session_config_model
        .as_deref()
        .filter(|m| !m.is_empty())
    {
        let _ = request(
            &mut stdin,
            &mut line_rx,
            &state,
            "session/set_config_option",
            json!({
                "sessionId": session_id,
                "configId": "model",
                "value": model
            }),
            &config.cancel,
        )
        .await;
        acp_trace(format!("session/set_config_option model={model}"));
    }

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
        &config.cancel,
    )
    .await;

    if config.cancel.load(Ordering::SeqCst) {
        let _ = write_notification(
            &mut stdin,
            "session/cancel",
            json!({ "sessionId": session_id }),
        )
        .await;
        let _ = timeout(
            Duration::from_millis(CANCEL_GRACE_MS),
            wait_child(&mut child, child_pid),
        )
        .await;
    }

    let _ = child.start_kill();
    #[cfg(unix)]
    if let Some(pid) = child_pid {
        terminate_group(pid).await;
    }
    let status = child.wait().await?;
    reader.abort();
    stderr_reader.abort();
    let diagnostic = stderr_tail
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");

    match prompt_result {
        Ok(outcome) => {
            if config.cancel.load(Ordering::SeqCst) {
                if config.lifecycle == AcpLifecycle::Managed {
                    send_event(event_tx, agent, context, AgentEvent::Cancelled).await;
                }
                return Ok(outcome);
            }
            if status.success() || outcome.stop_reason.is_some() {
                if config.lifecycle == AcpLifecycle::Managed {
                    send_event(event_tx, agent, context, AgentEvent::Completed).await;
                }
                Ok(AcpRunOutcome {
                    session_id: outcome.session_id,
                    stop_reason: outcome.stop_reason,
                    effective_model: open.effective_model.clone(),
                })
            } else {
                let msg = format_acp_exit(status, &diagnostic);
                if config.lifecycle == AcpLifecycle::Managed {
                    send_event(event_tx, agent, context, AgentEvent::Error(msg.clone())).await;
                }
                Err(io::Error::other(msg))
            }
        }
        Err(error) => {
            let msg = if diagnostic.is_empty() {
                error
            } else {
                format!("{error} ({diagnostic})")
            };
            if config.lifecycle == AcpLifecycle::Managed {
                send_event(event_tx, agent, context, AgentEvent::Error(msg.clone())).await;
            }
            Err(io::Error::other(msg))
        }
    }
}

struct OpenSessionOutcome {
    session_id: String,
    effective_model: Option<String>,
    notice: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn open_session(
    stdin: &mut ChildStdin,
    line_rx: &mut mpsc::Receiver<String>,
    state: &Arc<SharedState>,
    cwd: &Path,
    resume_session: Option<&str>,
    config: &AcpTurnConfig,
    event_tx: &mpsc::Sender<AgentOutput>,
    agent: Agent,
    context: &OutputContext,
) -> Result<OpenSessionOutcome, String> {
    let init_params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {},
        "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION }
    });
    let init = request(
        stdin,
        line_rx,
        state,
        "initialize",
        init_params,
        &config.cancel,
    )
    .await?;
    let caps = parse_agent_capabilities(&init);

    if let Some(id) = resume_session.filter(|s| !s.is_empty()) {
        if caps.load_session {
            if let Ok(loaded) = request(
                stdin,
                line_rx,
                state,
                "session/load",
                json!({ "sessionId": id, "cwd": cwd.to_string_lossy() }),
                &config.cancel,
            )
            .await
            {
                let sid = loaded
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_owned();
                return finish_open(
                    config,
                    sid,
                    loaded,
                    SessionOpenNote::Loaded,
                    None,
                    event_tx,
                    agent,
                    context,
                )
                .await;
            }
        }
        if caps.resume_session {
            if let Ok(resumed) = request(
                stdin,
                line_rx,
                state,
                "session/resume",
                json!({ "sessionId": id }),
                &config.cancel,
            )
            .await
            {
                let sid = resumed
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_owned();
                return finish_open(
                    config,
                    sid,
                    resumed,
                    SessionOpenNote::Resumed,
                    None,
                    event_tx,
                    agent,
                    context,
                )
                .await;
            }
        }
    }

    let created = request(
        stdin,
        line_rx,
        state,
        "session/new",
        json!({
            "cwd": cwd.to_string_lossy(),
            "mcpServers": []
        }),
        &config.cancel,
    )
    .await?;
    let session_id = created
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "session/new missing sessionId".to_owned())?;
    let notice = resume_session.filter(|s| !s.is_empty()).map(|_| {
        "Previous provider session was not found; started a new ACP session (transcript may be missing)."
            .to_owned()
    });
    let note = if notice.is_some() {
        SessionOpenNote::NewAfterResumeFailed
    } else {
        SessionOpenNote::New
    };
    finish_open(
        config, session_id, created, note, notice, event_tx, agent, context,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finish_open(
    config: &AcpTurnConfig,
    session_id: String,
    session_payload: Value,
    _note: SessionOpenNote,
    notice: Option<String>,
    _event_tx: &mpsc::Sender<AgentOutput>,
    _agent: Agent,
    _context: &OutputContext,
) -> Result<OpenSessionOutcome, String> {
    let effective_model = if config.runner == AcpRunnerKind::Cursor {
        Some(cursor_acp_model_check(
            &session_payload,
            config.requested_model.as_deref(),
        )?)
    } else {
        config.requested_model.clone().or_else(|| {
            session_payload
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    };
    Ok(OpenSessionOutcome {
        session_id,
        effective_model,
        notice,
    })
}

async fn apply_read_only_mode(
    stdin: &mut ChildStdin,
    line_rx: &mut mpsc::Receiver<String>,
    state: &Arc<SharedState>,
    session_id: &str,
    policy: TurnPolicy,
    runner: AcpRunnerKind,
) -> Result<(), String> {
    let Some(mode) = read_only_acp_mode(policy) else {
        return Ok(());
    };
    let method = match runner {
        AcpRunnerKind::Cursor | AcpRunnerKind::Grok | AcpRunnerKind::Opencode => "session/set_mode",
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let _ = request(
        stdin,
        line_rx,
        state,
        method,
        session_set_mode_params(session_id, mode),
        &cancel,
    )
    .await;
    Ok(())
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
    cancel: &Arc<AtomicBool>,
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
    while let Ok(Some(raw)) = timeout(Duration::from_secs(3600), line_rx.recv()).await {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".to_owned());
        }
        let value: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(error) => {
                acp_trace(format!("skip malformed line: {error}"));
                continue;
            }
        };
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
        dispatch_inbound(&value, stdin, permission_ctx, event_tx, agent, context).await?;
    }

    Ok(AcpRunOutcome {
        session_id: Some(session_id.to_owned()),
        stop_reason,
        effective_model: None,
    })
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_inbound(
    value: &Value,
    stdin: &mut ChildStdin,
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
                if value.get("id").is_some() {
                    let req_id = value.get("id").cloned().unwrap_or(Value::Null);
                    let resp = json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "error": { "code": -32601, "message": "Method not found" }
                    });
                    write_line(stdin, &resp.to_string())
                        .await
                        .map_err(|e| e.to_string())?;
                } else {
                    acp_trace(format!("ignored notification method={method}"));
                }
            }
        }
        return Ok(());
    }

    Ok(())
}

async fn request(
    stdin: &mut ChildStdin,
    line_rx: &mut mpsc::Receiver<String>,
    state: &Arc<SharedState>,
    method: &str,
    params: Value,
    cancel: &Arc<AtomicBool>,
) -> Result<Value, String> {
    let id = state.next_id();
    let line = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    });
    write_line(stdin, &line.to_string())
        .await
        .map_err(|e| e.to_string())?;
    acp_trace(format!("request method={method} id={id}"));

    while let Ok(Some(raw)) = timeout(Duration::from_secs(120), line_rx.recv()).await {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".to_owned());
        }
        let value: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(error) => {
                acp_trace(format!("skip malformed line: {error}"));
                continue;
            }
        };
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(err) = value.get("error") {
                return Err(err.to_string());
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
        if value.get("method").is_some() {
            // Handled opportunistically during handshake; permission/update not expected here.
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

fn format_acp_exit(status: std::process::ExitStatus, diagnostic: &str) -> String {
    if diagnostic.is_empty() {
        format!("ACP agent exited with {status}")
    } else {
        format!("ACP agent exited with {status}: {diagnostic}")
    }
}

async fn wait_child(child: &mut tokio::process::Child, _pid: Option<u32>) {
    let _ = child.wait().await;
}

#[cfg(unix)]
async fn terminate_group(pgid: u32) {
    let _ = signal_group(pgid, "-TERM");
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        if !signal_group(pgid, "-0") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let _ = signal_group(pgid, "-KILL");
}

#[cfg(unix)]
fn signal_group(pgid: u32, signal: &str) -> bool {
    std::process::Command::new("kill")
        .args([signal, "--", &format!("-{pgid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
async fn terminate_group(_pid: u32) {}

pub fn new_cancel_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
