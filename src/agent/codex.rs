use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, Instant, timeout_at},
};

use dmuxai::attachments::prompt_with_image_references;

use super::{
    AdapterError, Agent, AgentAdapter, AgentEvent, AgentOutput, AgentRequest, ConversationId,
    ModelInfo, OutputContext, SpawnedAdapter, TurnPolicy,
};
use dmuxai::process::{headless_command, resolve_program};

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const RAW_MESSAGE_CHANNEL_CAPACITY: usize = 64;
const CODEX_BINARY_OVERRIDE: &str = "DMUXAI_CODEX_BIN";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const TOOL_SUMMARY_LIMIT: usize = 240;

enum CodexCommand {
    Send(AgentRequest),
    Cancel,
    ListModels,
    SelectModel(String),
    SelectEffort(String),
    RequestStatus,
}

pub struct CodexAdapter {
    command_tx: mpsc::Sender<CodexCommand>,
}

impl AgentAdapter for CodexAdapter {
    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn send(&self, request: AgentRequest) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::Send(request))
            .map_err(map_send_error)
    }

    fn cancel(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::Cancel)
            .map_err(map_send_error)
    }

    fn request_models(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::ListModels)
            .map_err(map_send_error)
    }

    fn select_model(&self, model: String) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::SelectModel(model))
            .map_err(map_send_error)
    }

    fn select_effort(&self, effort: String) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::SelectEffort(effort))
            .map_err(map_send_error)
    }

    fn request_status(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::RequestStatus)
            .map_err(map_send_error)
    }
}

fn map_send_error(error: mpsc::error::TrySendError<CodexCommand>) -> AdapterError {
    match error {
        mpsc::error::TrySendError::Full(_) => AdapterError::CommandQueueFull,
        mpsc::error::TrySendError::Closed(_) => AdapterError::RuntimeStopped,
    }
}

pub fn spawn_codex_adapter(
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
) -> SpawnedAdapter {
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let worker = tokio::spawn(runtime_loop(command_rx, event_tx, cwd, mcp_servers));

    (Box::new(CodexAdapter { command_tx }), worker)
}

#[cfg(test)]
pub(crate) fn build_codex_app_server_args(
    servers: &[dmuxai::mcp::ResolvedMcpServer],
) -> Vec<String> {
    let (args, _, _) = build_codex_app_server_args_and_env(servers);
    args
}

pub(crate) fn build_codex_app_server_args_and_env(
    servers: &[dmuxai::mcp::ResolvedMcpServer],
) -> (
    Vec<String>,
    std::collections::BTreeMap<String, String>,
    Vec<String>,
) {
    let applicable: Vec<dmuxai::mcp::ResolvedMcpServer> = servers
        .iter()
        .filter(|s| s.applies_to("codex"))
        .cloned()
        .collect();
    let (mut args, env, notices) =
        dmuxai::mcp::propagate::codex_config_overrides_and_env(&applicable);
    args.push("app-server".to_owned());
    args.push("--stdio".to_owned());
    (args, env, notices)
}

struct CodexClient {
    child: Child,
    stdin: ChildStdin,
    raw_rx: mpsc::Receiver<RawMessage>,
    readers: Vec<JoinHandle<()>>,
    thread_id: Option<String>,
    conversation_id: Option<ConversationId>,
    output_context: Option<OutputContext>,
    active_turn_id: Option<String>,
    pending_turn_request_id: Option<u64>,
    turn_ready_for_interrupt: bool,
    interrupt_sent: bool,
    cancel_requested: bool,
    next_request_id: u64,
    active_mode: TurnPolicy,
    implement_write_observed: bool,
}

enum RawMessage {
    Json(Value),
    Stderr(String),
    StdoutClosed,
}

impl CodexClient {
    async fn start(
        cwd: &Path,
        event_tx: &mpsc::Sender<AgentOutput>,
        mcp_servers: &[dmuxai::mcp::ResolvedMcpServer],
    ) -> io::Result<Self> {
        let program = resolve_program(CODEX_BINARY_OVERRIDE, "codex.exe", "codex");
        let mut command = headless_command(program);
        let (args, env, notices) = build_codex_app_server_args_and_env(mcp_servers);
        for notice in notices {
            send_event(event_tx, None, AgentEvent::Notice(notice)).await;
        }
        command
            .args(&args)
            .envs(&env)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Codex stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Codex stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("Codex stderr was not piped"))?;

        let (raw_tx, raw_rx) = mpsc::channel(RAW_MESSAGE_CHANNEL_CAPACITY);
        let readers = vec![
            tokio::spawn(read_stdout(stdout, raw_tx.clone())),
            tokio::spawn(read_stderr(stderr, raw_tx)),
        ];
        let mut client = Self {
            child,
            stdin,
            raw_rx,
            readers,
            thread_id: None,
            conversation_id: None,
            output_context: None,
            active_turn_id: None,
            pending_turn_request_id: None,
            turn_ready_for_interrupt: false,
            interrupt_sent: false,
            cancel_requested: false,
            next_request_id: 1,
            active_mode: TurnPolicy::default(),
            implement_write_observed: false,
        };

