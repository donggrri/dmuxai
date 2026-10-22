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
use tokio::time::{sleep, timeout};

use crate::agent::{Agent, AgentEvent, AgentOutput, OutputContext, TurnPolicy};
use dmuxai::process::strip_dashboard_env;

use super::debug::acp_trace;
use super::permissions::{PermissionContext, choose_permission_option, permission_option_denies};
use super::session::{
    AgentCapabilities, SessionOpenNote, available_model_ids, cursor_acp_model_check,
    extract_reported_model, is_fast_model_label, parse_agent_capabilities, pick_non_fast_model,
    read_only_mode_request, session_load_params, session_new_params, session_resume_params,
    session_set_model_params,
};
use super::updates::{ToolCallCache, map_session_update};

const CLIENT_NAME: &str = "dmuxai";
const CLIENT_VERSION: &str = "0.2.2";
const PROTOCOL_VERSION: i64 = 1;
const STDERR_TAIL: usize = 8;
const CANCEL_GRACE_MS: u64 = 1500;
pub const PERMISSION_DENIED_NOTICE: &str =
    "dmuxai denied an ACP tool permission (outside workspace or read-only turn); turn cancelled.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpRunnerKind {
    Cursor,
    Grok,
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
    /// ACP `mcpServers` array passed on session/new|load|resume (FR-1414).
    pub mcp_servers: Value,
}

pub struct AcpRunOutcome {
    pub session_id: Option<String>,
    pub stop_reason: Option<String>,
    #[allow(dead_code)]
    pub effective_model: Option<String>,
}

impl AcpRunOutcome {
    /// FR-1416: a turn the agent ended with `cancelled` (user cancel or denied permission).
    pub fn cancelled(&self) -> bool {
        self.stop_reason.as_deref() == Some("cancelled")
    }
}

struct SharedState {
    next_id: Mutex<u64>,
    session_id: Mutex<Option<String>>,
    permission_denied: AtomicBool,
    saw_agent_text: AtomicBool,
    cancel_sent: AtomicBool,
    text_break_before_next_delta: AtomicBool,
    tool_calls: Mutex<ToolCallCache>,
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
        permission_denied: AtomicBool::new(false),
        saw_agent_text: AtomicBool::new(false),
        cancel_sent: AtomicBool::new(false),
        text_break_before_next_delta: AtomicBool::new(false),
        tool_calls: Mutex::new(ToolCallCache::default()),
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

    let runner_label = match config.runner {
        AcpRunnerKind::Cursor => "cursor",
        AcpRunnerKind::Grok => "grok",
    };