        let initialize_id = client.next_id();
        client
            .write_message(json!({
                "method": "initialize",
                "id": initialize_id,
                "params": {
                    "clientInfo": {
                        "name": "dmuxai",
                        "title": "DmuxAI",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }))
            .await?;
        client
            .wait_for_success_response(initialize_id, event_tx)
            .await?;
        client
            .write_message(json!({ "method": "initialized", "params": {} }))
            .await?;

        Ok(client)
    }

    async fn open_thread(
        &mut self,
        cwd: &Path,
        request: &AgentRequest,
        event_tx: &mpsc::Sender<AgentOutput>,
    ) -> io::Result<Option<String>> {
        if thread_can_be_reused(
            self.conversation_id.as_ref(),
            self.thread_id.as_deref(),
            &request.conversation_id,
            request.resume_session.as_deref(),
        ) {
            return Ok(None);
        }

        let request_id = self.next_id();
        self.write_message(thread_open_request(
            request_id,
            cwd,
            request.resume_session.as_deref(),
        ))
        .await?;
        let response = self.wait_for_success_response(request_id, event_tx).await?;
        let thread_id = extract_thread_id(&response)?;
        self.thread_id = Some(thread_id.clone());
        self.conversation_id = Some(request.conversation_id.clone());
        Ok(Some(thread_id))
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    async fn start_turn(
        &mut self,
        request: &AgentRequest,
        model: Option<&str>,
        effort: Option<&str>,
        mode: TurnPolicy,
    ) -> io::Result<()> {
        let thread_id = self
            .thread_id
            .clone()
            .ok_or_else(|| io::Error::other("Codex thread was not opened"))?;
        let request_id = self.next_id();
        self.pending_turn_request_id = Some(request_id);
        self.output_context = Some(OutputContext::from(request));
        self.turn_ready_for_interrupt = false;
        self.interrupt_sent = false;
        self.cancel_requested = false;
        self.active_mode = mode;
        self.implement_write_observed = false;
        self.write_message(turn_start_request(
            request_id, &thread_id, request, model, effort, mode,
        ))
        .await
    }

    async fn list_models(
        &mut self,
        event_tx: &mpsc::Sender<AgentOutput>,
    ) -> io::Result<(Vec<ModelInfo>, Option<String>)> {
        let request_id = self.next_id();
        self.write_message(json!({
            "method": "model/list",
            "id": request_id,
            "params": { "includeHidden": false }
        }))
        .await?;
        let response = self.wait_for_success_response(request_id, event_tx).await?;
        parse_model_list(&response)
    }

    async fn status(&mut self, event_tx: &mpsc::Sender<AgentOutput>) -> String {
        let rate_limits = self
            .status_response("account/rateLimits/read", event_tx)
            .await
            .map(|response| format_rate_limits(&response))
            .unwrap_or_else(|error| format!("Rate limits: unavailable ({error})"));
        let usage = self
            .status_response("account/usage/read", event_tx)
            .await
            .map(|response| format_usage(&response))
            .unwrap_or_else(|error| format!("Token activity: unavailable ({error})"));
        format!("Codex status\n{usage}\n{rate_limits}")
    }

    async fn status_response(
        &mut self,
        method: &str,
        event_tx: &mpsc::Sender<AgentOutput>,
    ) -> io::Result<Value> {
        let request_id = self.next_id();
        self.write_message(json!({ "method": method, "id": request_id, "params": {} }))
            .await?;
        self.wait_for_success_response(request_id, event_tx).await
    }

    async fn request_cancel(&mut self) -> io::Result<()> {
        if self.active_turn_id.is_some() || self.pending_turn_request_id.is_some() {
            self.cancel_requested = true;
            self.send_interrupt_if_ready().await?;
        }
        Ok(())
    }

    async fn send_interrupt_if_ready(&mut self) -> io::Result<()> {
        if self.turn_ready_for_interrupt
            && !self.interrupt_sent
            && let Some(turn_id) = self.active_turn_id.clone()
            && let Some(thread_id) = self.thread_id.clone()
        {
            let request_id = self.next_id();
            self.write_message(json!({
                "method": "turn/interrupt",
                "id": request_id,
                "params": {
                    "threadId": thread_id,
                    "turnId": turn_id
                }
            }))
            .await?;
            self.interrupt_sent = true;
        }
        Ok(())
    }

    async fn write_message(&mut self, message: Value) -> io::Result<()> {
        let mut encoded = serde_json::to_vec(&message).map_err(io::Error::other)?;
        encoded.push(b'\n');
        self.stdin.write_all(&encoded).await?;
        self.stdin.flush().await
    }

    async fn wait_for_success_response(
        &mut self,
        request_id: u64,
        event_tx: &mpsc::Sender<AgentOutput>,
    ) -> io::Result<Value> {
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let mut last_stderr = None;
        loop {
            let raw = match timeout_at(deadline, self.raw_rx.recv()).await {
                Ok(Some(raw)) => raw,
                Ok(None) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Codex app-server message channel closed during startup",
                    ));
                }
                Err(_) => {
                    let diagnostic = last_stderr
                        .map(|line: String| format!("; last stderr: {line}"))
                        .unwrap_or_default();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out waiting for Codex response id {request_id}{diagnostic}"),
                    ));
                }
            };

            match raw {
                RawMessage::Json(message) => {
                    if response_id(&message) == Some(request_id) {
                        if let Some(error) = response_error(&message) {
                            return Err(io::Error::other(error));
                        }
                        return Ok(message);
                    }
                    forward_notification(message, event_tx).await;
                }
                RawMessage::Stderr(line) => {
                    if !line.trim().is_empty() {
                        last_stderr = Some(line);
                    }
                }
                RawMessage::StdoutClosed => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Codex app-server closed stdout during startup",
                    ));
                }
            }
        }
    }

    async fn shutdown(mut self) {
        drop(self.stdin);
        if !matches!(
            tokio::time::timeout(Duration::from_secs(1), self.child.wait()).await,
            Ok(Ok(_))
        ) {
            let _ = self.child.kill().await;
        }
        for reader in self.readers.drain(..) {
            reader.abort();
            let _ = reader.await;
        }
    }
}

fn thread_open_request(request_id: u64, cwd: &Path, resume_session: Option<&str>) -> Value {
    let mut params = json!({
        "cwd": cwd.to_string_lossy(),
        "approvalPolicy": "never",
        "sandbox": "read-only"
    });
    let method = if let Some(thread_id) = resume_session {
        params["threadId"] = Value::String(thread_id.to_owned());
        "thread/resume"
    } else {
        "thread/start"
    };
    json!({
        "method": method,
        "id": request_id,
        "params": params
    })
}

fn turn_start_request(
    request_id: u64,
    thread_id: &str,
    request: &AgentRequest,
    model: Option<&str>,
    effort: Option<&str>,
    mode: TurnPolicy,
) -> Value {
    let mut params = json!({
        "threadId": thread_id,
        "input": [{ "type": "text", "text": prompt_with_image_references(
            &codex_prompt(mode, &request.prompt),
            &request.image_paths,
        ) }],
        "approvalPolicy": "never",
        "sandboxPolicy": {
            "type": match mode {
                TurnPolicy::ReadOnly => "readOnly",
                TurnPolicy::Agent => "workspaceWrite",
            }
        }
    });
    if let Some(model) = model {
        params["model"] = Value::String(model.to_owned());
    }
    if let Some(effort) = effort {
        params["effort"] = Value::String(effort.to_owned());
    }
    json!({
        "method": "turn/start",
        "id": request_id,
        "params": params
    })
}

fn codex_prompt(mode: TurnPolicy, prompt: &str) -> String {
    match mode {
        TurnPolicy::ReadOnly | TurnPolicy::Agent => prompt.to_owned(),
    }
}

async fn begin_request(
    client: &mut CodexClient,
    request: &AgentRequest,
    selected_model: Option<&str>,
    selected_effort: Option<&str>,
    mode: TurnPolicy,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
) -> io::Result<()> {
    let context = OutputContext::from(request);
    if let Some(thread_id) = client.open_thread(cwd, request, event_tx).await? {
        send_event(
            event_tx,
            Some(&context),
            AgentEvent::ProviderSessionBound(thread_id),
        )
        .await;
    }
    client
        .start_turn(request, selected_model, selected_effort, mode)
        .await?;
    send_event(event_tx, Some(&context), AgentEvent::Started).await;
    Ok(())
}

fn thread_can_be_reused(
    current_conversation: Option<&ConversationId>,
    current_thread_id: Option<&str>,
    requested_conversation: &ConversationId,
    requested_resume_session: Option<&str>,
) -> bool {
    current_conversation == Some(requested_conversation)
        && requested_resume_session.is_some()
        && current_thread_id == requested_resume_session
}

fn conversation_requires_reconnect(
    current_conversation: Option<&ConversationId>,
    current_thread_id: Option<&str>,
    requested_conversation: &ConversationId,
    requested_resume_session: Option<&str>,
) -> bool {
    current_conversation.is_some()
        && !thread_can_be_reused(
            current_conversation,
            current_thread_id,
            requested_conversation,
            requested_resume_session,
        )
}

async fn connect_for_request(
    request: &AgentRequest,
    selected_model: Option<&str>,
    selected_effort: Option<&str>,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
    mcp_servers: &[dmuxai::mcp::ResolvedMcpServer],
) -> Option<CodexClient> {
    let context = OutputContext::from(request);
    match CodexClient::start(cwd, event_tx, mcp_servers).await {
        Ok(mut started) => {
            if let Err(error) = begin_request(
                &mut started,
                request,
                selected_model,
                selected_effort,
                request.policy,
                cwd,
                event_tx,
            )
            .await
            {
                send_event(
                    event_tx,
                    Some(&context),
                    AgentEvent::Error(error.to_string()),
                )
                .await;
                started.shutdown().await;
                None
            } else {
                Some(started)
            }
        }
        Err(error) => {
            send_event(
                event_tx,
                Some(&context),
                AgentEvent::Error(format!("could not start Codex: {error}")),
            )
            .await;
            None
        }
    }
}

async fn runtime_loop(
    mut command_rx: mpsc::Receiver<CodexCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
) {
    let mut client: Option<CodexClient> = None;
    let mut selected_model: Option<String> = None;
    let mut selected_effort: Option<String> = None;
    let mut model_catalog = Vec::new();

    loop {
        if client.is_none() {
            match command_rx.recv().await {
                Some(CodexCommand::Send(request)) => {
                    client = connect_for_request(
                        &request,
                        selected_model.as_deref(),
                        selected_effort.as_deref(),
                        &cwd,
                        &event_tx,
                        &mcp_servers,
                    )
                    .await;
                }
                Some(CodexCommand::Cancel) => {}
                Some(CodexCommand::SelectModel(model)) => {
                    selected_model = Some(model.clone());
                    send_event(&event_tx, None, AgentEvent::ModelSelected(model)).await;
                }
                Some(CodexCommand::SelectEffort(effort)) => {
                    selected_effort = Some(effort.clone());
                    send_event(&event_tx, None, AgentEvent::EffortSelected(effort)).await;
                }
                Some(CodexCommand::ListModels) => {
                    match CodexClient::start(&cwd, &event_tx, &mcp_servers).await {
                        Ok(mut started) => match started.list_models(&event_tx).await {
                            Ok((models, default_model)) => {
                                model_catalog = models.clone();
                                if selected_model.is_none() {
                                    selected_model = default_model;
                                }
                                if selected_effort.is_none() {
                                    selected_effort =
                                        selected_model.as_ref().and_then(|selected| {
                                            model_catalog
                                                .iter()
                                                .find(|model| &model.id == selected)
                                                .and_then(|model| model.default_effort.clone())
                                        });
                                }
                                send_event(
                                    &event_tx,
                                    None,
                                    AgentEvent::ModelsListed {
                                        models,
                                        selected: selected_model.clone(),
                                        selected_effort: selected_effort.clone(),
                                    },
                                )
                                .await;
                                client = Some(started);
                            }
                            Err(error) => {
                                send_event(&event_tx, None, AgentEvent::Error(error.to_string()))
                                    .await;
                                started.shutdown().await;
                            }
                        },
                        Err(error) => {
                            send_event(
                                &event_tx,
                                None,
                                AgentEvent::Error(format!("could not start Codex: {error}")),
                            )
                            .await;
                        }
                    }
                }
                Some(CodexCommand::RequestStatus) => {
                    match CodexClient::start(&cwd, &event_tx, &mcp_servers).await {
                        Ok(mut started) => {
                            let status = started.status(&event_tx).await;
                            send_event(&event_tx, None, AgentEvent::Status(status)).await;
                            client = Some(started);
                        }
                        Err(error) => {
                            send_event(
                            &event_tx,
                            None,
                            AgentEvent::Status(format!(
                                "Codex status\nToken activity: unavailable ({error})\nRate limits: unavailable ({error})"
                            )),
                        )
                        .await;
                        }
                    }
                }
                None => break,
            }
            continue;
        }

        let active_client = client.as_mut().expect("checked above");
        let mut reconnect_request = None;
        tokio::select! {
            command = command_rx.recv() => {
                match command {
                    Some(CodexCommand::Send(request)) => {
                        let context = OutputContext::from(&request);
                        if active_client.active_turn_id.is_some()
                            || active_client.pending_turn_request_id.is_some()
                        {
                            send_event(
                                &event_tx,
                                Some(&context),
                                AgentEvent::Error(
                                    "Codex is already running a turn; cancel it before sending another prompt"
                                        .to_owned(),
                                ),
                            ).await;
                        } else if conversation_requires_reconnect(
                            active_client.conversation_id.as_ref(),
                            active_client.thread_id.as_deref(),
                            &request.conversation_id,
                            request.resume_session.as_deref(),
                        ) {
                            reconnect_request = Some(request);
                        } else if let Err(error) = begin_request(
                            active_client,
                            &request,
                            selected_model.as_deref(),
                            selected_effort.as_deref(),
                            request.policy,
                            &cwd,
                            &event_tx,
                        ).await {
                            send_event(
                                &event_tx,
                                Some(&context),
                                AgentEvent::Error(error.to_string()),
                            ).await;
                        }
                    }
                    Some(CodexCommand::Cancel) => {
                        if let Err(error) = active_client.request_cancel().await {
                            let context = active_client.output_context.clone();
                            send_event(
                                &event_tx,
                                context.as_ref(),
                                AgentEvent::Error(error.to_string()),
                            ).await;
                        }
                    }
                    Some(CodexCommand::ListModels) => {
                        if active_client.active_turn_id.is_some()
                            || active_client.pending_turn_request_id.is_some()
                        {
                            send_event(
                                &event_tx,
                                None,
                                AgentEvent::Notice(
                                    "finish or cancel the Codex turn before loading models".to_owned(),
                                ),
                            ).await;
                        } else {
                            match active_client.list_models(&event_tx).await {
                                Ok((models, default_model)) => {
                                    model_catalog = models.clone();
                                    if selected_model.is_none() {
                                        selected_model = default_model;
                                    }
                                    if selected_effort.is_none() {
                                        selected_effort = selected_model.as_ref().and_then(|selected| {
                                            model_catalog
                                                .iter()
                                                .find(|model| &model.id == selected)
                                                .and_then(|model| model.default_effort.clone())
                                        });
                                    }
                                    send_event(
                                        &event_tx,
                                        None,
                                        AgentEvent::ModelsListed {
                                            models,
                                            selected: selected_model.clone(),
                                            selected_effort: selected_effort.clone(),
                                        },
                                    ).await;
                                }
                                Err(error) => {
                                    send_event(&event_tx, None, AgentEvent::Error(error.to_string())).await;
                                }
                            }
                        }
                    }
                    Some(CodexCommand::SelectModel(model)) => {
                        if active_client.active_turn_id.is_some()
                            || active_client.pending_turn_request_id.is_some()
                        {
                            send_event(
                                &event_tx,
                                None,
                                AgentEvent::Notice(
                                    "finish or cancel the Codex turn before changing models".to_owned(),
                                ),
                            ).await;
                        } else {
                            selected_model = Some(model.clone());
                            selected_effort = model_catalog
                                .iter()
                                .find(|candidate| candidate.id == model)
                                .and_then(|candidate| candidate.default_effort.clone());
                            send_event(&event_tx, None, AgentEvent::ModelSelected(model)).await;
                        }
                    }
                    Some(CodexCommand::SelectEffort(effort)) => {
                        if active_client.active_turn_id.is_some()
                            || active_client.pending_turn_request_id.is_some()
                        {
                            send_event(
                                &event_tx,
                                None,
                                AgentEvent::Notice(
                                    "finish or cancel the Codex turn before changing effort".to_owned(),
                                ),
                            )
                            .await;
                        } else {
                            selected_effort = Some(effort.clone());
                            send_event(&event_tx, None, AgentEvent::EffortSelected(effort)).await;
                        }
                    }
                    Some(CodexCommand::RequestStatus) => {
                        if active_client.active_turn_id.is_some()
                            || active_client.pending_turn_request_id.is_some()
                        {
                            send_event(
                                &event_tx,
                                None,
                                AgentEvent::Status(
                                    "Codex status\nToken activity: unavailable while a turn is active\nRate limits: unavailable while a turn is active".to_owned(),
                                ),
                            )
                            .await;
                        } else {
                            let status = active_client.status(&event_tx).await;
                            send_event(&event_tx, None, AgentEvent::Status(status)).await;
                        }
                    }
                    None => break,
                }
            }
            raw = active_client.raw_rx.recv() => {
                match raw {
                    Some(RawMessage::Json(message)) => {
                        handle_message(active_client, message, &event_tx).await;
                    }
                    Some(RawMessage::Stderr(_line)) => {}
                    Some(RawMessage::StdoutClosed) | None => {
                        send_event(
                            &event_tx,
                            active_client.output_context.as_ref(),
                            AgentEvent::Error("Codex app-server stopped unexpectedly".to_owned()),
                        ).await;
                        break;
                    }
                }
            }
        }

        if let Some(request) = reconnect_request {
            let previous = client.take().expect("a conversation client was active");
            previous.shutdown().await;
            client = connect_for_request(
                &request,
                selected_model.as_deref(),
                selected_effort.as_deref(),
                &cwd,
                &event_tx,
                &mcp_servers,
            )
            .await;
        }
    }

    if let Some(client) = client {
        client.shutdown().await;
    }
}