    let turn_result: Result<AcpRunOutcome, String> = async {
        let open = open_session(
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
        .await?;

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

        if config.policy == TurnPolicy::ReadOnly {
            // FR-1413: modes → set_mode, configOptions → set_config_option, else rely on
            // the client-side read-only permission deny (read_only_fail_closed).
            match read_only_mode_request(&open.session_payload, &session_id) {
                Some((method, params)) => {
                    match request(&mut stdin, &mut line_rx, &state, method, params, &config.cancel)
                        .await
                    {
                        Ok(_) => acp_trace(format!("{method} read-only ok")),
                        Err(error) => acp_trace(format!(
                            "{method} read-only failed ({runner_label}): {error}; permissions stay fail-closed"
                        )),
                    }
                }
                None => acp_trace(format!(
                    "{runner_label}: no ACP read-only mode; permissions stay fail-closed"
                )),
            }
        }

        if let Some(model) = config
            .session_config_model
            .as_deref()
            .filter(|m| !m.is_empty())
        {
            match request(
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
            .await
            {
                Ok(_) => {
                    acp_trace(format!("session/set_config_option model={model} ok"));
                }
                Err(error) => {
                    acp_trace(format!("session/set_config_option failed: {error}"));
                }
            }
        }

        run_prompt(
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
        .await
        .map(|outcome| AcpRunOutcome {
            session_id: outcome.session_id,
            stop_reason: outcome.stop_reason,
            effective_model: open.effective_model.clone(),
        })
    }
    .await;

    let session_id_for_cancel = state.session_id.lock().unwrap().clone();
    if config.cancel.load(Ordering::SeqCst) {
        send_session_cancel_once(&state, &mut stdin, session_id_for_cancel.as_deref()).await;
        let _ = timeout(
            Duration::from_millis(CANCEL_GRACE_MS),
            wait_child(&mut child, child_pid),
        )
        .await;
    }

    shutdown_acp_child(&mut child, child_pid).await;
    reader.abort();
    stderr_reader.abort();
    let status = child.wait().await?;
    let diagnostic = stderr_tail
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");

    let prompt_result = turn_result;

    if config.cancel.load(Ordering::SeqCst) || state.permission_denied.load(Ordering::SeqCst) {
        // FR-1416: user cancel or a denied permission ends the turn as cancelled.
        if config.lifecycle == AcpLifecycle::Managed {
            if state.permission_denied.load(Ordering::SeqCst) {
                send_event(
                    event_tx,
                    agent,
                    context,
                    AgentEvent::Notice(PERMISSION_DENIED_NOTICE.to_owned()),
                )
                .await;
            }
            send_event(event_tx, agent, context, AgentEvent::Cancelled).await;
        }
        let session_id = state.session_id.lock().unwrap().clone();
        return Ok(AcpRunOutcome {
            session_id,
            stop_reason: Some("cancelled".to_owned()),
            effective_model: None,
        });
    }

    match prompt_result {
        Ok(outcome) => {
            if outcome.cancelled() {
                if config.lifecycle == AcpLifecycle::Managed {
                    send_event(event_tx, agent, context, AgentEvent::Cancelled).await;
                }
                return Ok(outcome);
            }
            if status.success() || outcome.stop_reason.is_some() {
                if config.lifecycle == AcpLifecycle::Managed {
                    send_event(event_tx, agent, context, AgentEvent::Completed).await;
                }
                Ok(outcome)
            } else {
                let msg = format_acp_exit(status, &diagnostic);
                if config.lifecycle == AcpLifecycle::Managed {
                    send_event(event_tx, agent, context, AgentEvent::Error(msg.clone())).await;
                }
                Err(io::Error::other(msg))
            }
        }
        Err(error) => {
            let msg = if diagnostic.is_empty() || diagnostic.contains(&error) {
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
    session_payload: Value,
}

#[cfg_attr(not(unix), allow(unused_variables))]
async fn shutdown_acp_child(child: &mut tokio::process::Child, child_pid: Option<u32>) {
    let _ = child.start_kill();
    #[cfg(unix)]
    if let Some(pid) = child_pid {
        terminate_group(pid).await;
    }
}

async fn send_session_cancel_once(
    state: &Arc<SharedState>,
    stdin: &mut ChildStdin,
    session_id: Option<&str>,
) {
    if state.cancel_sent.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(session_id) = session_id {
        let _ =
            write_notification(stdin, "session/cancel", json!({ "sessionId": session_id })).await;
    }
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
    let mut resume_warnings: Vec<String> = Vec::new();

    if let Some(id) = resume_session.filter(|s| !s.is_empty()) {
        if caps.load_session {
            match request(
                stdin,
                line_rx,
                state,
                "session/load",
                session_load_params(id, cwd, &config.mcp_servers),
                &config.cancel,
            )
            .await
            {
                Ok(loaded) => {
                    let sid = loaded
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_owned();
                    return finish_open(
                        config,
                        caps,
                        stdin,
                        line_rx,
                        state,
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
                Err(error) => resume_warnings.push(format!("session/load failed: {error}")),
            }
        }
        if caps.resume_session {
            match request(
                stdin,
                line_rx,
                state,
                "session/resume",
                session_resume_params(id, cwd, &config.mcp_servers),
                &config.cancel,
            )
            .await
            {
                Ok(resumed) => {
                    let sid = resumed
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_owned();
                    return finish_open(
                        config,
                        caps,
                        stdin,
                        line_rx,
                        state,
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
                Err(error) => resume_warnings.push(format!("session/resume failed: {error}")),
            }
        }
    }

    let created = request(
        stdin,
        line_rx,
        state,
        "session/new",
        session_new_params(cwd, &config.mcp_servers),
        &config.cancel,
    )
    .await?;
    let session_id = created
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "session/new missing sessionId".to_owned())?;
    let mut notice = resume_session.filter(|s| !s.is_empty()).map(|_| {
        "Previous provider session was not found; started a new ACP session (transcript may be missing)."
            .to_owned()
    });
    if !resume_warnings.is_empty() {
        let detail = resume_warnings.join("; ");
        notice = Some(match notice {
            Some(existing) => format!("{existing} ({detail})"),
            None => format!("Could not resume prior session ({detail}); started a new session."),
        });
    }
    let note = if notice.is_some() {
        SessionOpenNote::NewAfterResumeFailed
    } else {
        SessionOpenNote::New
    };
    finish_open(
        config, caps, stdin, line_rx, state, session_id, created, note, notice, event_tx, agent,
        context,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finish_open(
    config: &AcpTurnConfig,
    caps: AgentCapabilities,
    stdin: &mut ChildStdin,
    line_rx: &mut mpsc::Receiver<String>,
    state: &Arc<SharedState>,
    session_id: String,
    session_payload: Value,
    _note: SessionOpenNote,
    notice: Option<String>,
    _event_tx: &mpsc::Sender<AgentOutput>,
    _agent: Agent,
    _context: &OutputContext,
) -> Result<OpenSessionOutcome, String> {
    let effective_model = if config.runner == AcpRunnerKind::Cursor {
        Some(
            ensure_cursor_session_model(
                stdin,
                line_rx,
                state,
                &config.cancel,
                &caps,
                &session_id,
                &session_payload,
                config.requested_model.as_deref(),
            )
            .await?,
        )
    } else {
        config
            .requested_model
            .clone()
            .or_else(|| extract_reported_model(&session_payload))
    };
    Ok(OpenSessionOutcome {
        session_id,
        effective_model,
        notice,
        session_payload,
    })
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
    loop {
        let raw = tokio::select! {
            _ = wait_for_cancel(cancel.clone()) => {
                send_session_cancel_once(state, stdin, Some(session_id)).await;
                return Err("cancelled".to_owned());
            }
            line = line_rx.recv() => line,
        };
        let Some(raw) = raw else {
            break;
        };
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
        dispatch_inbound(
            &value,
            stdin,
            permission_ctx,
            event_tx,
            agent,
            context,
            state,
        )
        .await?;
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
    state: &Arc<SharedState>,
) -> Result<(), String> {
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        match method {
            "session/update" => {
                let update = value
                    .pointer("/params/update")
                    .or_else(|| value.pointer("/params"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let kind = update
                    .get("sessionUpdate")
                    .or_else(|| update.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if (kind == "tool_call" || kind == "tool_call_update")
                    && state.saw_agent_text.load(Ordering::SeqCst)
                {
                    state
                        .text_break_before_next_delta
                        .store(true, Ordering::SeqCst);
                }
                let events = {
                    let mut tool_cache = state.tool_calls.lock().unwrap();
                    map_session_update(&update, &mut tool_cache)
                };
                for event in events {
                    let event = match event {
                        AgentEvent::TextDelta(delta) => {
                            state.saw_agent_text.store(true, Ordering::SeqCst);
                            let delta = if state
                                .text_break_before_next_delta
                                .swap(false, Ordering::SeqCst)
                            {
                                format!("\n\n{delta}")
                            } else {
                                delta
                            };
                            AgentEvent::TextDelta(delta)
                        }
                        other => other,
                    };
                    send_event(event_tx, agent, context, event).await;
                }
            }
            "session/request_permission" => {
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                let req_id = value.get("id").cloned().unwrap_or(Value::Null);
                let option = choose_permission_option(&params, permission_ctx);
                let denied = option.is_none()
                    || option
                        .as_ref()
                        .is_some_and(|id| permission_option_denies(&params, id));
                if denied {
                    state.permission_denied.store(true, Ordering::SeqCst);
                }
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

    let deadline = Duration::from_secs(120);
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        let raw = tokio::select! {
            _ = wait_for_cancel(cancel.clone()) => {
                let sid = state.session_id.lock().unwrap().clone();
                send_session_cancel_once(state, stdin, sid.as_deref()).await;
                return Err("cancelled".to_owned());
            }
            line = line_rx.recv() => line,
        };
        let Some(raw) = raw else {
            break;
        };
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
            let result = value.get("result").cloned().unwrap_or(Value::Null);
            if should_trace_response(method) {
                acp_trace(format!(
                    "response method={method} id={id} {}",
                    acp_result_trace_summary(method, &result)
                ));
            }
            return Ok(result);
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

pub fn new_cancel_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

async fn wait_for_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::SeqCst) {
        sleep(Duration::from_millis(50)).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn ensure_cursor_session_model(
    stdin: &mut ChildStdin,
    line_rx: &mut mpsc::Receiver<String>,
    state: &Arc<SharedState>,
    cancel: &Arc<AtomicBool>,
    caps: &AgentCapabilities,
    session_id: &str,
    payload: &Value,
    requested: Option<&str>,
) -> Result<String, String> {
    let reported = extract_reported_model(payload);
    if let Some(id) = reported.as_deref().filter(|m| !is_fast_model_label(m)) {
        return Ok(id.to_owned());
    }
    let candidates = available_model_ids(payload);
    let target = pick_non_fast_model(&candidates, requested);
    if let Some(model_id) = target {
        if caps.set_model
            && let Ok(updated) = request(
                stdin,
                line_rx,
                state,
                "session/set_model",
                session_set_model_params(session_id, &model_id),
                cancel,
            )
            .await
        {
            let effective = extract_reported_model(&updated).unwrap_or_else(|| model_id.clone());
            if !is_fast_model_label(&effective) {
                return Ok(effective);
            }
        }
        if caps.set_config_option
            && let Ok(updated) = request(
                stdin,
                line_rx,
                state,
                "session/set_config_option",
                json!({
                    "sessionId": session_id,
                    "configId": "model",
                    "value": model_id
                }),
                cancel,
            )
            .await
        {
            let effective = extract_reported_model(&updated).unwrap_or_else(|| model_id.clone());
            if !is_fast_model_label(&effective) {
                return Ok(effective);
            }
        }
    }
    cursor_acp_model_check(payload, requested)
}

fn should_trace_response(method: &str) -> bool {
    matches!(
        method,
        "session/new"
            | "session/load"
            | "session/resume"
            | "session/set_mode"
            | "session/set_model"
            | "session/set_config_option"
    )
}

fn acp_result_trace_summary(method: &str, result: &Value) -> String {
    if !matches!(
        method,
        "session/new"
            | "session/load"
            | "session/resume"
            | "session/set_mode"
            | "session/set_model"
            | "session/set_config_option"
    ) {
        return format!("result={}", compact_json(result));
    }
    let mode_ids = super::session::available_mode_ids(result);
    let model_ids = super::session::available_model_ids(result);
    let current_mode = result
        .pointer("/modes/currentModeId")
        .and_then(Value::as_str);
    let current_model = super::session::extract_reported_model(result);
    let config_model = super::session::config_option_current(result, "model");
    format!(
        "modes.currentModeId={current_mode:?} modes.availableModes={mode_ids:?} \
models.currentModelId={current_model:?} models.availableModels={model_ids:?} \
configOptions.model.currentValue={config_model:?}"
    )
}

fn compact_json(value: &Value) -> String {
    let raw = value.to_string();
    if raw.len() > 256 {
        format!("{}…", &raw[..256])
    } else {
        raw
    }
}