async fn handle_message(
    client: &mut CodexClient,
    message: Value,
    event_tx: &mpsc::Sender<AgentOutput>,
) {
    if is_pending_turn_response(client.pending_turn_request_id, &message) {
        client.pending_turn_request_id = None;
        if let Some(error) = response_error(&message) {
            let context = client.output_context.take();
            reset_turn(client);
            send_event(event_tx, context.as_ref(), AgentEvent::Error(error)).await;
            return;
        }
        client.active_turn_id = extract_turn_id(&message);
        return;
    }

    if message.get("method").and_then(Value::as_str) == Some("turn/started") {
        if client.output_context.is_none() {
            return;
        }
        if let Some(turn_id) = message.pointer("/params/turn/id").and_then(Value::as_str) {
            client.active_turn_id = Some(turn_id.to_owned());
        }
        client.turn_ready_for_interrupt = true;
        if client.cancel_requested
            && let Err(error) = client.send_interrupt_if_ready().await
        {
            send_event(
                event_tx,
                client.output_context.as_ref(),
                AgentEvent::Error(error.to_string()),
            )
            .await;
        }
        return;
    }

    let terminal = message.get("method").and_then(Value::as_str) == Some("turn/completed");
    if let Some(event) = normalized_event(&message) {
        if matches!(
            &event,
            AgentEvent::ToolProgress { summary, .. }
                if summary.starts_with("File changes")
        ) {
            client.implement_write_observed = true;
        }
        let context = client.output_context.clone();
        if terminal
            && client.active_mode == TurnPolicy::Agent
            && matches!(&event, AgentEvent::Completed)
            && !client.implement_write_observed
        {
            send_event(
                event_tx,
                context.as_ref(),
                AgentEvent::Error(
                    "implement mode completed without any observed file changes".to_owned(),
                ),
            )
            .await;
        } else {
            send_event(event_tx, context.as_ref(), event).await;
        }
    }
    if terminal {
        reset_turn(client);
        client.output_context = None;
    }
}

fn reset_turn(client: &mut CodexClient) {
    client.active_turn_id = None;
    client.pending_turn_request_id = None;
    client.turn_ready_for_interrupt = false;
    client.interrupt_sent = false;
    client.cancel_requested = false;
}

fn is_pending_turn_response(pending_request_id: Option<u64>, message: &Value) -> bool {
    pending_request_id.is_some() && pending_request_id == response_id(message)
}

async fn forward_notification(message: Value, event_tx: &mpsc::Sender<AgentOutput>) {
    if let Some(event) = normalized_event(&message) {
        send_event(event_tx, None, event).await;
    }
}

fn normalized_event(message: &Value) -> Option<AgentEvent> {
    match message.get("method")?.as_str()? {
        "item/agentMessage/delta" => message
            .pointer("/params/delta")
            .and_then(Value::as_str)
            .map(|delta| AgentEvent::TextDelta(delta.to_owned())),
        "item/started" | "item/completed" => normalize_tool_progress(message),
        "turn/completed" => match message
            .pointer("/params/turn/status")
            .and_then(Value::as_str)
        {
            Some("completed") => Some(AgentEvent::Completed),
            Some("interrupted") => Some(AgentEvent::Cancelled),
            Some("failed") => Some(AgentEvent::Error(
                message
                    .pointer("/params/turn/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex turn failed")
                    .to_owned(),
            )),
            _ => None,
        },
        "error"
            if !message
                .pointer("/params/willRetry")
                .and_then(Value::as_bool)
                .unwrap_or(false) =>
        {
            Some(AgentEvent::Error(
                message
                    .pointer("/params/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex reported an error")
                    .to_owned(),
            ))
        }
        _ => None,
    }
}

fn normalize_tool_progress(message: &Value) -> Option<AgentEvent> {
    let item = message.pointer("/params/item")?;
    let item_id = item.get("id")?.as_str()?.to_owned();
    let state = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if message.get("method").and_then(Value::as_str) == Some("item/started") {
                "in progress"
            } else {
                "completed"
            }
        })
        .to_owned();
    let summary = match item.get("type")?.as_str()? {
        "commandExecution" => format!(
            "Command: {}",
            bounded_summary(
                item.get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("command")
            )
        ),
        "mcpToolCall" => format!(
            "MCP: {}/{}",
            item.get("server")
                .and_then(Value::as_str)
                .unwrap_or("server"),
            item.get("tool").and_then(Value::as_str).unwrap_or("tool")
        ),
        "dynamicToolCall" => format!(
            "Tool: {}",
            item.get("tool").and_then(Value::as_str).unwrap_or("tool")
        ),
        "collabToolCall" => format!(
            "Collaboration: {}",
            item.get("tool").and_then(Value::as_str).unwrap_or("tool")
        ),
        "webSearch" => format!(
            "Web search: {}",
            bounded_summary(
                item.get("query")
                    .and_then(Value::as_str)
                    .unwrap_or("search")
            )
        ),
        "imageView" => format!(
            "View image: {}",
            bounded_summary(item.get("path").and_then(Value::as_str).unwrap_or("image"))
        ),
        "fileChange" => {
            let paths = item
                .get("changes")
                .and_then(Value::as_array)
                .map(|changes| {
                    changes
                        .iter()
                        .filter_map(|change| change.get("path").and_then(Value::as_str))
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|paths| !paths.is_empty())
                .unwrap_or_else(|| "files".to_owned());
            format!("File changes: {}", bounded_summary(&paths))
        }
        _ => return None,
    };
    Some(AgentEvent::ToolProgress {
        item_id,
        summary,
        state,
    })
}

fn bounded_summary(value: &str) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = single_line.chars();
    let prefix = chars.by_ref().take(TOOL_SUMMARY_LIMIT).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn response_id(message: &Value) -> Option<u64> {
    message.get("id").and_then(Value::as_u64)
}

fn response_error(message: &Value) -> Option<String> {
    message
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn parse_model_list(message: &Value) -> io::Result<(Vec<ModelInfo>, Option<String>)> {
    let data = message
        .pointer("/result/data")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("model/list response did not include a data array"))?;
    let mut selected = None;
    let models = data
        .iter()
        .filter_map(|model| {
            let id = model.get("model")?.as_str()?.to_owned();
            let label = model
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_owned();
            if model
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                selected = Some(id.clone());
            }
            let efforts: Vec<String> = model
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .map(|efforts| {
                    efforts
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let default_effort = model
                .get("defaultReasoningEffort")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|effort| efforts.iter().any(|supported| supported == effort));
            Some(ModelInfo::with_efforts(id, label, efforts, default_effort))
        })
        .collect::<Vec<_>>();
    Ok((models, selected))
}

fn format_usage(response: &Value) -> String {
    let summary = response.pointer("/result/summary");
    let lifetime = summary
        .and_then(|summary| summary.get("lifetimeTokens"))
        .and_then(Value::as_u64)
        .map(format_tokens)
        .unwrap_or_else(|| "unavailable".to_owned());
    let latest_daily = response
        .pointer("/result/dailyUsageBuckets")
        .and_then(Value::as_array)
        .and_then(|buckets| buckets.last())
        .and_then(|bucket| {
            Some((
                bucket.get("startDate")?.as_str()?,
                bucket.get("tokens")?.as_u64()?,
            ))
        })
        .map(|(date, tokens)| format!("{date}: {}", format_tokens(tokens)))
        .unwrap_or_else(|| "unavailable".to_owned());
    format!("Token activity: lifetime {lifetime}; latest day {latest_daily}")
}

fn format_rate_limits(response: &Value) -> String {
    let limits = response
        .pointer("/result/rateLimitsByLimitId")
        .and_then(Value::as_object);
    let entries = if let Some(limits) = limits {
        limits
            .iter()
            .map(|(id, limit)| (id.clone(), limit))
            .collect::<Vec<_>>()
    } else if let Some(limit) = response.pointer("/result/rateLimits") {
        vec![("codex".to_owned(), limit)]
    } else {
        return "Rate limits: unavailable".to_owned();
    };
    let rendered = entries
        .into_iter()
        .filter_map(|(id, limit)| {
            let used = limit.pointer("/primary/usedPercent")?.as_f64()?;
            let remaining = (100.0 - used).clamp(0.0, 100.0);
            let minutes = limit
                .pointer("/primary/windowDurationMins")
                .and_then(Value::as_u64)
                .map(|minutes| format!(" / {minutes}m"))
                .unwrap_or_default();
            let reset = limit
                .pointer("/primary/resetsAt")
                .and_then(Value::as_i64)
                .map(reset_description)
                .unwrap_or_else(|| "reset unavailable".to_owned());
            Some(format!("{id}: {remaining:.0}% remaining{minutes}; {reset}"))
        })
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        "Rate limits: unavailable".to_owned()
    } else {
        format!("Rate limits: {}", rendered.join("; "))
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn reset_description(resets_at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(resets_at);
    let minutes = (resets_at.saturating_sub(now) + 59) / 60;
    if minutes <= 0 {
        "reset due".to_owned()
    } else {
        format!("resets in {minutes}m")
    }
}

fn extract_thread_id(message: &Value) -> io::Result<String> {
    message
        .pointer("/result/thread/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("thread/start response did not include a thread id"))
}

fn extract_turn_id(message: &Value) -> Option<String> {
    message
        .pointer("/result/turn/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

async fn send_event(
    event_tx: &mpsc::Sender<AgentOutput>,
    context: Option<&OutputContext>,
    event: AgentEvent,
) {
    let _ = event_tx
        .send(AgentOutput {
            agent: Agent::Codex,
            conversation_id: context.map(|context| context.conversation_id.clone()),
            turn_id: context.map(|context| context.turn_id.clone()),
            event,
        })
        .await;
}

async fn read_stdout(stdout: ChildStdout, raw_tx: mpsc::Sender<RawMessage>) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str(&line) {
            Ok(message) => {
                if raw_tx.send(RawMessage::Json(message)).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                let message = json!({
                    "method": "error",
                    "params": {
                        "willRetry": false,
                        "error": { "message": format!("invalid Codex JSONL: {error}") }
                    }
                });
                if raw_tx.send(RawMessage::Json(message)).await.is_err() {
                    return;
                }
            }
        }
    }
    let _ = raw_tx.send(RawMessage::StdoutClosed).await;
}

async fn read_stderr(stderr: ChildStderr, raw_tx: mpsc::Sender<RawMessage>) {
    let mut lines: Lines<BufReader<ChildStderr>> = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if raw_tx.send(RawMessage::Stderr(line)).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(prompt: &str) -> AgentRequest {
        AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: prompt.to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        }
    }

    async fn idle_test_client() -> CodexClient {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .arg("--help")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let stdin = child
            .stdin
            .take()
            .expect("test child stdin should be piped");
        let (_raw_tx, raw_rx) = mpsc::channel(1);
        CodexClient {
            child,
            stdin,
            raw_rx,
            readers: Vec::new(),
            thread_id: Some("thread".to_owned()),
            conversation_id: Some(super::super::ConversationId::new("conversation")),
            output_context: None,
            active_turn_id: None,
            pending_turn_request_id: None,
            turn_ready_for_interrupt: false,
            interrupt_sent: false,
            cancel_requested: false,
            next_request_id: 1,
            active_mode: TurnPolicy::default(),
            implement_write_observed: false,
        }
    }

    #[test]
    fn extracts_thread_and_turn_ids_from_responses() {
        let thread = json!({ "id": 2, "result": { "thread": { "id": "thr_123" } } });
        let turn = json!({ "id": 3, "result": { "turn": { "id": "turn_456" } } });

        assert_eq!(extract_thread_id(&thread).unwrap(), "thr_123");
        assert_eq!(extract_turn_id(&turn).as_deref(), Some("turn_456"));
    }

    #[test]
    fn builds_fresh_and_resumed_thread_payloads() {
        let cwd = Path::new("workspace");
        let fresh = thread_open_request(10, cwd, None);
        let resumed = thread_open_request(11, cwd, Some("thr_existing"));

        assert_eq!(
            fresh.get("method").and_then(Value::as_str),
            Some("thread/start")
        );
        assert!(fresh.pointer("/params/threadId").is_none());
        assert_eq!(
            resumed.get("method").and_then(Value::as_str),
            Some("thread/resume")
        );
        assert_eq!(
            resumed.pointer("/params/threadId").and_then(Value::as_str),
            Some("thr_existing")
        );
        assert_eq!(
            resumed
                .pointer("/params/approvalPolicy")
                .and_then(Value::as_str),
            Some("never")
        );
        assert_eq!(
            resumed.pointer("/params/sandbox").and_then(Value::as_str),
            Some("read-only")
        );
    }

    #[test]
    fn conversation_thread_ownership_controls_reconnect() {
        let first = super::super::ConversationId::new("first");
        let second = super::super::ConversationId::new("second");

        assert!(!conversation_requires_reconnect(None, None, &first, None));
        assert!(!conversation_requires_reconnect(
            Some(&first),
            Some("thread"),
            &first,
            Some("thread")
        ));
        assert!(conversation_requires_reconnect(
            Some(&first),
            Some("thread"),
            &first,
            None
        ));
        assert!(conversation_requires_reconnect(
            Some(&first),
            Some("thread"),
            &first,
            Some("other-thread")
        ));
        assert!(conversation_requires_reconnect(
            Some(&first),
            Some("thread"),
            &second,
            Some("thread")
        ));
    }

    #[test]
    fn summary_projection_starts_a_fresh_native_thread() {
        let conversation = super::super::ConversationId::new("conversation");

        assert!(!thread_can_be_reused(
            Some(&conversation),
            Some("old-thread"),
            &conversation,
            None,
        ));
        let request = thread_open_request(12, Path::new("workspace"), None);
        assert_eq!(
            request.get("method").and_then(Value::as_str),
            Some("thread/start")
        );
        assert!(request.pointer("/params/threadId").is_none());
    }

    #[tokio::test]
    async fn late_turn_started_does_not_reactivate_an_idle_client() {
        let mut client = idle_test_client().await;
        let (event_tx, mut event_rx) = mpsc::channel(1);

        handle_message(
            &mut client,
            json!({
                "method": "turn/started",
                "params": { "turn": { "id": "late-turn" } }
            }),
            &event_tx,
        )
        .await;

        assert!(client.active_turn_id.is_none());
        assert!(!client.turn_ready_for_interrupt);
        assert!(event_rx.try_recv().is_err());
        client.shutdown().await;
    }

    #[test]
    fn normalizes_text_delta() {
        let message = json!({
            "method": "item/agentMessage/delta",
            "params": { "delta": "hello" }
        });

        assert_eq!(
            normalized_event(&message),
            Some(AgentEvent::TextDelta("hello".to_owned()))
        );
    }

    #[test]
    fn notification_without_an_id_is_not_a_pending_turn_response() {
        let notification = json!({
            "method": "item/agentMessage/delta",
            "params": { "delta": "hello" }
        });

        assert!(!is_pending_turn_response(None, &notification));
        assert!(!is_pending_turn_response(Some(3), &notification));
        assert!(is_pending_turn_response(
            Some(3),
            &json!({ "id": 3, "result": {} })
        ));
    }

    #[test]
    fn parses_codex_model_catalog_and_default() {
        let response = json!({
            "result": {
                "data": [
                    { "model": "gpt-a", "displayName": "GPT A", "isDefault": false },
                    { "model": "gpt-b", "displayName": "GPT B", "isDefault": true }
                ]
            }
        });

        let (models, selected) = parse_model_list(&response).unwrap();
        assert_eq!(
            models,
            vec![
                ModelInfo::new("gpt-a", "GPT A"),
                ModelInfo::new("gpt-b", "GPT B")
            ]
        );
        assert_eq!(selected.as_deref(), Some("gpt-b"));
    }

    #[test]
    // TC-040: selected effort is a per-turn override and preserves safety fields.
    fn codex_turn_request_includes_selected_effort() {
        let request = request("inspect this");
        let payload = turn_start_request(
            1,
            "thread",
            &request,
            Some("gpt-test"),
            Some("high"),
            TurnPolicy::ReadOnly,
        );

        assert_eq!(payload.pointer("/params/effort"), Some(&json!("high")));
        assert_eq!(
            payload.pointer("/params/approvalPolicy"),
            Some(&json!("never"))
        );
        assert_eq!(
            payload.pointer("/params/sandboxPolicy/type"),
            Some(&json!("readOnly"))
        );
    }

    #[test]
    // TC-042 and TC-043: only recognized tool items become concise lifecycle events.
    fn normalizes_codex_tool_item_lifecycle() {
        let started = json!({
            "method": "item/started",
            "params": { "item": {
                "id": "tool-1", "type": "commandExecution",
                "command": "rg --files", "status": "inProgress"
            }}
        });
        let completed = json!({
            "method": "item/completed",
            "params": { "item": {
                "id": "tool-1", "type": "commandExecution",
                "command": "rg --files", "status": "completed",
                "aggregatedOutput": "not rendered"
            }}
        });
        let malformed = json!({ "method": "item/started", "params": { "item": { "id": "x" } } });

        assert_eq!(
            normalized_event(&started),
            Some(AgentEvent::ToolProgress {
                item_id: "tool-1".to_owned(),
                summary: "Command: rg --files".to_owned(),
                state: "inProgress".to_owned(),
            })
        );
        assert_eq!(
            normalized_event(&completed),
            Some(AgentEvent::ToolProgress {
                item_id: "tool-1".to_owned(),
                summary: "Command: rg --files".to_owned(),
                state: "completed".to_owned(),
            })
        );
        assert_eq!(normalized_event(&malformed), None);
    }

    #[test]
    // TC-044 and TC-045: provider status can be complete or explicitly unavailable.
    fn codex_status_formats_usage_and_rate_limit_fixtures() {
        let usage = json!({
            "result": {
                "summary": { "lifetimeTokens": 12345 },
                "dailyUsageBuckets": [{ "startDate": "2026-08-24", "tokens": 321 }]
            }
        });
        let rates = json!({
            "result": { "rateLimits": {
                "primary": { "usedPercent": 25, "windowDurationMins": 15, "resetsAt": 0 }
            }}
        });

        assert_eq!(
            format_usage(&usage),
            "Token activity: lifetime 12.3K; latest day 2026-08-24: 321"
        );
        assert!(format_rate_limits(&rates).contains("75% remaining / 15m; reset due"));
        assert_eq!(
            format_usage(&json!({ "result": {} })),
            "Token activity: lifetime unavailable; latest day unavailable"
        );
        assert_eq!(
            format_rate_limits(&json!({ "result": {} })),
            "Rate limits: unavailable"
        );
    }

    #[test]
    fn normalizes_all_terminal_turn_statuses() {
        let completed = json!({
            "method": "turn/completed",
            "params": { "turn": { "status": "completed" } }
        });
        let interrupted = json!({
            "method": "turn/completed",
            "params": { "turn": { "status": "interrupted" } }
        });
        let failed = json!({
            "method": "turn/completed",
            "params": {
                "turn": { "status": "failed", "error": { "message": "boom" } }
            }
        });

        assert_eq!(normalized_event(&completed), Some(AgentEvent::Completed));
        assert_eq!(normalized_event(&interrupted), Some(AgentEvent::Cancelled));
        assert_eq!(
            normalized_event(&failed),
            Some(AgentEvent::Error("boom".to_owned()))
        );
    }

    #[test]
    fn ignores_retrying_errors_but_surfaces_final_errors() {
        let retrying = json!({
            "method": "error",
            "params": { "willRetry": true, "error": { "message": "retry" } }
        });
        let final_error = json!({
            "method": "error",
            "params": { "willRetry": false, "error": { "message": "failed" } }
        });

        assert_eq!(normalized_event(&retrying), None);
        assert_eq!(
            normalized_event(&final_error),
            Some(AgentEvent::Error("failed".to_owned()))
        );
    }

    #[test]
    // TC-046 / INT-040: App Server requires a tagged sandbox-policy object.
    fn codex_execution_modes_set_per_turn_sandbox_and_approval_policy() {
        let request = request("implement this");

        let ask = turn_start_request(1, "thread", &request, None, None, TurnPolicy::ReadOnly);
        assert_eq!(
            ask.pointer("/params/sandboxPolicy/type"),
            Some(&json!("readOnly"))
        );
        assert!(ask.pointer("/params/sandboxPolicy").unwrap().is_object());
        assert_eq!(ask.pointer("/params/approvalPolicy"), Some(&json!("never")));

        let read_only = turn_start_request(2, "thread", &request, None, None, TurnPolicy::ReadOnly);
        assert_eq!(
            read_only.pointer("/params/sandboxPolicy/type"),
            Some(&json!("readOnly"))
        );
        assert_eq!(
            read_only.pointer("/params/input/0/text"),
            Some(&json!("implement this"))
        );

        let implement = turn_start_request(
            3,
            "thread",
            &request,
            Some("model"),
            None,
            TurnPolicy::Agent,
        );
        assert_eq!(
            implement.pointer("/params/sandboxPolicy/type"),
            Some(&json!("workspaceWrite"))
        );
        assert!(
            implement
                .pointer("/params/sandboxPolicy")
                .unwrap()
                .is_object()
        );
        assert_eq!(implement.pointer("/params/model"), Some(&json!("model")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a locally installed and authenticated Codex CLI"]
    async fn real_codex_app_server_handshake() {
        let (event_tx, _event_rx) = mpsc::channel(8);
        let cwd = std::env::current_dir().unwrap();
        let client = CodexClient::start(&cwd, &event_tx, &[])
            .await
            .expect("the installed Codex app-server should complete its handshake");

        assert!(
            client.thread_id.is_none(),
            "handshake/model discovery must not create a conversation binding"
        );
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a locally installed and authenticated Codex CLI"]
    async fn real_codex_turn_streams_to_completion() {
        let (event_tx, mut event_rx) = mpsc::channel(128);
        let cwd = std::env::current_dir().unwrap();
        let mut client = CodexClient::start(&cwd, &event_tx, &[])
            .await
            .expect("the installed Codex app-server should complete its handshake");
        let request = request("Reply with exactly CODEX_OK. Do not use tools or modify files.");
        client.open_thread(&cwd, &request, &event_tx).await.unwrap();
        client
            .start_turn(&request, None, None, TurnPolicy::ReadOnly)
            .await
            .unwrap();

        let answer = tokio::time::timeout(Duration::from_secs(90), async {
            let mut answer = String::new();
            loop {
                let raw = client
                    .raw_rx
                    .recv()
                    .await
                    .expect("Codex output should remain connected");
                match raw {
                    RawMessage::Json(message) => {
                        handle_message(&mut client, message, &event_tx).await;
                    }
                    RawMessage::Stderr(line) => eprintln!("codex stderr: {line}"),
                    RawMessage::StdoutClosed => panic!("Codex stdout closed during the turn"),
                }

                while let Ok(output) = event_rx.try_recv() {
                    match output.event {
                        AgentEvent::TextDelta(delta) => answer.push_str(&delta),
                        AgentEvent::Completed => return answer,
                        AgentEvent::Error(error) => panic!("Codex turn failed: {error}"),
                        AgentEvent::Started
                        | AgentEvent::ProviderSessionBound(_)
                        | AgentEvent::Notice(_)
                        | AgentEvent::ModelsListed { .. }
                        | AgentEvent::ModelSelected(_)
                        | AgentEvent::EffortSelected(_)
                        | AgentEvent::ToolProgress { .. }
                        | AgentEvent::Activity(_)
                        | AgentEvent::Status(_)
                        | AgentEvent::Cancelled => {}
                    }
                }
            }
        })
        .await
        .expect("Codex should complete the smoke-test turn within 90 seconds");

        assert!(answer.contains("CODEX_OK"), "unexpected answer: {answer}");
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a locally installed and authenticated Codex CLI"]
    async fn real_codex_turn_can_be_interrupted() {
        let (event_tx, mut event_rx) = mpsc::channel(128);
        let cwd = std::env::current_dir().unwrap();
        let mut client = CodexClient::start(&cwd, &event_tx, &[])
            .await
            .expect("the installed Codex app-server should complete its handshake");
        let request = request(
            "Write a very detailed 5,000-word explanation of Rust ownership. Do not use tools.",
        );
        client.open_thread(&cwd, &request, &event_tx).await.unwrap();
        client
            .start_turn(&request, None, None, TurnPolicy::ReadOnly)
            .await
            .unwrap();
        client.request_cancel().await.unwrap();

        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let raw = client
                    .raw_rx
                    .recv()
                    .await
                    .expect("Codex output should remain connected");
                match raw {
                    RawMessage::Json(message) => {
                        handle_message(&mut client, message, &event_tx).await;
                    }
                    RawMessage::Stderr(line) => eprintln!("codex stderr: {line}"),
                    RawMessage::StdoutClosed => panic!("Codex stdout closed during cancellation"),
                }

                while let Ok(output) = event_rx.try_recv() {
                    match output.event {
                        AgentEvent::Cancelled => return,
                        AgentEvent::Completed => {
                            panic!("Codex completed before acknowledging the interrupt")
                        }
                        AgentEvent::Error(error) => panic!("Codex turn failed: {error}"),
                        AgentEvent::Started
                        | AgentEvent::ProviderSessionBound(_)
                        | AgentEvent::TextDelta(_)
                        | AgentEvent::Notice(_)
                        | AgentEvent::ModelsListed { .. }
                        | AgentEvent::ModelSelected(_)
                        | AgentEvent::EffortSelected(_) => {}
                        AgentEvent::ToolProgress { .. }
                        | AgentEvent::Activity(_)
                        | AgentEvent::Status(_) => {}
                    }
                }
            }
        })
        .await
        .expect("Codex should acknowledge turn/interrupt within 30 seconds");

        client.shutdown().await;
    }

    #[test]
    fn codex_argv_includes_mcp_config_overrides() {
        // TC-564, INT-561: Codex receives -c mcp_servers.* overrides before app-server --stdio.
        use dmuxai::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
        use std::collections::BTreeMap;

        // With no servers, defaults to app-server --stdio exactly
        assert_eq!(
            build_codex_app_server_args(&[]),
            vec!["app-server", "--stdio"]
        );

        let mut env = BTreeMap::new();
        env.insert("API_KEY".to_owned(), "secret".to_owned());

        let stdio_server = ResolvedMcpServer {
            name: "local".to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Stdio {
                command: "npx".to_owned(),
                args: vec!["-y".to_owned(), "server-pkg".to_owned()],
                env,
            },
            runners: None,
            tools: ToolNames::default(),
        };

        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_owned(), "bearer-token".to_owned());

        let http_server = ResolvedMcpServer {
            name: "remote".to_owned(),
            kind: ServerKind::Jira,
            transport: Transport::Http {
                url: "https://example.invalid/mcp".to_owned(),
                headers,
            },
            runners: Some(vec!["codex".to_owned(), "cursor".to_owned()]),
            tools: ToolNames::default(),
        };

        let cursor_only = ResolvedMcpServer {
            name: "cursor_only".to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Stdio {
                command: "cursor-tool".to_owned(),
                args: vec![],
                env: BTreeMap::new(),
            },
            runners: Some(vec!["cursor".to_owned()]),
            tools: ToolNames::default(),
        };

        let servers = vec![stdio_server, http_server, cursor_only];
        let args = build_codex_app_server_args(&servers);

        assert_eq!(args[args.len() - 2], "app-server");
        assert_eq!(args[args.len() - 1], "--stdio");

        assert!(args.contains(&"-c".to_owned()));
        assert!(args.contains(&"mcp_servers.local.command=\"npx\"".to_owned()));
        assert!(
            args.contains(&"mcp_servers.remote.url=\"https://example.invalid/mcp\"".to_owned())
        );

        // Filtered out because runners does not include codex
        assert!(!args.iter().any(|arg| arg.contains("cursor_only")));
    }

    #[test]
    fn codex_mcp_secrets_not_in_argv() {
        // TC-577, NFR-561, INT-561: servers with secret env (stdio) and secret headers (http).
        // Build Codex spawn argv and env: no secret value in argv; the values are in the child env;
        // config uses env-var-name keys.
        use dmuxai::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
        use std::collections::BTreeMap;

        let mut env = BTreeMap::new();
        env.insert(
            "SECRET_KEY".to_owned(),
            "super-secret-stdio-token".to_owned(),
        );

        let stdio_server = ResolvedMcpServer {
            name: "vault".to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Stdio {
                command: "vault-tool".to_owned(),
                args: vec![],
                env,
            },
            runners: None,
            tools: ToolNames::default(),
        };

        let mut headers = BTreeMap::new();
        headers.insert(
            "Authorization".to_owned(),
            "Bearer super-secret-http-token".to_owned(),
        );

        let http_server = ResolvedMcpServer {
            name: "remote-vault".to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Http {
                url: "https://vault.example.invalid/mcp".to_owned(),
                headers,
            },
            runners: None,
            tools: ToolNames::default(),
        };

        let servers = vec![stdio_server, http_server];
        let (args, child_env, notices) = build_codex_app_server_args_and_env(&servers);

        assert!(notices.is_empty(), "expected no notices for valid servers");

        // 1. No secret value in argv!
        let full_argv = args.join(" ");
        assert!(
            !full_argv.contains("super-secret-stdio-token"),
            "secret stdio token found in argv: {full_argv}"
        );
        assert!(
            !full_argv.contains("super-secret-http-token"),
            "secret http token found in argv: {full_argv}"
        );

        // 2. Secret values are in child env
        assert_eq!(
            child_env.get("SECRET_KEY").map(String::as_str),
            Some("super-secret-stdio-token")
        );
        assert!(
            child_env
                .values()
                .any(|v| v == "Bearer super-secret-http-token"),
            "child_env should contain the http secret value: {child_env:?}"
        );

        // 3. Config uses env-var-name keys (env_vars and env_http_headers)
        assert!(
            args.iter().any(|arg| arg.contains("mcp_servers.vault.env_vars")
                && arg.contains("SECRET_KEY")),
            "expected mcp_servers.vault.env_vars in argv: {args:?}"
        );
        assert!(
            args.iter().any(
                |arg| arg.contains("mcp_servers.remote-vault.env_http_headers")
                    && arg.contains("Authorization")
            ),
            "expected mcp_servers.remote-vault.env_http_headers in argv: {args:?}"
        );
    }
}
